use crate::state::Ctx;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
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
        .layer(middleware::from_fn_with_state(ctx.clone(), auth))
        .with_state(ctx)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "service": "BIT", "time": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string() }))
}

/// 双重认证：Bearer Client Key + X-Access-Password 访问密码，含 HTTP 请求审计
async fn auth(State(ctx): State<Arc<Ctx>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if path == "/api/health" {
        return next.run(req).await;
    }

    let cfg = ctx.config.lock().unwrap().clone();
    let provided = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_string();

    if provided != cfg.client_key {
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
            Json(json!({ "error": "无效的 Client Key" })),
        )
            .into_response();
    }

    // 第二重：访问密码校验
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
    match crate::registry::invoke(&ctx, &id, params, &actor).await {
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

    match crate::agent::chat_turn(&ctx, &session_id, &message).await {
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
