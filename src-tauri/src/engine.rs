// yxpil · BIT
// 对话引擎调度垫片：所有"启动对话回合"的调用一律经这里分发——
// worker 活着就代理给子进程，否则（worker 停用 / 未就绪 / 代理失败）透明回退进程内执行。
// worker 进程内部调用（如 sub_agent、后台 shell 唤回）在此直接走本地路径，不做 HTTP 自环。
use crate::state::Ctx;
use serde_json::json;
use std::sync::Arc;

use crate::ai::ChatMessage;

/// 解析 worker 返回的 { messages: [...] }
fn parse_messages(v: serde_json::Value) -> Result<Vec<ChatMessage>, String> {
    serde_json::from_value(v.get("messages").cloned().unwrap_or(serde_json::Value::Null))
        .map_err(|e| format!("worker response malformed: {e}"))
}

/// 回合级结论性错误：worker 已经给出确定结果（如"对话已中断"），回退重跑只会
/// 重复入历史 + 让 AI 中断后又冒出一段话。这类错误直接透传给前端，绝不 fallback 重跑。
fn is_turn_final_error(e: &str) -> bool {
    e.contains("对话已中断")
}

/// 带自动推进的流式对话回合（UI 发消息主路径）
pub async fn chat_stream_auto(
    ctx: &Arc<Ctx>,
    session_id: &str,
    message: &str,
    event_name: &str,
    images: Vec<String>,
) -> Result<Vec<ChatMessage>, String> {
    if crate::worker::IN_WORKER.load(std::sync::atomic::Ordering::Relaxed) || !crate::worker::active() {
        return crate::agent::chat_turn_stream_auto(ctx, session_id, message, event_name, images).await;
    }
    match crate::worker::proxy_chat_stream(session_id, message, event_name, images.clone()).await {
        Ok(v) => parse_messages(v),
        Err(e) => {
            if is_turn_final_error(&e) {
                return Err(e);
            }
            // worker 突然失联：回退进程内，保功能可用；监督循环随后会重启 worker
            crate::audit::record(ctx, "host", "worker.proxy_fallback", "chat_stream", json!({ "error": e }), false);
            crate::agent::chat_turn_stream_auto(ctx, session_id, message, event_name, images).await
        }
    }
}

/// 带自动推进的对话回合（远程 API / 后台唤回 / TUI）
pub async fn chat_auto(
    ctx: &Arc<Ctx>,
    session_id: &str,
    message: &str,
    images: Vec<String>,
) -> Result<Vec<ChatMessage>, String> {
    if crate::worker::IN_WORKER.load(std::sync::atomic::Ordering::Relaxed) || !crate::worker::active() {
        return crate::agent::chat_turn_auto(ctx, session_id, message, images).await;
    }
    match crate::worker::proxy_chat(session_id, message, images.clone()).await {
        Ok(v) => parse_messages(v),
        Err(e) => {
            if is_turn_final_error(&e) {
                return Err(e);
            }
            crate::audit::record(ctx, "host", "worker.proxy_fallback", "chat", json!({ "error": e }), false);
            crate::agent::chat_turn_auto(ctx, session_id, message, images).await
        }
    }
}

/// 单回合对话（无自动推进）
pub async fn chat(
    ctx: &Arc<Ctx>,
    session_id: &str,
    message: &str,
    images: Vec<String>,
) -> Result<Vec<ChatMessage>, String> {
    // 单回合也走 auto 端点：worker 侧按其 config.auto_drive 行为一致
    // （进程内路径保持原语义：直接单回合）
    if crate::worker::active()
        && !crate::worker::IN_WORKER.load(std::sync::atomic::Ordering::Relaxed)
        && ctx.config.lock().unwrap().auto_drive
    {
        return chat_auto(ctx, session_id, message, images).await;
    }
    if crate::worker::active()
        && !crate::worker::IN_WORKER.load(std::sync::atomic::Ordering::Relaxed)
    {
        match crate::worker::proxy_chat_single(session_id, message, images.clone()).await {
            Ok(v) => return parse_messages(v),
            Err(e) => {
                crate::audit::record(ctx, "host", "worker.proxy_fallback", "chat_single", json!({ "error": e }), false);
            }
        }
    }
    crate::agent::chat_turn(ctx, session_id, message, images).await
}

/// 中断会话：worker 活着就转发；同时必查 host 本地中断表。
/// 回合可能因 worker "会话不存在" 而回退进程内执行（中断表注册在 host 侧），
/// 只问 worker 会出现"worker 说没有 → 提前返回 → host 里真正在跑的回合永远收不到中断"
/// 的僵尸回合（表现：终止按钮点了没反应、对话停不下来），所以两边都打标志取或。
pub async fn interrupt(ctx: &Arc<Ctx>, session_id: &str) -> Result<serde_json::Value, String> {
    let sid = if session_id.is_empty() { ctx.sessions.lock().unwrap().active.clone() } else { session_id.to_string() };
    let mut hit = false;
    if crate::worker::active()
        && !crate::worker::IN_WORKER.load(std::sync::atomic::Ordering::Relaxed)
    {
        if let Ok(v) = crate::worker::proxy_interrupt(&sid).await {
            hit = v.get("interrupted").and_then(|x| x.as_bool()).unwrap_or(false);
        }
    }
    let local_hit = {
        let map = ctx.interrupts.lock().unwrap();
        match map.get(&sid) {
            Some(flag) => {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
                true
            }
            None => false,
        }
    };
    hit = hit || local_hit;
    crate::audit::record(ctx, "local-app", "chat.interrupt", &sid, json!({ "was_running": hit }), true);
    Ok(json!({ "id": sid, "interrupted": hit }))
}

/// 审批应答：worker 活着就转发到 worker 的审批表（等待其 120 秒审批等待循环消费）；
/// worker 侧查无此 id 时再查 host 本地表——回合回退进程内执行时审批注册在 host 侧
pub async fn approve(ctx: &Arc<Ctx>, id: &str, allow: bool) -> Result<serde_json::Value, String> {
    if crate::worker::active()
        && !crate::worker::IN_WORKER.load(std::sync::atomic::Ordering::Relaxed)
    {
        match crate::worker::proxy_approve(id, allow).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                crate::audit::record(ctx, "host", "worker.proxy_fallback", "approve", json!({ "id": id, "error": e }), false);
                // worker 失联 / 无此审批 → 落到本地表兜底（不直接报错，回退回合的审批在这边）
            }
        }
    }
    let sender = ctx.approvals.lock().unwrap().remove(id).map(|p| p.tx);
    match sender {
        Some(tx) => {
            let _ = tx.send(allow);
            Ok(json!({ "id": id, "allow": allow }))
        }
        None => Err("审批请求不存在或已处理".into()),
    }
}

/// 待审批列表：worker 活着就取 worker 的审批表
pub async fn list_approvals(ctx: &Arc<Ctx>) -> Result<serde_json::Value, String> {
    // worker 与 host 两张审批表合并去重：回合回退进程内执行时，审批注册在 host 侧，
    // 只看 worker 表会让弹窗永远出不来（同一类"回退后状态分裂"问题）
    let mut items: Vec<serde_json::Value> = Vec::new();
    if crate::worker::active()
        && !crate::worker::IN_WORKER.load(std::sync::atomic::Ordering::Relaxed)
    {
        if let Ok(v) = crate::worker::proxy_list_approvals().await {
            if let Some(arr) = v.get("approvals").and_then(|x| x.as_array()) {
                items.extend(arr.iter().cloned());
            }
        }
    }
    let map = ctx.approvals.lock().unwrap();
    for (id, p) in map.iter() {
        // worker 已有的 id 不重复注入
        if items.iter().any(|x| x.get("id").and_then(|v| v.as_str()) == Some(id)) {
            continue;
        }
        items.push(json!({
            "id": id,
            "tool": p.tool,
            "params": p.params,
            "age_secs": p.created.elapsed().as_secs(),
        }));
    }
    Ok(json!({ "approvals": items }))
}
