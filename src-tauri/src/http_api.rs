// yxpil · BIT
use crate::state::Ctx;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::Emitter;

/// 启动/重启远程访问 HTTP 服务
pub async fn restart_server(ctx: &Arc<Ctx>) -> Result<String, String> {
    // 停止旧服务（先取出 JoinHandle，释放锁后再 await）
    let old_task = ctx.server_task.lock().unwrap().take();
    if let Some(task) = old_task {
        task.abort();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    // 旧中继循环必须一并终止：否则每次重启都泄漏一个 poller，
    // 多个 poller 轮流抢走隧道请求，泄漏循环一旦卡死请求就无人应答
    if let Some(t) = ctx.relay_task.lock().unwrap().take() {
        t.abort();
    }

    let cfg = ctx.config.lock().unwrap().clone();
    if !cfg.remote_enabled {
        // 远程关闭：中继循环一并停止
        if let Some(t) = ctx.relay_task.lock().unwrap().take() {
            t.abort();
        }
        return Ok("disabled".into());
    }

    let addr = cfg.listen_addr();
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => {
            // 绑定到配置端口：清空切换提示
            *ctx.port_switch.lock().unwrap() = None;
            l
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // 端口被占用：自动向后尝试 port+1..=+50，最后兜底临时端口；
            // 切换结果写入配置并通知前端告知用户
            let host = cfg.host.clone();
            let wanted_port = cfg.port;
            let mut found = None;
            for p in wanted_port + 1..=wanted_port + 50 {
                if let Ok(l) = tokio::net::TcpListener::bind(crate::config::join_host_port(&host, p)).await {
                    found = Some((l, p));
                    break;
                }
            }
            let (listener, new_port) = match found {
                Some(x) => x,
                // 兜底：内核分配临时端口（保证服务能起来）
                None => {
                    let l = tokio::net::TcpListener::bind(crate::config::join_host_port(&host, 0))
                        .await
                        .map_err(|e| format!("绑定 {addr} 失败（{wanted_port}..{} 均被占用）: {e}", wanted_port + 50))?;
                    let p = l.local_addr().map_err(|e| e.to_string())?.port();
                    (l, p)
                }
            };
            let new_addr = crate::config::join_host_port(&host, new_port);
            // 更新配置并落盘（单处真源：托盘/远程页/连通性测试都读配置）
            {
                let mut c = ctx.config.lock().unwrap();
                c.port = new_port;
                c.revision += 1;
            }
            ctx.save_config();
            *ctx.port_switch.lock().unwrap() = Some(wanted_port);
            crate::audit::record(ctx, "local-user", "remote.port_switch", "config", json!({ "from": wanted_port, "to": new_port }), true);
            let _ = ctx.app.emit("remote-port-switched", json!({ "from": wanted_port, "to": new_port, "addr": new_addr }));
            let _ = crate::tray::refresh(&ctx.app);
            eprintln!("[BIT] port {wanted_port} in use, switched to {new_port}");
            listener
        }
        Err(e) => return Err(format!("绑定 {addr} 失败: {e}")),
    };

    // local_addr 在 listener 被 move 进服务任务前取出
    let bound_port = listener
        .local_addr()
        .map_err(|e| format!("获取监听端口失败: {e}"))?
        .port();
    let router = build_router(ctx.clone());
    let task = tauri::async_runtime::spawn(async move {
        // with_connect_info：让 handler 能取到客户端 IP（对话限速按 IP 分桶）
        if let Err(e) = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        {
            eprintln!("[BIT] http server stopped: {e}");
        }
    });
    *ctx.server_task.lock().unwrap() = Some(task);
    // 设备凭证注册（幂等，先于服务与中继启动）：bitsign-v2 验签材料依赖它。首次启动
    // 采集硬件指纹 + 公网 IP（公开 API 竞速 5s）+ 时间戳派生并落盘；失败不阻断服务
    if let Err(e) = crate::state::device::ensure_device_key(&ctx).await {
        eprintln!("[BIT] device register failed (channel verify degraded): {e}");
    }
    // 云中继客户端：与 HTTP 服务同生命周期；内部动态读配置，未配置中继时不产生外联
    {
        let relay_ctx = ctx.clone();
        let relay_loop = tauri::async_runtime::spawn(async move { crate::relay::run_loop(relay_ctx).await });
        *ctx.relay_task.lock().unwrap() = Some(relay_loop);
    }
    Ok(crate::config::join_host_port(&cfg.host, bound_port))
}

pub fn build_router(ctx: Arc<Ctx>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/tools", get(list_tools).post(register_tool))
        .route("/api/tools/{id}", axum::routing::delete(remove_tool))
        .route("/api/tools/{id}/invoke", post(invoke_tool))
        .route("/api/subagents", get(list_subagents).post(spawn_subagent))
        .route("/api/chat", post(remote_chat))
        .route("/api/approvals", get(list_approvals))
        .route("/api/approvals/{id}", post(answer_approval))
        .route("/api/context/metrics", get(context_metrics_route))
        .route("/api/audit", get(list_audit))
        // 连接二维码 payload（手机端 / E2E 测试用，与 Tauri get_remote_qr 同源）
        .route("/api/qr", get(qr_route))
        // 自动更新：检测 / 状态 / 手动下载（启动后台任务已自动下，此处供远程管理用）
        .route("/api/update/check", get(update_check))
        .route("/api/update/status", get(update_status))
        .route("/api/update/download", post(update_download_route))
        // 调试接口（ADB 调试桥联动）：只读快照，供外部调试工具分析与控制 BIT
        .route("/api/debug/state", get(debug_state))
        .route("/api/debug/sessions", get(debug_sessions))
        .route("/api/debug/sessions/{id}", get(debug_session_detail))
        .route("/api/debug/goals", get(debug_goals))
        .route("/api/debug/mcp", get(debug_mcp))
        .route("/api/debug/interrupt", post(debug_interrupt))
        .route("/api/debug/config", post(debug_config))
        .route("/mcp", post(mcp_endpoint).delete(mcp_delete))
        // OpenAI 兼容端点：第三方 OpenAI 格式客户端可直接接入（API Key 填 Client Key）
        .route("/v1/models", get(openai_models))
        .route("/v1/chat/completions", post(openai_chat_completions))
        .layer(middleware::from_fn_with_state(ctx.clone(), auth))
        .with_state(ctx)
}

/// GET /api/qr：连接二维码 payload（与 Tauri get_remote_qr 同一数据源，含 128 位识别码
/// 与三种连接方式），手机端脚本化获取 / E2E 断言用
async fn qr_route(State(ctx): State<Arc<Ctx>>) -> Response {
    match crate::commands::qr_payload(&ctx).await {
        Ok(p) => Json(p).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "service": "BIT", "time": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string() }))
}

/// 更新检测（同 check_updates 命令，远程管理/自动化测试用）
async fn update_check() -> Response {
    match crate::update::fetch_latest().await {
        Ok(l) => {
            let has_update = crate::commands::version_gt(&l.version, env!("CARGO_PKG_VERSION"));
            Json(json!({
                "current": env!("CARGO_PKG_VERSION"),
                "latest": l.version,
                "has_update": has_update,
                "notes": l.notes,
                "url": l.url,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

/// 本地升级状态：有没有已下载的更新包
async fn update_status(State(ctx): State<Arc<Ctx>>) -> Response {
    match crate::update::read_state(&ctx) {
        Some(st) => Json(json!({
            "current": env!("CARGO_PKG_VERSION"),
            "downloaded": st["state"] == "downloaded",
            "update": st,
        }))
        .into_response(),
        None => Json(json!({ "current": env!("CARGO_PKG_VERSION"), "downloaded": false })).into_response(),
    }
}

/// 触发下载当前平台更新包（幂等：已下载同版本直接返回）
async fn update_download_route(State(ctx): State<Arc<Ctx>>) -> Response {
    match crate::update::download_update(&ctx).await {
        Ok(status) => Json(status).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

/// BIT 支持的 MCP 协议版本（Streamable HTTP / JSON-RPC 2.0）。
/// 协商规则：客户端 initialize 请求的版本在列表内则回显同版本，否则返回最新支持版
const MCP_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
/// MCP 会话空闲过期时间（懒清理）
const MCP_SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// 会话数上限（超出清最旧，防未认证洪泛堆积）
const MCP_SESSION_MAX: usize = 512;

fn mcp_negotiate_version(client: &str) -> &'static str {
    match MCP_VERSIONS.iter().find(|v| **v == client) {
        Some(v) => v,
        None => MCP_VERSIONS[MCP_VERSIONS.len() - 1],
    }
}

fn gen_mcp_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("mcp-{nanos:x}-{:x}-{seq:x}", std::process::id())
}

/// 会话校验（对 map 的纯函数，便于单测）：
/// - 未携带 Mcp-Session-Id → 400（会话建立后规范要求客户端所有请求都携带）
/// - 未知 / 已过期会话 → 404（客户端应重新 initialize）
/// - 有效 → 刷新活跃时间
fn mcp_check_session_map(
    map: &mut std::collections::HashMap<String, std::time::Instant>,
    sid: Option<&str>,
    ttl: std::time::Duration,
) -> Result<(), StatusCode> {
    let Some(sid) = sid else {
        return Err(StatusCode::BAD_REQUEST);
    };
    map.retain(|_, t| t.elapsed() < ttl);
    match map.get_mut(sid) {
        Some(t) => {
            *t = std::time::Instant::now();
            Ok(())
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// 单条 JSON-RPC 消息处理。返回 (状态码, 可选响应体, 新分配的会话 id)
async fn mcp_handle_one(
    ctx: &Arc<Ctx>,
    headers: &HeaderMap,
    msg: &serde_json::Value,
) -> (StatusCode, Option<serde_json::Value>, Option<String>) {
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let id = msg.get("id").cloned();
    // JSON-RPC 2.0：无 id 字段 = 通知，服务器不回 body（Streamable HTTP 返回 202）
    let is_notification = id.is_none();
    let rpc_ok = |result: serde_json::Value| {
        json!({ "jsonrpc": "2.0", "id": id.clone().unwrap_or(serde_json::Value::Null), "result": result })
    };
    let rpc_err = |code: i64, m: &str| {
        json!({ "jsonrpc": "2.0", "id": id.clone().unwrap_or(serde_json::Value::Null), "error": { "code": code, "message": m } })
    };

    match method {
        "initialize" => {
            if is_notification {
                return (StatusCode::ACCEPTED, None, None);
            }
            let client_ver = msg
                .pointer("/params/protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ver = mcp_negotiate_version(client_ver);
            let sid = gen_mcp_session_id();
            let mut sessions = ctx.mcp_sessions.lock().unwrap();
            if sessions.len() >= MCP_SESSION_MAX {
                if let Some(oldest) = sessions.iter().min_by_key(|(_, t)| *t).map(|(k, _)| k.clone()) {
                    sessions.remove(&oldest);
                }
            }
            sessions.insert(sid.clone(), std::time::Instant::now());
            drop(sessions);
            (
                StatusCode::OK,
                Some(rpc_ok(json!({
                    "protocolVersion": ver,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "BIT", "version": env!("CARGO_PKG_VERSION") }
                }))),
                Some(sid),
            )
        }
        "ping" => {
            if let Err(st) = mcp_check_session_map(
                &mut ctx.mcp_sessions.lock().unwrap(),
                headers.get("mcp-session-id").and_then(|v| v.to_str().ok()),
                MCP_SESSION_TTL,
            ) {
                return (st, None, None);
            }
            (StatusCode::OK, Some(rpc_ok(json!({}))), None)
        }
        "tools/list" => {
            if let Err(st) = mcp_check_session_map(
                &mut ctx.mcp_sessions.lock().unwrap(),
                headers.get("mcp-session-id").and_then(|v| v.to_str().ok()),
                MCP_SESSION_TTL,
            ) {
                return (st, None, None);
            }
            let tools = ctx.tools.lock().unwrap();
            let list: Vec<serde_json::Value> = tools
                .iter()
                .filter(|t| t.enabled)
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.parameters,
                    })
                })
                .collect();
            (StatusCode::OK, Some(rpc_ok(json!({ "tools": list }))), None)
        }
        "tools/call" => {
            if let Err(st) = mcp_check_session_map(
                &mut ctx.mcp_sessions.lock().unwrap(),
                headers.get("mcp-session-id").and_then(|v| v.to_str().ok()),
                MCP_SESSION_TTL,
            ) {
                return (st, None, None);
            }
            let name = msg
                .pointer("/params/name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or(json!({}));
            let found = {
                let tools = ctx.tools.lock().unwrap();
                tools
                    .iter()
                    .find(|t| t.name == name)
                    .map(|t| (t.id.clone(), t.enabled))
            };
            match found {
                None => (
                    StatusCode::OK,
                    Some(rpc_err(-32602, "tool not found")),
                    None,
                ),
                Some((_, false)) => (
                    StatusCode::OK,
                    Some(rpc_err(-32000, "tool is paused")),
                    None,
                ),
                Some((tid, _)) => {
                    match crate::registry::invoke(&ctx, &tid, args, "mcp-client", None).await {
                        Ok(v) => (
                            StatusCode::OK,
                            Some(rpc_ok(json!({
                                "content": [{ "type": "text", "text": v.to_string() }],
                                "isError": false
                            }))),
                            None,
                        ),
                        Err(e) => (
                            StatusCode::OK,
                            Some(rpc_ok(json!({
                                "content": [{ "type": "text", "text": e }],
                                "isError": true
                            }))),
                            None,
                        ),
                    }
                }
            }
        }
        // 任意通知（notifications/initialized、notifications/cancelled、未知通知）→ 202 无 body
        m if m.starts_with("notifications/") || is_notification => {
            (StatusCode::ACCEPTED, None, None)
        }
        "" => (
            StatusCode::BAD_REQUEST,
            Some(rpc_err(-32600, "missing method")),
            None,
        ),
        _ => (StatusCode::OK, Some(rpc_err(-32601, "method not found")), None),
    }
}

/// BIT 自身作为 MCP 服务器（Streamable HTTP / JSON-RPC 2.0，支持 2024-11-05 / 2025-03-26 / 2025-06-18 协商）：
/// 任何标准 MCP 客户端（Claude Desktop、Cherry Studio、MCP Inspector、BIT 自己的自动发现）都可接入 BIT 的全部启用工具。
/// 会话：initialize 响应头返回 Mcp-Session-Id，后续请求必须携带（缺失 400 / 过期 404）；
/// DELETE /mcp 终止会话；GET 返回 405（不提供服务器推送流）；POST 数组按 JSON-RPC batch 处理。
/// 认证：Bearer Client Key 或 ?key=（见 auth 中间件）。
async fn mcp_endpoint(
    State(ctx): State<Arc<Ctx>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    // JSON-RPC batch（数组）：逐条处理，通知不产生 body，全部为通知时返回 202
    if let serde_json::Value::Array(msgs) = &body {
        let mut bodies: Vec<serde_json::Value> = Vec::new();
        let mut sid_out: Option<String> = None;
        for m in msgs {
            let (_, resp, sid) = mcp_handle_one(&ctx, &headers, m).await;
            if let Some(s) = sid {
                sid_out = Some(s);
            }
            if let Some(b) = resp {
                bodies.push(b);
            }
        }
        let mut resp = if bodies.is_empty() {
            StatusCode::ACCEPTED.into_response()
        } else {
            Json(bodies).into_response()
        };
        if let Some(s) = sid_out {
            if let Ok(v) = HeaderValue::from_str(&s) {
                resp.headers_mut().insert("Mcp-Session-Id", v);
            }
        }
        return resp;
    }

    let (status, resp_body, sid) = mcp_handle_one(&ctx, &headers, &body).await;
    let mut resp = match resp_body {
        Some(b) => (status, Json(b)).into_response(),
        None => status.into_response(),
    };
    if let Some(s) = sid {
        if let Ok(v) = HeaderValue::from_str(&s) {
            resp.headers_mut().insert("Mcp-Session-Id", v);
        }
    }
    resp
}

/// DELETE /mcp：客户端显式终止会话
async fn mcp_delete(State(ctx): State<Arc<Ctx>>, headers: HeaderMap) -> Response {
    match headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) {
        Some(sid) => {
            if ctx.mcp_sessions.lock().unwrap().remove(sid).is_some() {
                StatusCode::OK.into_response()
            } else {
                StatusCode::NOT_FOUND.into_response()
            }
        }
        None => StatusCode::BAD_REQUEST.into_response(),
    }
}

/// 来源 IP 网段归并（与 relay-worker / fake_relay 的 ipPrefix 完全同口径）：
/// v4 取前 3 段（/24），v6 取前 4 组（/48），回环原样，空 → "unknown"。
/// 口径必须与 Worker 端许可盖章逐字符一致——BIT 端比对"真实来源 IP 的网段"与
/// Worker 盖章的许可网段（x-bit-permit-pfx），实现差异会造成误杀正常请求
fn ip_prefix(ip: &str) -> String {
    if ip.is_empty() {
        return "unknown".to_string();
    }
    if ip == "::1" || ip == "127.0.0.1" {
        return ip.to_string();
    }
    if ip.contains(':') {
        // 与 JS ip.split(":").slice(0, 4).join(":") 同语义（含 "::" 在前 4 组时的边缘形态）
        let parts: Vec<&str> = ip.split(':').collect();
        return parts.iter().take(4).copied().collect::<Vec<_>>().join(":");
    }
    let o: Vec<&str> = ip.split('.').collect();
    if o.len() == 4 {
        o[..3].join(".")
    } else {
        ip.to_string()
    }
}

/// 鉴权校验（纯函数，便于单测）：
/// - /api/health 免鉴权
/// - 第一重：Bearer Client Key 或 ?key= 查询参数；Client Key 未配置一律拒绝（防止空 key 绕过）
/// - 第二重：X-Access-Password（/v1/ OpenAI 兼容端点与 /mcp 端点豁免——OpenAI 客户端无法携带自定义头）
/// 返回 Err(状态码) 表示拒绝
fn check_auth(
    cfg: &crate::config::Config,
    path: &str,
    bearer: &str,
    qkey: &str,
    access_password: &str,
) -> Result<(), StatusCode> {
    if path == "/api/health" {
        return Ok(());
    }
    if cfg.client_key.is_empty() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if !cfg.verify_client_key(bearer) && !cfg.verify_client_key(qkey) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let openai_endpoint = path.starts_with("/v1/");
    let mcp_endpoint = path == "/mcp";
    if !openai_endpoint && !mcp_endpoint && !cfg.verify_access_password(access_password) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

/// 双重认证：Bearer Client Key + X-Access-Password 访问密码，含 HTTP 请求审计。
/// OpenAI 兼容端点（/v1/）例外：OpenAI 客户端无法携带自定义头，仅校验 Client Key。
async fn auth(State(ctx): State<Arc<Ctx>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let provided = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_string();
    // 兼容 ?key= 查询参数（自动发现/无 header 能力的客户端）
    let qkey = req
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|p| p.strip_prefix("key=")))
        .unwrap_or("");
    let access_password = req
        .headers()
        .get("x-access-password")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cfg = ctx.config.lock().unwrap().clone();

    if let Err(status) = check_auth(&cfg, &path, &provided, &qkey, access_password) {
        if status == StatusCode::UNAUTHORIZED {
            let reason = if !cfg.verify_client_key(&provided) && !cfg.verify_client_key(&qkey) {
                "client_key"
            } else {
                "access_password"
            };
            crate::audit::record(
                &ctx,
                &actor_of(&provided),
                "http.auth_failed",
                &path,
                json!({ "reason": reason }),
                false,
            );
            let body = if reason == "client_key" {
                Json(json!({ "error": { "message": "无效的 API Key（BIT Client Key）", "type": "invalid_request_error", "code": "invalid_api_key" } }))
            } else {
                Json(json!({ "error": "访问密码错误或缺失（需 X-Access-Password 头）" }))
            };
            return (status, body).into_response();
        }
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "BIT 尚未配置 Client Key，远程访问已禁用" })),
        )
            .into_response();
    }

    // 信道防护（bitsign-v2）：x-bit-via 头由本机中继循环回环时添加（直连请求没有），
    // 命中即视为"经云中继进入"→ 强制验签。第三方 App 无法算出合法签名，借道中继直接 403；
    // LAN 直连 / 本机访问不受影响（第三方 OpenAI 客户端兼容保留）。
    // 验签通过后返回经中继进入时附带的真实手机端 IP（x-bit-client-ip，本机 poller 注入）
    let via_relay = req.headers().get("x-bit-via").and_then(|v| v.to_str().ok()) == Some("relay");
    let mut relay_client_ip: Option<String> = None;
    // 敏感端点中继隔离：/api/qr（响应含 client_key/识别码/局域网地址）与 /api/debug/*
    // （可远程改配置——关防护/关审核/换上游）绝不允许经云中继触达。放在验签之前：
    // channel_guard 关闭时同样生效（与站点门槛同理：客户端开关绕不过），
    // 与 Worker 站点层路径拒绝互为纵深（私有中继也拦得住）
    if via_relay && (path == "/api/qr" || path.starts_with("/api/debug")) {
        crate::audit::record(&ctx, &actor_of(&provided), "http.channel_sensitive", &path, json!({}), false);
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "sensitive endpoint not available over relay channel" })),
        )
            .into_response();
    }
    // 中继通道功能收窄（chat-only）：经中继只允许"远程下发对话 + 看回复"这一条最小闭环
    // ——聊天端点与健康探测；其余路径（工具/文件/审批/更新/MCP/审计/调试…）一律 403。
    // 最小攻击面：即使请求签名与许可全部合法，能做的也只有对话本身，
    // 中继面上不存在"其它功能"可被利用。放在鉴权之后：无凭据仍返回 401，
    // 有凭据打非聊天路径才吃到这里的 403
    const RELAY_ALLOW: [&str; 4] = ["/api/chat", "/v1/chat/completions", "/v1/models", "/api/health"];
    if via_relay && !RELAY_ALLOW.contains(&path.as_str()) {
        crate::audit::record(&ctx, &actor_of(&provided), "http.relay_path_denied", &path, json!({}), false);
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "relay tunnel is chat-only (command & reply)" })),
        )
            .into_response();
    }
    // 来源一致性闸门（端到端 IP 比对，确保真实性才放行）：比对"本次请求的真实来源 IP"
    // （x-bit-client-ip ← Worker cf-connecting-ip）与"许可签发时绑定的网段"（x-bit-permit-pfx，
    // ← Worker 许可记录盖章，非现算）。官方中继在许可校验通过后必盖章：缺章或与真实
    // 来源不同段 = 中继实现被替换或被破坏，拒绝放行。两个头都只有本机 poller 从中继
    // 信封注入（PASS_HEADERS 白名单外），手机端无法伪造
    if via_relay {
        let cip = req.headers().get("x-bit-client-ip").and_then(|v| v.to_str().ok()).unwrap_or("");
        let cpfx = req.headers().get("x-bit-permit-pfx").and_then(|v| v.to_str().ok()).unwrap_or("");
        if cip.is_empty() || cpfx.is_empty() || cpfx != ip_prefix(cip) {
            crate::audit::record(&ctx, &actor_of(&provided), "http.relay_ip_mismatch", &path, json!({ "ip": cip, "pfx": cpfx }), false);
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "source ip inconsistent with permit" })),
            )
                .into_response();
        }
    }
    if via_relay && cfg.channel_guard {
        let rid = req.headers().get("x-bit-rid").and_then(|v| v.to_str().ok()).unwrap_or("");
        let ts = req.headers().get("x-bit-ts").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        let nonce = req.headers().get("x-bit-nonce").and_then(|v| v.to_str().ok()).unwrap_or("");
        let sign = req.headers().get("x-bit-sign").and_then(|v| v.to_str().ok()).unwrap_or("");
        let now = chrono::Utc::now().timestamp();
        let check = crate::security::BitsignCheck { ts, nonce: nonce.to_string(), sign: sign.to_string() };
        // 验签材料：设备凭证（bitsign-v2）——未注册时用空材料（理论上不会发生：启动即注册）
        let material = crate::state::device::sig_material(cfg.device_key.as_deref().unwrap_or(""));
        // 验签 path = 本机请求路径（不含 query，与手机端签名口径一致）
        match crate::security::bitsign_verify(&cfg.client_key, &material, rid, req.method().as_str(), req.uri().path(), &check, now) {
            Ok(()) => {
                relay_client_ip = req
                    .headers()
                    .get("x-bit-client-ip")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<std::net::IpAddr>().ok())
                    .map(|ip| ip.to_string());
                // nonce 防重放：同一识别码下每个 nonce 只允许用一次（缓存窗口=签名时间窗）
                let key = format!("{rid}|{nonce}");
                if !ctx.nonce_seen.lock().unwrap().fresh(&key) {
                    crate::audit::record(&ctx, &actor_of(&provided), "http.channel_replay", &path, json!({ "rid": rid }), false);
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({ "error": "channel nonce replayed" })),
                    )
                        .into_response();
                }
            }
            Err(reason) => {
                crate::audit::record(&ctx, &actor_of(&provided), "http.channel_denied", &path, json!({ "rid": rid, "reason": reason }), false);
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": format!("channel signature invalid: {reason}") })),
                )
                    .into_response();
            }
        }
    }

    // IP 黑名单：命中直接 403（防滥用第一道闸；黑名单在 debug_config / 设置页维护）。
    // 经验签的中继请求按真实手机端 IP（envelope.ip ← Worker cf-connecting-ip）计；
    // 直连 / 未验签请求按 socket 地址——伪造头无法绕过黑名单（需先过 bitsign）
    if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<std::net::SocketAddr>>() {
        let ip = relay_client_ip.clone().unwrap_or_else(|| addr.ip().to_string());
        let blocked = cfg.ip_blocklist.as_ref().map(|l| l.iter().any(|b| *b == ip)).unwrap_or(false);
        if blocked {
            crate::audit::record(&ctx, &actor_of(&provided), "http.ip_blocked", &path, json!({ "ip": ip }), false);
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "client ip blocked" })),
            )
                .into_response();
        }
    }

    let actor = actor_of(&provided);
    let method = req.method().to_string();
    crate::audit::record(
        &ctx,
        &actor,
        "http.request",
        &path,
        json!({ "method": method }),
        true,
    );
    let resp = next.run(req).await;
    // 中继响应只放行文本类（JSON / text / SSE）：二进制内容（图片/文件/任意流）不经云中继
    // 返回——信道只传文字，防被当作二进制外传管道（请求侧由 Worker 415 拦截，此为响应侧兜底）
    if via_relay {
        let binary = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| {
                let ct = ct.to_ascii_lowercase();
                !(ct.starts_with("application/json")
                    || ct.starts_with("text/")
                    || ct.is_empty())
            })
            .unwrap_or(false);
        if binary {
            crate::audit::record(&ctx, &actor, "http.channel_binary_blocked", &path, json!({}), false);
            return (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                Json(json!({ "error": "relay channel is text-only" })),
            )
                .into_response();
        }
    }
    resp
}

fn actor_of(key: &str) -> String {
    let prefix: String = key.chars().skip(4).take(8).collect();
    if prefix.is_empty() {
        "agent:unknown".to_string()
    } else {
        format!("agent:{prefix}")
    }
}

/// 对话限速 / 并发计数用的客户端 IP：经验签的中继请求取真实手机端 IP（x-bit-client-ip，
/// 由本机 poller 注入，来源 = Worker cf-connecting-ip；签名验证在 auth 中间件已完成），
/// 直连请求按 socket 地址。channel_guard 关闭时中继请求未经验签，退回 socket 地址防伪造
fn chat_client_ip(ctx: &Arc<Ctx>, headers: &HeaderMap, addr: &std::net::IpAddr) -> String {
    let via = headers.get("x-bit-via").and_then(|v| v.to_str().ok()) == Some("relay");
    let guard = ctx.config.lock().unwrap().channel_guard;
    if via && guard {
        if let Some(v) = headers.get("x-bit-client-ip").and_then(|v| v.to_str().ok()) {
            if let Ok(ip) = v.parse::<std::net::IpAddr>() {
                return ip.to_string();
            }
        }
    }
    addr.to_string()
}

async fn list_tools(State(ctx): State<Arc<Ctx>>) -> Json<serde_json::Value> {
    let tools = ctx.tools.lock().unwrap().clone();
    Json(json!({ "tools": tools }))
}

#[derive(serde::Deserialize)]
struct RegisterReq {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parameters: serde_json::Value,
    /// Agent 提供的回调端点，BIT 调用工具时会 POST 到该地址
    url: String,
}

async fn register_tool(
    State(ctx): State<Arc<Ctx>>,
    headers: HeaderMap,
    Json(req): Json<RegisterReq>,
) -> Response {
    let actor = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| actor_of(v.strip_prefix("Bearer ").unwrap_or("")))
        .unwrap_or_else(|| "agent:unknown".into());

    if req.url.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "缺少回调 url" })),
        )
            .into_response();
    }

    match crate::registry::register(
        &ctx,
        &req.name,
        &req.description,
        req.parameters,
        crate::registry::ToolKind::Remote { url: req.url.trim().to_string() },
        &actor,
    ) {
        Ok(tool) => {
            crate::audit::record(
                &ctx,
                &actor,
                "tool.register",
                &tool.name,
                json!({ "url": req.url }),
                true,
            );
            (StatusCode::CREATED, Json(json!({ "tool": tool }))).into_response()
        }
        Err(e) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

async fn remove_tool(State(ctx): State<Arc<Ctx>>, Path(id): Path<String>) -> Response {
    match crate::registry::remove(&ctx, &id) {
        Ok(removed) => {
            crate::audit::record(&ctx, "remote", "tool.remove", &removed, json!({}), true);
            Json(json!({ "removed": removed })).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

/// GET /api/approvals：列出待审批的工具调用（远程客户端轮询用；本地 UI 走 tool-approval 事件）
async fn list_approvals(State(ctx): State<Arc<Ctx>>) -> Response {
    let map = ctx.approvals.lock().unwrap();
    let mut items: Vec<_> = map
        .iter()
        .map(|(id, p)| {
            json!({
                "id": id,
                "tool": p.tool,
                "params": p.params,
                "age_secs": p.created.elapsed().as_secs(),
            })
        })
        .collect();
    drop(map);
    // ap-N 自增 id 按数值排序（字典序会把 ap-10 排在 ap-2 前）
    items.sort_by_key(|x| {
        x["id"]
            .as_str()
            .unwrap_or_default()
            .strip_prefix("ap-")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    });
    Json(json!({ "approvals": items })).into_response()
}

/// POST /api/approvals/{id}：应答审批请求（body: {"allow": bool}）。本地 UI 与远程客户端共用同一张审批表
async fn answer_approval(
    State(ctx): State<Arc<Ctx>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let allow = match body.get("allow").and_then(|v| v.as_bool()) {
        Some(b) => b,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Missing boolean field `allow`" })),
            )
                .into_response()
        }
    };
    let sender = ctx.approvals.lock().unwrap().remove(&id).map(|p| p.tx);
    match sender {
        Some(tx) => {
            let _ = tx.send(allow);
            Json(json!({ "id": id, "allow": allow })).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Approval request not found or already answered" })),
        )
            .into_response(),
    }
}

async fn invoke_tool(
    State(ctx): State<Arc<Ctx>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let actor = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| actor_of(v.strip_prefix("Bearer ").unwrap_or("")))
        .unwrap_or_else(|| "agent:unknown".into());

    let params = body.get("params").cloned().unwrap_or(json!({}));
    // 审批模式真实生效：ask/auto 下非安全工具须先经审批（本地弹卡片 / 远程轮询应答），
    // 否则远程客户端可绕过审批直接调用 shell 等危险工具。allow_all 一律放行
    let tool_name = {
        let tools = ctx.tools.lock().unwrap();
        tools
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| id.clone())
    };
    let mode = ctx.config.lock().unwrap().tool_approval.clone();
    if !crate::agent::auto_pass(&mode, &tool_name) {
        if let Err(e) =
            crate::agent::request_approval(&ctx, &tool_name, &params, None, &actor).await
        {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": e })),
            )
                .into_response();
        }
    }
    match crate::registry::invoke(&ctx, &id, params, &actor, None).await {
        Ok(result) => Json(json!({ "result": result })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

// ==================== 子代理（宿主调度）====================
// 模型侧没有 sub_agent 工具，远程宿主通过这里派生 / 查看在途子代理。
// 均在 /api/* 双重认证（Client Key + 访问密码）之下。

/// 在途子代理数量
async fn list_subagents() -> Json<serde_json::Value> {
    Json(json!({
        "running": crate::registry::subagent_depth(),
        "max_concurrent": crate::delegation::MAX_CONCURRENT,
    }))
}

/// 派生子代理：body { task, title?, session_id? }，阻塞到子代理跑完（内部上限 15 分钟）
async fn spawn_subagent(
    State(ctx): State<Arc<Ctx>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let actor = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| actor_of(v.strip_prefix("Bearer ").unwrap_or("")))
        .unwrap_or_else(|| "agent:unknown".into());
    let task = body.get("task").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if task.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "task is required" }))).into_response();
    }
    let title = body.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
    let parent = body
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    crate::audit::record(
        &ctx,
        &actor,
        "subagent.spawn",
        parent.as_deref().unwrap_or("global"),
        json!({ "task_chars": task.chars().count(), "title": title }),
        true,
    );
    match crate::delegation::spawn(&ctx, parent.as_deref(), &task, title.as_deref()).await {
        Ok(result) => Json(json!({ "result": result })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

async fn list_audit(State(ctx): State<Arc<Ctx>>) -> Json<serde_json::Value> {
    let log = ctx.audit.lock().unwrap().clone();
    Json(json!({ "entries": log }))
}

// ==================== 调试接口（ADB 调试桥联动）====================
// 供 ADB（github.com/yxpil/ADB）等调试工具只读拉取 BIT 内部状态做分析。
// 全部在 /api/* 双重认证之下：Client Key（Bearer / ?key=）+ X-Access-Password。

/// Client Key / API Key 脱敏：前 6 位 + 长度，完整密钥不出机器
fn key_hint(key: &str) -> String {
    let n = key.chars().count();
    if n <= 6 {
        "***".into()
    } else {
        let head: String = key.chars().take(6).collect();
        format!("{head}…({n})")
    }
}

/// GET /api/debug/state：运行快照——激活 Provider（密钥脱敏）、工具注册表、
/// MCP 服务器、会话/记忆/技能计数、远程访问配置。ADB「BIT」页的数据源。
async fn debug_state(State(ctx): State<Arc<Ctx>>) -> Response {
    let ai = ctx.ai_config.lock().unwrap().clone();
    let tools = ctx.tools.lock().unwrap().clone();
    let mcp = ctx.mcp.lock().unwrap().clone();
    let sessions = ctx.sessions.lock().unwrap().clone();
    let memories = ctx.memories.lock().unwrap().len();
    let skills = ctx.skills.lock().unwrap().len();
    let cfg = ctx.config.lock().unwrap().clone();
    let active = ai.providers.iter().find(|p| p.active).map(|p| {
        json!({
            "name": p.name,
            "protocol": p.protocol,
            "model": p.model,
            "base_url": p.base_url,
            "api_key_hint": key_hint(&p.api_key),
        })
    });
    let total_messages: usize = sessions.sessions.iter().map(|s| s.messages.len()).sum();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "data_dir": ctx.data_dir.display().to_string(),
        "remote": { "port": cfg.port, "access_password_enabled": cfg.password_enabled },
        "provider": active,
        "reasoning_effort": ai.reasoning_effort,
        "tools": {
            "count": tools.len(),
            "names": tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
        },
        "mcp": {
            "count": mcp.len(),
            "servers": mcp.iter().map(|s| json!({
                "id": s.id, "name": s.name, "url": s.url,
                "enabled": s.enabled, "protocol": s.protocol, "version": s.version,
            })).collect::<Vec<_>>(),
        },
        "sessions": {
            "count": sessions.sessions.len(),
            "active": sessions.active,
            "messages": total_messages,
        },
        "memories": memories,
        "skills": skills,
    }))
    .into_response()
}

/// GET /api/debug/sessions：会话列表（不含消息体，含条数与摘要）
async fn debug_sessions(State(ctx): State<Arc<Ctx>>) -> Response {
    let store = ctx.sessions.lock().unwrap().clone();
    let list: Vec<_> = store
        .sessions
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "title": s.title,
                "created": s.created,
                "updated": s.updated,
                "messages": s.messages.len(),
                "preview": s.preview(),
            })
        })
        .collect();
    Json(json!({ "active": store.active, "sessions": list })).into_response()
}

/// GET /api/debug/sessions/{id}：单个会话全部消息（含 tool_calls 明细）
async fn debug_session_detail(State(ctx): State<Arc<Ctx>>, Path(id): Path<String>) -> Response {
    let store = ctx.sessions.lock().unwrap().clone();
    match store.sessions.iter().find(|s| s.id == id) {
        Some(s) => Json(json!({
            "id": s.id,
            "title": s.title,
            "created": s.created,
            "updated": s.updated,
            "messages": s.messages,
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "session not found" })),
        )
            .into_response(),
    }
}

/// GET /api/debug/goals：目标与待办快照（供 E2E / 调试桥断言目标状态）
async fn debug_goals(State(ctx): State<Arc<Ctx>>) -> Response {
    let goals = ctx.goals.lock().unwrap().clone();
    let todos = ctx.todos.lock().unwrap().clone();
    Json(json!({ "goals": goals, "todos": todos })).into_response()
}

/// GET /api/debug/mcp：MCP 服务器连接状态 + 每台服务器导入的工具名
async fn debug_mcp(State(ctx): State<Arc<Ctx>>) -> Response {
    let mcp = ctx.mcp.lock().unwrap().clone();
    let tools = ctx.tools.lock().unwrap().clone();
    let servers: Vec<_> = mcp
        .iter()
        .map(|s| {
            let imported: Vec<&String> = tools
                .iter()
                .filter_map(|t| match &t.kind {
                    crate::registry::ToolKind::Mcp { server_id, tool }
                        if server_id == &s.id =>
                    {
                        Some(tool)
                    }
                    _ => None,
                })
                .collect();
            json!({
                "id": s.id,
                "name": s.name,
                "url": s.url,
                "enabled": s.enabled,
                "protocol": s.protocol,
                "version": s.version,
                "connected_at": s.connected_at,
                "tools": imported,
            })
        })
        .collect();
    Json(json!({ "servers": servers })).into_response()
}

/// POST /api/debug/interrupt：置位会话中断标志（远程触发「停止对话」，语义同 Tauri 命令
/// chat_interrupt）。仅当该会话有正在执行的回合（映射中存在标志）时生效，
/// 执行循环在下一检查点停止并返回「对话已中断」
async fn debug_interrupt(State(ctx): State<Arc<Ctx>>, Json(body): Json<serde_json::Value>) -> Response {
    let sid = body
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if sid.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "lose session_id" })),
        )
            .into_response();
    }
    let hit = {
        let map = ctx.interrupts.lock().unwrap();
        match map.get(&sid) {
            Some(flag) => {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
                true
            }
            None => false,
        }
    };
    crate::audit::record(&ctx, "remote", "chat.interrupt", &sid, json!({ "was_running": hit }), true);
    Json(json!({ "id": sid, "interrupted": hit })).into_response()
}

/// POST /api/debug/config：运行时调整幻觉防护阈值 / 对话限速 / 审批模式（E2E 熔断用例 / 调试桥），仅接受列出的键，同步落盘
async fn debug_config(State(ctx): State<Arc<Ctx>>, Json(body): Json<serde_json::Value>) -> Response {
    let (w, t) = {
        let mut cfg = ctx.config.lock().unwrap();
        if let Some(v) = body.get("word_repeat_max").and_then(|x| x.as_u64()) {
            cfg.word_repeat_max = (v.min(10000)) as u32;
        }
        if let Some(v) = body.get("tool_loop_max").and_then(|x| x.as_u64()) {
            cfg.tool_loop_max = (v.min(10000)) as u32;
        }
        if let Some(v) = body.get("chat_rpm_max").and_then(|x| x.as_u64()) {
            cfg.chat_rpm_max = (v.min(100000)) as u32;
        }
        if let Some(v) = body.get("relay_kbps_per_user").and_then(|x| x.as_u64()) {
            // 中继每用户响应带宽（KB/s，0=不限）：中继循环按请求动态读取，改完即生效
            cfg.relay_kbps_per_user = (v.min(1000000)) as u32;
        }
        if let Some(v) = body.get("relay_max_text_chars").and_then(|x| x.as_u64()) {
            // 中继文本长度上限（字符数，0=不限）：验签后按请求动态读取，改完即生效
            cfg.relay_max_text_chars = (v.min(10_000_000)) as u32;
        }
        if let Some(v) = body.get("tool_approval").and_then(|x| x.as_str()) {
            // 非法值直接拒绝，避免把配置改成永远无法审批的死状态
            if ["ask", "auto", "allow_all"].contains(&v) {
                cfg.tool_approval = v.to_string();
            }
        }
        if let Some(v) = body.get("cloud_relay_url").and_then(|x| x.as_str()) {
            // 云中继入口（空串=清除）：中继循环每轮动态读取，改完即生效
            cfg.cloud_relay_url = if v.trim().is_empty() { None } else { Some(v.to_string()) };
        }
        if let Some(v) = body.get("stun_servers") {
            // 自定 STUN 列表：数组或逗号/分号分隔字符串（空=清除，恢复内置默认列表）
            let list: Vec<String> = match v {
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect(),
                serde_json::Value::String(s) => s.split([',', ';', '，', '；']).map(String::from).collect(),
                _ => Vec::new(),
            };
            let list: Vec<String> = list
                .iter()
                .flat_map(|s| s.split([',', ';', '，', '；']))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            cfg.stun_servers = if list.is_empty() { None } else { Some(list) };
        }
        // 敏感词审核开关
        if let Some(v) = body.get("moderation_enabled").and_then(|x| x.as_bool()) {
            cfg.moderation_enabled = v;
        }
        // 自定敏感词表：数组或逗号分隔字符串（空=清除，恢复内置默认词库）
        if let Some(v) = body.get("blocked_words") {
            let list: Vec<String> = match v {
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect(),
                serde_json::Value::String(s) => s.split([',', ';', '，', '；']).map(String::from).collect(),
                _ => Vec::new(),
            };
            let list: Vec<String> = list
                .iter()
                .flat_map(|s| s.split([',', ';', '，', '；']))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            cfg.blocked_words = if list.is_empty() { None } else { Some(list) };
        }
        // 每 IP 并发在途上限（0=不限）
        if let Some(v) = body.get("max_active_per_ip").and_then(|x| x.as_u64()) {
            cfg.max_active_per_ip = (v.min(1000)) as u32;
        }
        // IP 黑名单：数组或逗号分隔字符串（空=清空黑名单）
        if let Some(v) = body.get("ip_blocklist") {
            let list: Vec<String> = match v {
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect(),
                serde_json::Value::String(s) => s.split([',', ';', '，', '；']).map(String::from).collect(),
                _ => Vec::new(),
            };
            let list: Vec<String> = list
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            cfg.ip_blocklist = if list.is_empty() { None } else { Some(list) };
        }
        // 信道防护开关：经中继请求是否强制 bitsign 验签
        if let Some(v) = body.get("channel_guard").and_then(|x| x.as_bool()) {
            cfg.channel_guard = v;
        }
        (cfg.word_repeat_max, cfg.tool_loop_max)
    };
    // E2E 钩子：运行时切换激活提供方（多协议原生工具调用桥接测试用）。
    // 与 set_provider_active 同互斥语义；写 ai_config（探测缓存不动，目标提供方首次对话重新探测）
    if let Some(v) = body.get("active_provider").and_then(|x| x.as_str()) {
        {
            let mut ai = ctx.ai_config.lock().unwrap();
            if ai.providers.iter().any(|p| p.id == v) {
                for p in ai.providers.iter_mut() {
                    p.active = p.id == v;
                }
                // 必须先 drop MutexGuard 再 save——std::sync::Mutex 不可重入，
                // save_ai_config 内部会再 lock 同一把，重入会直接死锁
            }
        }
        ctx.save_ai_config();
    }
    // 可选：清空限速窗口（E2E 用例隔离，避免上一用例的计数影响下一个）
    if body.get("chat_rate_reset").and_then(|x| x.as_bool()) == Some(true) {
        ctx.chat_rate.lock().unwrap().clear();
    }
    ctx.save_config();
    Json(json!({ "word_repeat_max": w, "tool_loop_max": t })).into_response()
}

/// 远程对话限速：60 秒滑动窗口按客户端 IP 计数（/api/chat 与 /v1/chat/completions 各自独立桶）。
/// 返回 Some(建议等待秒数) 表示超限；chat_rpm_max=0 时不限速。
fn chat_rate_check(ctx: &Arc<Ctx>, client: &str, kind: &str) -> Option<u64> {
    let max = ctx.config.lock().unwrap().chat_rpm_max;
    if max == 0 {
        return None;
    }
    let now = std::time::Instant::now();
    let key = format!("{kind}|{client}");
    let mut map = ctx.chat_rate.lock().unwrap();
    let q = map.entry(key).or_default();
    while let Some(front) = q.front() {
        if now.duration_since(*front).as_secs() >= 60 {
            q.pop_front();
        } else {
            break;
        }
    }
    if q.len() >= max as usize {
        let wait = 60 - now.duration_since(*q.front().unwrap()).as_secs();
        return Some(wait.max(1));
    }
    q.push_back(now);
    None
}

/// 限速 429 响应：英文提示 + Retry-After（外部客户端 / 旧模型可读）
fn rate_limited_response(wait: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("Retry-After", wait.to_string().as_str())],
        Json(json!({ "error": "rate limited: too many chat requests, slow down and retry later" })),
    )
        .into_response()
}

/// 每 IP 并发在途请求守卫：acquire 成功后计数 +1，Drop 时递减（含 panic/提前返回路径）。
/// 超上限返回 None → 调用方回 429。流式响应场景守卫在 handler 返回时释放（非流结束），
/// 是保守近似——占位计时短于真实流时长，不影响防护语义
struct ActiveGuard {
    ctx: Arc<Ctx>,
    ip: String,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let mut m = self.ctx.active_per_ip.lock().unwrap();
        if let Some(c) = m.get_mut(&self.ip) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                m.remove(&self.ip);
            }
        }
    }
}

fn active_acquire(ctx: &Arc<Ctx>, ip: &str, max: u32) -> Option<ActiveGuard> {
    {
        let mut m = ctx.active_per_ip.lock().unwrap();
        let c = m.entry(ip.to_string()).or_insert(0);
        if max != 0 && *c >= max {
            return None;
        }
        *c += 1;
    }
    Some(ActiveGuard { ctx: ctx.clone(), ip: ip.to_string() })
}

/// 并发超限 429 响应
fn too_active_response() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("Retry-After", "5")],
        Json(json!({ "error": "too many concurrent requests from this ip, wait and retry" })),
    )
        .into_response()
}

/// 敏感词审核（输入方向）：启用时命中即拒绝。返回英文 403（不透露命中词，防词库探测）
fn moderation_input_check(ctx: &Arc<Ctx>, text: &str, actor: &str, path: &str) -> Option<Response> {
    let cfg = ctx.config.lock().unwrap().clone();
    if !cfg.moderation_enabled {
        return None;
    }
    let words: Vec<String> = cfg
        .blocked_words
        .clone()
        .filter(|l| !l.is_empty())
        .map(|l| l.clone())
        .unwrap_or_else(|| crate::security::DEFAULT_BLOCKED_WORDS.iter().map(|s| s.to_string()).collect());
    if let Some(hit) = crate::security::moderation_scan(text, &words) {
        crate::audit::record(ctx, actor, "chat.moderation_input", path, json!({ "hit": hit.word, "len": text.len() }), false);
        return Some((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": { "message": "moderation blocked: content violates usage policy", "type": "content_policy_violation" } })),
        )
            .into_response());
    }
    None
}

/// 敏感词审核（输出方向）：回复命中时替换为固定拒绝语并审计（不中断会话）
fn moderation_output_check(ctx: &Arc<Ctx>, reply: &str, actor: &str, path: &str) -> String {
    let cfg = ctx.config.lock().unwrap().clone();
    if !cfg.moderation_enabled {
        return reply.to_string();
    }
    let words: Vec<String> = cfg
        .blocked_words
        .clone()
        .filter(|l| !l.is_empty())
        .map(|l| l.clone())
        .unwrap_or_else(|| crate::security::DEFAULT_BLOCKED_WORDS.iter().map(|s| s.to_string()).collect());
    if let Some(hit) = crate::security::moderation_scan(reply, &words) {
        crate::audit::record(ctx, actor, "chat.moderation_output", path, json!({ "hit": hit.word, "len": reply.len() }), false);
        return "The response was withheld by moderation (content policy).".to_string();
    }
    reply.to_string()
}

/// GET /api/context/metrics：当前会话上下文用量 + 模型最大上下文（手机端 / E2E 用）
async fn context_metrics_route(State(ctx): State<Arc<Ctx>>) -> Response {
    let session_id = ctx.sessions.lock().unwrap().active.clone();
    let convo = match crate::agent::build_context(&ctx, &session_id) {
        Ok((c, _)) => c,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    let est = super::commands::estimate_context_tokens(&ctx, &session_id, &convo);
    Json(json!({
        "session_id": session_id,
        "est_tokens": est,
        "max_context": super::commands::active_max_context(&ctx),
    }))
    .into_response()
}

/// 远程对话：Agent 通过 HTTP 使用 BIT 的 AI 能力（含自写插件）
async fn remote_chat(
    State(ctx): State<Arc<Ctx>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    // 对话请求限速：按客户端 IP 滑动窗口（chat_rpm_max，0=不限）。
    // 经验签中继请求按真实手机端 IP 计（见 chat_client_ip）
    let client_ip = chat_client_ip(&ctx, &headers, &addr.ip());
    if let Some(wait) = chat_rate_check(&ctx, &client_ip, "api") {
        return rate_limited_response(wait);
    }
    let actor = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| actor_of(v.strip_prefix("Bearer ").unwrap_or("")))
        .unwrap_or_else(|| "agent:unknown".into());
    // 每 IP 并发在途上限（max_active_per_ip，0=不限）：防单 IP 洪泛占满对话通道
    let max_active = ctx.config.lock().unwrap().max_active_per_ip;
    let _active = match active_acquire(&ctx, &client_ip, max_active) {
        Some(g) => g,
        None => return too_active_response(),
    };

    let message = body.get("message").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    // 中继文本长度上限（relay_max_text_chars，0=不限）：只对经中继进入的请求生效，
    // LAN 直连不受限。防止把中继当大文本传输通道（正常对话通常 < 10K 字符）
    if headers.get("x-bit-via").and_then(|v| v.to_str().ok()) == Some("relay") {
        let cap = ctx.config.lock().unwrap().relay_max_text_chars as usize;
        if cap > 0 && message.chars().count() > cap {
            crate::audit::record(&ctx, &actor, "http.relay_text_too_long", "/api/chat", json!({ "chars": message.chars().count(), "cap": cap }), false);
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({ "error": format!("relay text limit: message exceeds {cap} chars") })),
            )
                .into_response();
        }
    }
    // 可选图片：data URL（data:image/png;base64,...）数组，仅支持视觉的模型能看到
    let images: Vec<String> = body
        .get("images")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    // 远程必须指定会话：每台设备用独立会话（QR sidPolicy=device，手机端自建 remote-<随机>）。
    // 绝不容许空 session_id 落入桌面激活会话——否则多设备/多人共用一条会话必然串线
    let session_id = body.get("session_id").and_then(|v| v.as_str()).unwrap_or_default().trim().to_string();
    if message.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "缺少 message 字段" })),
        )
            .into_response();
    }
    if session_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "缺少 session_id 字段：远程对话必须指定会话（每台设备自建独立会话，避免与桌面或其它设备串线）" })),
        )
            .into_response();
    }
    // 敏感词审核（输入）：启用时命中即 403（含经中继与 LAN 直连的全部远程入口）
    if let Some(resp) = moderation_input_check(&ctx, &message, &actor, "/api/chat") {
        return resp;
    }

    // 远程指定的会话不存在时自动创建（外部客户端可直接开启新会话）
    ctx.sessions.lock().unwrap().get_or_create_mut(&session_id);

    match crate::agent::chat_turn_auto(&ctx, &session_id, &message, images).await {
        Ok(messages) => {
            let last = messages
                .iter()
                .rev()
                .find(|m| m.role == "assistant")
                .map(|m| m.content.clone())
                .unwrap_or_default();
            // 敏感词审核（输出）：回复命中时以外发替换语返回（会话记录不动）
            let last = moderation_output_check(&ctx, &last, &actor, "/api/chat");
            crate::audit::record(&ctx, &actor, "chat.remote", "ai", json!({ "reply_len": last.len() }), true);
            Json(json!({ "reply": last, "messages": messages })).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e })),
        )
            .into_response(),
    }
}

// ==================== OpenAI 兼容端点 ====================
// 第三方 OpenAI 格式客户端（Cherry Studio / LobeChat / 沉浸式翻译等）可直接接入：
// Base URL = http://<host>:<port>/v1，API Key = BIT 的 Client Key

#[derive(serde::Deserialize)]
struct OaiRequest {
    // 接收但忽略：实际路由始终由 BIT 激活的 Provider 决定
    #[allow(dead_code)]
    #[serde(default)]
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(default)]
    stream: bool,
}

#[derive(serde::Deserialize)]
struct OaiMessage {
    role: String,
    /// OpenAI 格式：字符串 或 多模态数组 [{type:"text",...},{type:"image_url",...}]
    #[serde(default)]
    content: serde_json::Value,
}

/// OpenAI content 的文本字符数统计：字符串直计；多模态数组只累加 type:"text" 部分
/// （image_url 是 base64 数据，不占文本预算——长度上限防的是文本搬运）
fn oai_text_len(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::String(s) => s.chars().count(),
        serde_json::Value::Array(a) => a
            .iter()
            .map(|p| p.get("text").and_then(|t| t.as_str()).map(|s| s.chars().count()).unwrap_or(0))
            .sum(),
        _ => 0,
    }
}

/// OpenAI messages → (BIT ChatMessage 列表, 图片列表)
fn convert_oai_messages(msgs: &[OaiMessage]) -> (Vec<crate::ai::ChatMessage>, Vec<String>) {
    use crate::ai::ChatMessage;
    let mut out = Vec::new();
    let mut images = Vec::new();
    for m in msgs {
        let mut text = String::new();
        match &m.content {
            serde_json::Value::String(s) => text = s.clone(),
            serde_json::Value::Array(parts) => {
                for p in parts {
                    match p.get("type").and_then(|v| v.as_str()) {
                        Some("text") => {
                            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(t);
                            }
                        }
                        Some("image_url") => {
                            if let Some(u) = p
                                .get("image_url")
                                .and_then(|iu| iu.get("url"))
                                .and_then(|v| v.as_str())
                            {
                                images.push(u.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        out.push(match m.role.as_str() {
            "system" | "developer" => ChatMessage::system(&text),
            "assistant" => ChatMessage::assistant(&text),
            _ => ChatMessage::user(&text),
        });
    }
    (out, images)
}

/// GET /v1/models：返回激活 Provider 的模型，供客户端校验
async fn openai_models(State(ctx): State<Arc<Ctx>>) -> Response {
    let model = {
        let cfg = ctx.ai_config.lock().unwrap();
        cfg.active().map(|p| p.model.clone()).unwrap_or_else(|| "bit".into())
    };
    Json(json!({
        "object": "list",
        "data": [
            { "id": model, "object": "model", "created": 0, "owned_by": "bit" },
            { "id": "bit", "object": "model", "created": 0, "owned_by": "bit" }
        ]
    }))
    .into_response()
}

/// POST /v1/chat/completions：OpenAI 格式对话（支持 stream SSE 与非流式）
/// 直接透传给激活的 AI Provider，不进入 Agent 工具循环（客户端发来的是完整对话历史）
async fn openai_chat_completions(
    State(ctx): State<Arc<Ctx>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<OaiRequest>,
) -> Response {
    // 对话请求限速：按客户端 IP 滑动窗口（与 /api/chat 同一上限、独立计数桶）。
    // 经验签中继请求按真实手机端 IP 计（见 chat_client_ip）
    let client_ip = chat_client_ip(&ctx, &headers, &addr.ip());
    if let Some(wait) = chat_rate_check(&ctx, &client_ip, "v1") {
        return rate_limited_response(wait);
    }
    // 每 IP 并发在途上限（与 /api/chat 同一上限、独立计数同桶按 IP）
    let max_active = ctx.config.lock().unwrap().max_active_per_ip;
    let _active = match active_acquire(&ctx, &client_ip, max_active) {
        Some(g) => g,
        None => return too_active_response(),
    };
    let actor = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| actor_of(v.strip_prefix("Bearer ").unwrap_or("")))
        .unwrap_or_else(|| "agent:unknown".into());

    let (messages, images) = convert_oai_messages(&req.messages);
    if messages.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": { "message": "messages 不能为空", "type": "invalid_request_error" } })),
        )
            .into_response();
    }
    // 中继文本长度上限（relay_max_text_chars，0=不限）：OpenAI 客户端整包发送历史，
    // 上限按全部消息文本总字符数计（只算文本，图片不占预算）。仅限中继面，
    // LAN 直连不受限——防把中继当大文本传输通道
    if headers.get("x-bit-via").and_then(|v| v.to_str().ok()) == Some("relay") {
        let cap = ctx.config.lock().unwrap().relay_max_text_chars as usize;
        let chars: usize = req.messages.iter().map(|m| oai_text_len(&m.content)).sum();
        if cap > 0 && chars > cap {
            crate::audit::record(&ctx, &actor, "http.relay_text_too_long", "/v1/chat/completions", json!({ "chars": chars, "cap": cap }), false);
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({ "error": { "message": format!("relay text limit: total message text exceeds {cap} chars"), "type": "invalid_request_error" } })),
            )
                .into_response();
        }
    }
    // 敏感词审核（输入）：扫全部消息文本（OpenAI 客户端把历史整包发来，逐条都要过闸）
    {
        let joined: String = messages.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
        if let Some(resp) = moderation_input_check(&ctx, &joined, &actor, "/v1/chat/completions") {
            return resp;
        }
    }

    let model = {
        let cfg = ctx.ai_config.lock().unwrap();
        cfg.active().map(|p| p.model.clone()).unwrap_or_else(|| "bit".into())
    };
    let created = chrono::Utc::now().timestamp();
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());

    if req.stream {
        // 流式：SSE，逐 token 输出 OpenAI chunk 格式
        let ctx2 = ctx.clone();
        let actor2 = actor.clone();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, std::convert::Infallible>>();
        tauri::async_runtime::spawn(async move {
            let send_chunk = |delta: serde_json::Value, finish: Option<&str>| {
                let _ = tx.send(Ok(Event::default().data(
                    json!({
                        "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }]
                    })
                    .to_string(),
                )));
            };
            // 首个 chunk 带 role
            send_chunk(json!({ "role": "assistant" }), None);

            let result = crate::ai::chat_stream_with_images(&ctx2, &messages, &images, |kind, tok| {
                // 思考过程按 OpenAI 兼容约定转发为 reasoning_content 增量，正文为 content
                match kind {
                    crate::ai::TokenKind::Think => send_chunk(json!({ "reasoning_content": tok }), None),
                    crate::ai::TokenKind::Text => send_chunk(json!({ "content": tok }), None),
                }
                true
            })
            .await;

            match result {
                Ok((_, usage)) => {
                    send_chunk(json!({}), Some("stop"));
                    // usage chunk（OpenAI 规范：choices 为空数组，客户端据此统计 token）
                    let _ = tx.send(Ok(Event::default().data(
                        json!({
                            "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                            "choices": [],
                            "usage": {
                                "prompt_tokens": usage.prompt_tokens,
                                "completion_tokens": usage.completion_tokens,
                                "total_tokens": usage.prompt_tokens + usage.completion_tokens,
                                "prompt_tokens_details": { "cached_tokens": usage.cache_read_tokens }
                            }
                        })
                        .to_string(),
                    )));
                    let _ = tx.send(Ok(Event::default().data("[DONE]")));
                    crate::audit::record(&ctx2, &actor2, "chat.openai", "/v1/chat/completions", json!({ "stream": true, "ok": true }), true);
                }
                Err(e) => {
                    // SSE 中途出错：以错误 chunk 收尾
                    let _ = tx.send(Ok(Event::default().data(
                        json!({ "error": { "message": e, "type": "server_error" } }).to_string(),
                    )));
                    let _ = tx.send(Ok(Event::default().data("[DONE]")));
                    crate::audit::record(&ctx2, &actor2, "chat.openai", "/v1/chat/completions", json!({ "stream": true, "ok": false }), false);
                }
            }
        });

        let stream = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response()
    } else {
        // 非流式：整体返回 OpenAI completion 格式
        match crate::ai::chat_with_images(&ctx, &messages, &images).await {
            Ok((reply, usage)) => {
                // 敏感词审核（输出）：仅非流式可整体拦（流式 token 已外发，靠输入闸兜底）
                let reply = moderation_output_check(&ctx, &reply, &actor, "/v1/chat/completions");
                crate::audit::record(&ctx, &actor, "chat.openai", "/v1/chat/completions", json!({ "stream": false, "ok": true, "reply_len": reply.len() }), true);
                Json(json!({
                    "id": id, "object": "chat.completion", "created": created, "model": model,
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": reply },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": usage.prompt_tokens,
                        "completion_tokens": usage.completion_tokens,
                        "total_tokens": usage.prompt_tokens + usage.completion_tokens,
                        "prompt_tokens_details": { "cached_tokens": usage.cache_read_tokens }
                    }
                }))
                .into_response()
            }
            Err(e) => {
                crate::audit::record(&ctx, &actor, "chat.openai", "/v1/chat/completions", json!({ "stream": false, "ok": false }), false);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": { "message": e, "type": "server_error" } })),
                )
                    .into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn cfg_with_key(key: &str, password: Option<&str>) -> Config {
        let mut cfg = Config::default();
        cfg.client_key = key.to_string();
        cfg.access_password = password.map(String::from);
        cfg
    }

    // ---------- 安全：鉴权边界测试 ----------

    #[test]
    fn test_key_hint_masks_secrets() {
        // 调试接口只允许泄露前 6 位 + 长度，绝不出现完整密钥
        assert_eq!(key_hint("bit_0123456789abcdef"), "bit_01…(20)");
        assert_eq!(key_hint("short!"), "***");
        assert_eq!(key_hint(""), "***");
        // 非 ASCII 密钥不得 panic（char boundary 安全）
        assert_eq!(key_hint("密钥测试一"), "***");
    }

    #[test]
    fn test_auth_health_bypass() {
        // /api/health 免鉴权：无任何凭据也放行
        let cfg = cfg_with_key("sk-bit-test", Some("12345678"));
        assert!(check_auth(&cfg, "/api/health", "", "", "").is_ok());
    }

    #[test]
    fn test_ip_prefix_matches_worker_semantics() {
        // 口径与 relay-worker/fake_relay 的 ipPrefix 逐字符一致（来源一致性闸门的比对基准，
        // 实现差异会误杀正常请求）：v4 /24、v6 /48、回环原样、空 unknown、畸形原样
        assert_eq!(ip_prefix("203.0.113.7"), "203.0.113"); // v4 → 前 3 段
        assert_eq!(ip_prefix("203.0.113.200"), "203.0.113"); // 同段不同主机 → 同段
        assert_eq!(ip_prefix("127.0.0.1"), "127.0.0.1"); // v4 回环原样
        assert_eq!(ip_prefix("::1"), "::1"); // v6 回环原样
        assert_eq!(ip_prefix("2408:8207:18cc:a9e0::1"), "2408:8207:18cc:a9e0"); // v6 → 前 4 组
        assert_eq!(ip_prefix("2001:db8:85a3:1:2:3:4:5"), "2001:db8:85a3:1");
        assert_eq!(ip_prefix(""), "unknown"); // 空
        assert_eq!(ip_prefix("not-an-ip"), "not-an-ip"); // 畸形原样（不误杀非标来源）
    }

    #[test]
    fn test_auth_debug_endpoints_require_dual_auth() {
        // 调试接口（ADB 联动）与其它 /api/* 同级保护：Client Key + 访问密码缺一不可
        let cfg = cfg_with_key("sk-bit-test", Some("12345678"));
        let paths = [
            "/api/debug/state",
            "/api/debug/sessions",
            "/api/debug/sessions/s1",
            "/api/debug/mcp",
        ];
        for path in paths {
            assert!(
                check_auth(&cfg, path, "sk-bit-test", "", "12345678").is_ok(),
                "{path} 双凭据应放行"
            );
            // 缺 Client Key → 401
            assert_eq!(
                check_auth(&cfg, path, "", "", "12345678"),
                Err(StatusCode::UNAUTHORIZED),
                "{path} 缺 Client Key 应 401"
            );
            // 缺访问密码 → 401（调试端点不适用 /v1/ 豁免）
            assert_eq!(
                check_auth(&cfg, path, "sk-bit-test", "", ""),
                Err(StatusCode::UNAUTHORIZED),
                "{path} 缺访问密码应 401"
            );
        }
        // ?key= 查询参数通道同样适用于调试端点
        assert!(check_auth(&cfg, "/api/debug/state", "", "sk-bit-test", "12345678").is_ok());
        assert!(check_auth(&cfg, "/api/debug/state", "", "", "12345678").is_err());
    }

    #[test]
    fn test_auth_no_client_key_configured_rejects_all() {
        // 安全关键：Client Key 为空时即使请求也不带 key 也必须拒绝，杜绝空 key 绕过
        let cfg = cfg_with_key("", Some("12345678"));
        assert_eq!(
            check_auth(&cfg, "/api/tools", "", "", "12345678"),
            Err(StatusCode::SERVICE_UNAVAILABLE)
        );
        assert_eq!(check_auth(&cfg, "/v1/models", "", "", ""), Err(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(check_auth(&cfg, "/mcp", "", "", ""), Err(StatusCode::SERVICE_UNAVAILABLE));
    }

    #[test]
    fn test_auth_bearer_key_accept_and_reject() {
        let cfg = cfg_with_key("sk-bit-right", Some("12345678"));
        // 正确 Bearer + 密码 → 放行
        assert!(check_auth(&cfg, "/api/tools", "sk-bit-right", "", "12345678").is_ok());
        // 错误 Bearer → 401
        assert_eq!(
            check_auth(&cfg, "/api/tools", "sk-bit-wrong", "", "12345678"),
            Err(StatusCode::UNAUTHORIZED)
        );
        // 缺失 Bearer → 401
        assert_eq!(
            check_auth(&cfg, "/api/tools", "", "", "12345678"),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn test_auth_query_key_equivalent() {
        // 等价类：?key= 与 Bearer 应等价
        let cfg = cfg_with_key("sk-bit-right", Some("12345678"));
        assert!(check_auth(&cfg, "/api/tools", "", "sk-bit-right", "12345678").is_ok());
        assert_eq!(
            check_auth(&cfg, "/api/tools", "", "sk-bit-wrong", "12345678"),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn test_auth_access_password_enforced_only_on_api() {
        let cfg = cfg_with_key("sk-bit-right", Some("12345678"));
        // /api/ 需要访问密码
        assert_eq!(
            check_auth(&cfg, "/api/chat", "sk-bit-right", "", ""),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            check_auth(&cfg, "/api/chat", "sk-bit-right", "", "wrong-pwd"),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert!(check_auth(&cfg, "/api/chat", "sk-bit-right", "", "12345678").is_ok());
        // 等价类：/v1/ 与 /mcp 豁免访问密码（OpenAI 客户端无法携带自定义头）
        assert!(check_auth(&cfg, "/v1/chat/completions", "sk-bit-right", "", "").is_ok());
        assert!(check_auth(&cfg, "/mcp", "sk-bit-right", "", "").is_ok());
    }

    #[test]
    fn test_auth_password_disabled_passes() {
        // 未启用密码校验（password_enabled=false）时直接通过
        let mut cfg = cfg_with_key("sk-bit-right", None);
        cfg.password_enabled = false;
        assert!(check_auth(&cfg, "/api/chat", "sk-bit-right", "", "").is_ok());
        // 启用但 access_password 为 None：一律拒绝（无法匹配）
        let mut cfg2 = cfg_with_key("sk-bit-right", None);
        cfg2.password_enabled = true;
        assert_eq!(
            check_auth(&cfg2, "/api/chat", "sk-bit-right", "", ""),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    // ---------- 边缘：actor 标识 ----------

    #[test]
    fn test_actor_of_edges() {
        assert_eq!(actor_of(""), "agent:unknown");
        assert_eq!(actor_of("ab"), "agent:unknown"); // skip(4) 后为空 → unknown
        assert_eq!(actor_of("sk-bit-12345678xyz"), "agent:it-12345"); // skip 4 位取 8 位
    }

    // ---------- 错误路径：模型列表拉取 ----------

    /// 起一个只回 401 JSON 的本地 TCP 服务
    async fn spawn_401_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => return,
                };
                tokio::spawn(async move {
                    let body = r#"{"error":{"message":"Incorrect API key"}}"#;
                    let resp = format!(
                        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    use tokio::io::AsyncWriteExt;
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}/v1")
    }

    #[tokio::test]
    async fn test_list_models_http_error_propagates() {
        // 上游 401：错误信息应透传而不是静默返回空列表
        let base = spawn_401_server().await;
        let err = super::super::commands::fetch_provider_models("openai", &base, "bad-key")
            .await
            .unwrap_err();
        assert!(err.contains("401"), "应透传 HTTP 状态码: {err}");
    }

    #[tokio::test]
    async fn test_list_models_connection_refused() {
        // 连接不存在的端口：应返回 Err 而非 panic/空列表
        let err = super::super::commands::fetch_provider_models("openai", "http://127.0.0.1:9/v1", "")
            .await
            .unwrap_err();
        assert!(!err.is_empty());
    }

    #[tokio::test]
    async fn test_list_models_empty_base_url() {
        let err = super::super::commands::fetch_provider_models("openai", "  ", "")
            .await
            .unwrap_err();
        assert!(err.contains("Base URL"));
    }

    #[tokio::test]
    async fn test_list_models_trailing_slash_normalized() {
        // 边缘：base_url 带尾斜杠不应产生 //models 双斜杠（对 401 服务请求即可验证 URL 拼接正常）
        let base = spawn_401_server().await;
        let err = super::super::commands::fetch_provider_models("openai", &format!("{base}/"), "bad-key")
            .await
            .unwrap_err();
        assert!(err.contains("401"), "尾斜杠应被归一化: {err}");
    }

    // ── MCP 服务器（Streamable HTTP）协议符合性 ──

    #[test]
    fn test_mcp_negotiate_version_echoes_supported() {
        // 客户端请求受支持版本 → 必须回显同版本（规范要求）
        assert_eq!(super::mcp_negotiate_version("2025-06-18"), "2025-06-18");
        assert_eq!(super::mcp_negotiate_version("2025-03-26"), "2025-03-26");
        assert_eq!(super::mcp_negotiate_version("2024-11-05"), "2024-11-05");
    }

    #[test]
    fn test_mcp_negotiate_version_falls_back_to_latest() {
        // 客户端请求未知/缺失版本 → 返回服务器最新支持版
        assert_eq!(super::mcp_negotiate_version("1999-01-01"), "2025-06-18");
        assert_eq!(super::mcp_negotiate_version(""), "2025-06-18");
    }

    #[test]
    fn test_mcp_session_id_unique_and_shaped() {
        let a = super::gen_mcp_session_id();
        let b = super::gen_mcp_session_id();
        assert!(a.starts_with("mcp-"), "会话 id 应有固定前缀: {a}");
        assert_ne!(a, b, "连续生成的会话 id 必须唯一");
    }

    #[test]
    fn test_mcp_check_session_missing_sid_is_400() {
        // 会话建立后不携带 Mcp-Session-Id → 400（规范）
        let mut map = std::collections::HashMap::new();
        map.insert("mcp-1".to_string(), std::time::Instant::now());
        assert_eq!(
            super::mcp_check_session_map(&mut map, None, super::MCP_SESSION_TTL),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn test_mcp_check_session_unknown_is_404() {
        let mut map = std::collections::HashMap::new();
        map.insert("mcp-real".to_string(), std::time::Instant::now());
        assert_eq!(
            super::mcp_check_session_map(&mut map, Some("mcp-fake"), super::MCP_SESSION_TTL),
            Err(StatusCode::NOT_FOUND)
        );
    }

    #[test]
    fn test_mcp_check_session_valid_refreshes() {
        let mut map = std::collections::HashMap::new();
        map.insert("mcp-1".to_string(), std::time::Instant::now());
        assert!(super::mcp_check_session_map(&mut map, Some("mcp-1"), super::MCP_SESSION_TTL).is_ok());
        assert!(map.contains_key("mcp-1"), "有效会话不应被清除");
    }

    #[test]
    fn test_mcp_check_session_expired_pruned() {
        // TTL=0：所有现存会话立即过期 → 懒清理后视为未知 → 404
        let mut map = std::collections::HashMap::new();
        map.insert("mcp-old".to_string(), std::time::Instant::now());
        assert_eq!(
            super::mcp_check_session_map(&mut map, Some("mcp-old"), std::time::Duration::ZERO),
            Err(StatusCode::NOT_FOUND)
        );
        assert!(map.is_empty(), "过期会话应被懒清理");
    }
}
