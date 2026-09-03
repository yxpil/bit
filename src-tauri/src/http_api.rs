use crate::state::Ctx;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;
use std::sync::Arc;

/// 启动/重启远程访问 HTTP 服务
pub async fn restart_server(ctx: &Arc<Ctx>) -> Result<String, String> {
    // 停止旧服务（先取出 JoinHandle，释放锁后再 await）
    let old_task = ctx.server_task.lock().unwrap().take();
    if let Some(task) = old_task {
        task.abort();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    let cfg = ctx.config.lock().unwrap().clone();
    if !cfg.remote_enabled {
        return Ok("disabled".into());
    }

    let addr = cfg.listen_addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("绑定 {addr} 失败: {e}"))?;

    let router = build_router(ctx.clone());
    let task = tauri::async_runtime::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("[BIT] http server stopped: {e}");
        }
    });
    *ctx.server_task.lock().unwrap() = Some(task);
    Ok(addr)
}

pub fn build_router(ctx: Arc<Ctx>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/tools", get(list_tools).post(register_tool))
        .route("/api/tools/{id}", axum::routing::delete(remove_tool))
        .route("/api/tools/{id}/invoke", post(invoke_tool))
        .route("/api/chat", post(remote_chat))
        .route("/api/audit", get(list_audit))
        // 自动更新：检测 / 状态 / 手动下载（启动后台任务已自动下，此处供远程管理用）
        .route("/api/update/check", get(update_check))
        .route("/api/update/status", get(update_status))
        .route("/api/update/download", post(update_download_route))
        // 调试接口（ADB 调试桥联动）：只读快照，供外部调试工具分析与控制 BIT
        .route("/api/debug/state", get(debug_state))
        .route("/api/debug/sessions", get(debug_sessions))
        .route("/api/debug/sessions/{id}", get(debug_session_detail))
        .route("/api/debug/mcp", get(debug_mcp))
        .route("/mcp", post(mcp_endpoint))
        // OpenAI 兼容端点：第三方 OpenAI 格式客户端可直接接入（API Key 填 Client Key）
        .route("/v1/models", get(openai_models))
        .route("/v1/chat/completions", post(openai_chat_completions))
        .layer(middleware::from_fn_with_state(ctx.clone(), auth))
        .with_state(ctx)
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

/// BIT 自身作为 MCP 服务器（Streamable HTTP / JSON-RPC 2.0）：
/// 任何 MCP 客户端（Claude Desktop、Cherry Studio、BIT 自己的自动发现）都可接入 BIT 的全部启用工具。
/// 认证：Bearer Client Key 或 ?key=（见 auth 中间件）。
async fn mcp_endpoint(
    State(ctx): State<Arc<Ctx>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let method = body.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let rpc_ok = |result: serde_json::Value| {
        json!({ "jsonrpc": "2.0", "id": id, "result": result })
    };
    let rpc_err = |code: i64, msg: &str| {
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } })
    };

    match method {
        "initialize" => Json(rpc_ok(json!({
            "protocolVersion": "2025-03-26",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "BIT", "version": env!("CARGO_PKG_VERSION") }
        })))
        .into_response(),
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/list" => {
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
            Json(rpc_ok(json!({ "tools": list }))).into_response()
        }
        "tools/call" => {
            let name = body
                .pointer("/params/name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args = body
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
                None => Json(rpc_err(-32602, "tool not found")).into_response(),
                Some((_, false)) => Json(rpc_err(-32000, "tool is paused")).into_response(),
                Some((tid, _)) => match crate::registry::invoke(&ctx, &tid, args, "mcp-client", None).await {
                    Ok(v) => Json(rpc_ok(json!({
                        "content": [{ "type": "text", "text": v.to_string() }],
                        "isError": false
                    })))
                    .into_response(),
                    Err(e) => Json(rpc_ok(json!({
                        "content": [{ "type": "text", "text": e }],
                        "isError": true
                    })))
                    .into_response(),
                },
            }
        }
        "" => (StatusCode::BAD_REQUEST, Json(rpc_err(-32600, "missing method"))).into_response(),
        _ => Json(rpc_err(-32601, "method not found")).into_response(),
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
    if bearer != cfg.client_key && qkey != cfg.client_key {
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
            let reason = if provided != cfg.client_key && qkey != cfg.client_key {
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
    next.run(req).await
}

fn actor_of(key: &str) -> String {
    let prefix: String = key.chars().skip(4).take(8).collect();
    if prefix.is_empty() {
        "agent:unknown".to_string()
    } else {
        format!("agent:{prefix}")
    }
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
    match crate::registry::invoke(&ctx, &id, params, &actor, None).await {
        Ok(result) => Json(json!({ "result": result })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e })),
        )
            .into_response(),
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

/// 远程对话：Agent 通过 HTTP 使用 BIT 的 AI 能力（含自写插件）
async fn remote_chat(
    State(ctx): State<Arc<Ctx>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let actor = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| actor_of(v.strip_prefix("Bearer ").unwrap_or("")))
        .unwrap_or_else(|| "agent:unknown".into());

    let message = body.get("message").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    // 可选图片：data URL（data:image/png;base64,...）数组，仅支持视觉的模型能看到
    let images: Vec<String> = body
        .get("images")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    // 远程可指定会话；未指定则写入当前激活会话
    let session_id = body.get("session_id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if message.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "缺少 message 字段" })),
        )
            .into_response();
    }

    // 远程指定的会话不存在时自动创建（外部客户端可直接开启新会话）
    if !session_id.is_empty() {
        ctx.sessions.lock().unwrap().get_or_create_mut(&session_id);
    }

    match crate::agent::chat_turn(&ctx, &session_id, &message, images).await {
        Ok(messages) => {
            let last = messages
                .iter()
                .rev()
                .find(|m| m.role == "assistant")
                .map(|m| m.content.clone())
                .unwrap_or_default();
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
    headers: HeaderMap,
    Json(req): Json<OaiRequest>,
) -> Response {
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

            let result = crate::ai::chat_stream_with_images(&ctx2, &messages, &images, |tok| {
                send_chunk(json!({ "content": tok }), None);
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
        let err = super::super::commands::list_provider_models(
            "openai".into(),
            base,
            "bad-key".into(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("401"), "应透传 HTTP 状态码: {err}");
    }

    #[tokio::test]
    async fn test_list_models_connection_refused() {
        // 连接不存在的端口：应返回 Err 而非 panic/空列表
        let err = super::super::commands::list_provider_models(
            "openai".into(),
            "http://127.0.0.1:9/v1".into(),
            String::new(),
        )
        .await
        .unwrap_err();
        assert!(!err.is_empty());
    }

    #[tokio::test]
    async fn test_list_models_empty_base_url() {
        let err = super::super::commands::list_provider_models("openai".into(), "  ".into(), String::new())
            .await
            .unwrap_err();
        assert!(err.contains("Base URL"));
    }

    #[tokio::test]
    async fn test_list_models_trailing_slash_normalized() {
        // 边缘：base_url 带尾斜杠不应产生 //models 双斜杠（对 401 服务请求即可验证 URL 拼接正常）
        let base = spawn_401_server().await;
        let err = super::super::commands::list_provider_models(
            "openai".into(),
            format!("{base}/"),
            "bad-key".into(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("401"), "尾斜杠应被归一化: {err}");
    }
}
