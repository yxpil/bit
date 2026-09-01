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

/// 双重认证：Bearer Client Key + X-Access-Password 访问密码，含 HTTP 请求审计。
/// OpenAI 兼容端点（/v1/）例外：OpenAI 客户端无法携带自定义头，仅校验 Client Key。
async fn auth(State(ctx): State<Arc<Ctx>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if path == "/api/health" {
        return next.run(req).await;
    }
    let openai_endpoint = path.starts_with("/v1/");
    // MCP 端点：仅校验 Client Key（Bearer 或 ?key= 查询参数），与 /v1 同级
    let mcp_endpoint = path == "/mcp";

    let cfg = ctx.config.lock().unwrap().clone();
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

    // Client Key 为空 = 未配置密钥，一律拒绝（否则空 key 请求会绕过鉴权）
    if cfg.client_key.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "BIT 尚未配置 Client Key，远程访问已禁用" })),
        )
            .into_response();
    }
    if provided != cfg.client_key && qkey != cfg.client_key {
        crate::audit::record(
            &ctx,
            &actor_of(&provided),
            "http.auth_failed",
            &path,
            json!({ "reason": "client_key" }),
            false,
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": { "message": "无效的 API Key（BIT Client Key）", "type": "invalid_request_error", "code": "invalid_api_key" } })),
        )
            .into_response();
    }

    // 第二重：访问密码校验（OpenAI 兼容端点与 MCP 端点跳过）
    if !openai_endpoint && !mcp_endpoint {
        let provided_pwd = req
            .headers()
            .get("x-access-password")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !cfg.verify_access_password(provided_pwd) {
            crate::audit::record(
                &ctx,
                &actor_of(&provided),
                "http.auth_failed",
                &path,
                json!({ "reason": "access_password" }),
                false,
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "访问密码错误或缺失（需 X-Access-Password 头）" })),
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
    // 远程可指定会话；未指定则写入当前激活会话
    let session_id = body.get("session_id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if message.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "缺少 message 字段" })),
        )
            .into_response();
    }

    match crate::agent::chat_turn(&ctx, &session_id, &message, Vec::new()).await {
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
                Ok(_) => {
                    send_chunk(json!({}), Some("stop"));
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
