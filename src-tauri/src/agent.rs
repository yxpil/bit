use serde_json::json;
use std::sync::Arc;

use crate::ai::{self, ChatMessage};
use crate::state::{Ctx, CHAT_MAX};

/// 执行 AI 输出的单个工具调用（含 AI 自写插件能力）
pub async fn execute_tool_call(
    ctx: &Arc<Ctx>,
    name: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    match name {
        // ---- AI 基础能力：为自己写插件并注册 ----
        "write_plugin" => {
            let (pname, desc, code) = (
                params.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                params.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                params.get("code").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            );
            if pname.is_empty() || code.is_empty() {
                return Err("write_plugin 需要 name 与 code 参数".into());
            }
            // 注册前先试运行一次，确保脚本可用
            crate::script::run(&code, json!({})).map_err(|e| format!("插件代码校验失败: {e}"))?;
            let tool = crate::registry::register(
                ctx,
                &pname,
                &desc,
                json!({"type": "object", "properties": {}, "additionalProperties": true}),
                crate::registry::ToolKind::Script { code },
                "ai-self",
            )?;
            crate::audit::record(
                ctx,
                "ai-self",
                "plugin.write",
                &tool.name,
                json!({ "description": desc }),
                true,
            );
            Ok(json!({ "registered": tool.name, "id": tool.id }))
        }
        // ---- AI 基础能力：用本机解释器直接执行一段 JS/PY 代码 ----
        "run_script" => {
            let runtime = params.get("runtime").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let code = params.get("code").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let script_params = params.get("params").cloned().unwrap_or(json!({}));
            if runtime.is_empty() || code.is_empty() {
                return Err("run_script 需要 runtime 与 code 参数".into());
            }
            let ctx_cloned = ctx.clone();
            let handle = tauri::async_runtime::spawn_blocking(move || {
                crate::script_runtime::run(&ctx_cloned, &runtime, &code, &script_params)
            });
            let out = match tokio::time::timeout(std::time::Duration::from_secs(30), handle).await {
                Ok(res) => res.map_err(|e| format!("脚本任务失败: {e}"))?,
                Err(_) => Err("脚本执行超时（30 秒）".into()),
            };
            crate::audit::record(ctx, "ai-self", "script.run", "run_script", json!({ "ok": out.is_ok() }), out.is_ok());
            out
        }
        // ---- AI 基础能力：把一段 JS/PY 代码沉淀为常驻工具 ----
        "write_tool" => {
            let (tname, desc, runtime, code) = (
                params.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                params.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                params.get("runtime").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                params.get("code").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            );
            if tname.is_empty() || runtime.is_empty() || code.is_empty() {
                return Err("write_tool 需要 name / runtime / code 参数".into());
            }
            match crate::runtime::get(ctx, &runtime) {
                None => return Err(format!("解释器 `{runtime}` 未注册")),
                Some(rt) if !rt.enabled => return Err(format!("解释器 `{runtime}` 已暂停，无法用于新工具")),
                _ => {}
            }
            let tool = crate::registry::register(
                ctx,
                &tname,
                &desc,
                json!({"type": "object", "properties": {}, "additionalProperties": true}),
                crate::registry::ToolKind::Interpreter { runtime: runtime.clone(), code },
                "ai-self",
            )?;
            crate::audit::record(ctx, "ai-self", "tool.write", &tool.name, json!({ "runtime": runtime }), true);
            Ok(json!({ "registered": tool.name, "id": tool.id }))
        }
        "add_memory" => {
            let content = params.get("content").and_then(|v| v.as_str()).unwrap_or_default();
            let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("raw");
            if content.is_empty() {
                return Err("add_memory 需要 content 参数".into());
            }
            crate::memory::add_memory(ctx, content, kind, "ai");
            Ok(json!({ "saved": content }))
        }
        // ---- AI 基础能力：技能沉淀 ----
        "add_skill" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let summary = params.get("summary").and_then(|v| v.as_str()).unwrap_or_default();
            if name.is_empty() || summary.is_empty() {
                return Err("add_skill 需要 name 与 summary 参数".into());
            }
            crate::memory::add_skill(ctx, name, summary, "ai");
            Ok(json!({ "skill": name }))
        }
        // ---- AI 基础能力：目标 ----
        "goal_create" => {
            let title = params.get("title").and_then(|v| v.as_str()).unwrap_or_default();
            let detail = params.get("detail").and_then(|v| v.as_str()).unwrap_or_default();
            let g = crate::goal::create_goal(ctx, title, detail, "ai")?;
            crate::audit::record(ctx, "ai-self", "goal.create", &g.title, json!({}), true);
            Ok(json!({ "goal": g.title, "id": g.id }))
        }
        "goal_update" => {
            let id = params.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let status = params.get("status").and_then(|v| v.as_str()).unwrap_or_default();
            let g = crate::goal::update_goal_status(ctx, id, status)?;
            crate::audit::record(ctx, "ai-self", "goal.update", &g.title, json!({ "status": status }), true);
            Ok(json!({ "goal": g.title, "status": g.status }))
        }
        // ---- AI 基础能力：待办 ----
        "todo_add" => {
            let content = params.get("content").and_then(|v| v.as_str()).unwrap_or_default();
            let goal_id = params.get("goal_id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
            let t = crate::goal::add_todo(ctx, goal_id, content, "ai")?;
            crate::audit::record(ctx, "ai-self", "todo.add", &t.content, json!({}), true);
            Ok(json!({ "todo": t.content, "id": t.id }))
        }
        "todo_update" => {
            let id = params.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let status = params.get("status").and_then(|v| v.as_str()).unwrap_or_default();
            let t = crate::goal::update_todo_status(ctx, id, status)?;
            crate::audit::record(ctx, "ai-self", "todo.update", &t.content, json!({ "status": status }), true);
            Ok(json!({ "todo": t.content, "status": t.status }))
        }
        "todo_write" => {
            let items = params.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let goal_id = params.get("goal_id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
            let n = crate::goal::rewrite_todos(ctx, goal_id, &items, "ai")?;
            crate::audit::record(ctx, "ai-self", "todo.write", "todos", json!({ "count": n }), true);
            Ok(json!({ "written": n }))
        }
        // ---- 已注册工具（内置 / 远程 / AI 自写脚本插件） ----
        other => {
            let tool = {
                let tools = ctx.tools.lock().unwrap();
                tools
                    .iter()
                    .find(|t| t.name.eq_ignore_ascii_case(other))
                    .cloned()
                    .ok_or_else(|| format!("工具 `{other}` 不存在"))?
            };
            crate::registry::invoke(ctx, &tool.id, params.clone(), "ai-self").await
        }
    }
}

/// 对话主循环：调用模型 → 解析工具调用 → 执行 → 回喂结果 → 直到无工具调用
/// 结果写入指定会话（session_id 为空则写入当前激活会话），返回该会话最新完整消息列表
pub async fn chat_turn(
    ctx: &Arc<Ctx>,
    session_id: &str,
    user_input: &str,
    images: Vec<String>,
) -> Result<Vec<ChatMessage>, String> {
    // 1) 追加用户消息到目标会话
    {
        let mut store = ctx.sessions.lock().unwrap();
        let sess = if session_id.is_empty() {
            store.active_mut()
        } else {
            store.get_mut(session_id).ok_or("会话不存在")?
        };
        // 首条用户消息用于自动命名会话
        if sess.messages.iter().all(|m| m.role != "user") {
            let title: String = user_input.chars().take(20).collect();
            sess.title = title.replace('\n', " ");
        }
        sess.messages.push(ChatMessage::user(user_input));
        sess.touch();
        if sess.messages.len() > CHAT_MAX {
            let drop_n = sess.messages.len() - CHAT_MAX;
            sess.messages.drain(0..drop_n);
        }
    }
    crate::session::persist(ctx);

    let target = if session_id.is_empty() {
        ctx.sessions.lock().unwrap().active.clone()
    } else {
        session_id.to_string()
    };

    // 2) 构造发给模型的对话（system + 历史 + 每轮追加的工具反馈）
    let mut convo: Vec<ChatMessage> = {
        let store = ctx.sessions.lock().unwrap();
        let sess = store.sessions.iter().find(|s| s.id == target).ok_or("会话不存在")?;
        let mut v = vec![ChatMessage::system(ai::system_prompt(ctx))];
        // 只取最近若干条，且剥离 tool_calls（模型请求只需 role/content）
        for m in sess.messages.iter().rev().take(24).collect::<Vec<_>>().into_iter().rev() {
            v.push(ChatMessage { role: m.role.clone(), content: m.content.clone(), tool_calls: Vec::new() });
        }
        v
    };

    for round in 0..5 {
        // 图片只在第一轮（真正的用户轮）随请求发送，工具反馈轮不再重复携带
        let round_images: &[String] = if round == 0 { &images } else { &[] };
        let reply = ai::chat_with_images(ctx, &convo, round_images).await?;

        match crate::autopilot::parse_json_array(&reply) {
            Some(calls) if !calls.is_empty() && looks_like_tool_calls(&calls) => {
                // 执行工具，收集可视化记录
                let mut records: Vec<crate::ai::ToolCallRecord> = Vec::new();
                for call in calls.iter().take(6) {
                    let name = call.get("tool").and_then(|v| v.as_str()).unwrap_or_default();
                    let params = call.get("params").cloned().unwrap_or(json!({}));
                    let outcome = if name.is_empty() {
                        Err("缺少 tool 字段".to_string())
                    } else {
                        execute_tool_call(ctx, name, &params).await
                    };
                    records.push(crate::ai::ToolCallRecord {
                        tool: name.to_string(),
                        params,
                        ok: outcome.is_ok(),
                        result: outcome.unwrap_or_else(|e| json!(e)),
                    });
                }

                // 存一条带工具调用可视化的 assistant 消息（content 去掉裸 JSON，仅留说明文字）
                let visible = strip_tool_json(&reply);
                let mut msg = ChatMessage::assistant(visible);
                msg.tool_calls = records.clone();
                {
                    let mut store = ctx.sessions.lock().unwrap();
                    if let Some(sess) = store.get_mut(&target) {
                        sess.messages.push(msg);
                        sess.touch();
                        if sess.messages.len() > CHAT_MAX {
                            let drop_n = sess.messages.len() - CHAT_MAX;
                            sess.messages.drain(0..drop_n);
                        }
                    }
                }
                crate::session::persist(ctx);

                // 把结果回喂给模型（仅用于本轮请求，不落库为可见气泡）
                let feedback = format!(
                    "工具调用结果（继续你的回复，如已完成请直接输出最终答案）：\n{}",
                    serde_json::to_string_pretty(&records.iter().map(|r| json!({
                        "tool": r.tool, "ok": r.ok, "result": r.result
                    })).collect::<Vec<_>>()).unwrap_or_default()
                );
                convo.push(ChatMessage::assistant(reply.clone()));
                convo.push(ChatMessage::user(feedback));
            }
            _ => {
                // 纯文本回复：存入会话并结束
                {
                    let mut store = ctx.sessions.lock().unwrap();
                    if let Some(sess) = store.get_mut(&target) {
                        sess.messages.push(ChatMessage::assistant(reply.clone()));
                        sess.touch();
                        if sess.messages.len() > CHAT_MAX {
                            let drop_n = sess.messages.len() - CHAT_MAX;
                            sess.messages.drain(0..drop_n);
                        }
                    }
                }
                crate::session::persist(ctx);
                let out = ctx
                    .sessions
                    .lock()
                    .unwrap()
                    .sessions
                    .iter()
                    .find(|s| s.id == target)
                    .map(|s| s.messages.clone())
                    .unwrap_or_default();
                return Ok(out);
            }
        }
    }
    Err("工具调用轮次超出上限（5 轮）".into())
}

/// 流式版对话循环：通过 Tauri 事件 `event_name` 把增量文本 / 工具卡片 / 结束信号推给前端。
/// 事件 payload 形如 {type:"delta"|"tools"|"final"|"error", ...}
pub async fn chat_turn_stream(
    ctx: &Arc<Ctx>,
    session_id: &str,
    user_input: &str,
    event_name: &str,
    images: Vec<String>,
) -> Result<Vec<ChatMessage>, String> {
    use tauri::Emitter;
    let app = ctx.app.clone();
    let ev = event_name.to_string();
    let emit = move |payload: serde_json::Value| {
        let _ = app.emit(&ev, payload);
    };

    // 1) 追加用户消息
    {
        let mut store = ctx.sessions.lock().unwrap();
        let sess = if session_id.is_empty() {
            store.active_mut()
        } else {
            store.get_mut(session_id).ok_or("会话不存在")?
        };
        if sess.messages.iter().all(|m| m.role != "user") {
            let title: String = user_input.chars().take(20).collect();
            sess.title = title.replace('\n', " ");
        }
        sess.messages.push(ChatMessage::user(user_input));
        sess.touch();
        if sess.messages.len() > CHAT_MAX {
            let drop_n = sess.messages.len() - CHAT_MAX;
            sess.messages.drain(0..drop_n);
        }
    }
    crate::session::persist(ctx);

    let target = if session_id.is_empty() {
        ctx.sessions.lock().unwrap().active.clone()
    } else {
        session_id.to_string()
    };

    let mut convo: Vec<ChatMessage> = {
        let store = ctx.sessions.lock().unwrap();
        let sess = store.sessions.iter().find(|s| s.id == target).ok_or("会话不存在")?;
        let mut v = vec![ChatMessage::system(ai::system_prompt(ctx))];
        for m in sess.messages.iter().rev().take(24).collect::<Vec<_>>().into_iter().rev() {
            v.push(ChatMessage { role: m.role.clone(), content: m.content.clone(), tool_calls: Vec::new() });
        }
        v
    };

    for round in 0..5 {
        // 流式获取本轮回复，逐 token 推给前端
        emit(json!({ "type": "round_start" }));
        // 图片只在第一轮随请求发送
        let round_images: &[String] = if round == 0 { &images } else { &[] };
        let reply = {
            let emit_ref = &emit;
            ai::chat_stream_with_images(ctx, &convo, round_images, |tok| {
                emit_ref(json!({ "type": "delta", "text": tok }));
            })
            .await
        };
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                emit(json!({ "type": "error", "error": e.clone() }));
                return Err(e);
            }
        };

        match crate::autopilot::parse_json_array(&reply) {
            Some(calls) if !calls.is_empty() && looks_like_tool_calls(&calls) => {
                let mut records: Vec<crate::ai::ToolCallRecord> = Vec::new();
                for call in calls.iter().take(6) {
                    let name = call.get("tool").and_then(|v| v.as_str()).unwrap_or_default();
                    let params = call.get("params").cloned().unwrap_or(json!({}));
                    let outcome = if name.is_empty() {
                        Err("缺少 tool 字段".to_string())
                    } else {
                        execute_tool_call(ctx, name, &params).await
                    };
                    records.push(crate::ai::ToolCallRecord {
                        tool: name.to_string(),
                        params,
                        ok: outcome.is_ok(),
                        result: outcome.unwrap_or_else(|e| json!(e)),
                    });
                }

                let visible = strip_tool_json(&reply);
                let mut msg = ChatMessage::assistant(visible.clone());
                msg.tool_calls = records.clone();
                {
                    let mut store = ctx.sessions.lock().unwrap();
                    if let Some(sess) = store.get_mut(&target) {
                        sess.messages.push(msg);
                        sess.touch();
                        if sess.messages.len() > CHAT_MAX {
                            let drop_n = sess.messages.len() - CHAT_MAX;
                            sess.messages.drain(0..drop_n);
                        }
                    }
                }
                crate::session::persist(ctx);

                // 通知前端：本轮是工具调用 → 丢弃流式文本，改渲染工具卡片
                emit(json!({ "type": "tools", "visible": visible, "calls": records }));

                let feedback = format!(
                    "工具调用结果（继续你的回复，如已完成请直接输出最终答案）：\n{}",
                    serde_json::to_string_pretty(&records.iter().map(|r| json!({
                        "tool": r.tool, "ok": r.ok, "result": r.result
                    })).collect::<Vec<_>>()).unwrap_or_default()
                );
                convo.push(ChatMessage::assistant(reply.clone()));
                convo.push(ChatMessage::user(feedback));
            }
            _ => {
                {
                    let mut store = ctx.sessions.lock().unwrap();
                    if let Some(sess) = store.get_mut(&target) {
                        sess.messages.push(ChatMessage::assistant(reply.clone()));
                        sess.touch();
                        if sess.messages.len() > CHAT_MAX {
                            let drop_n = sess.messages.len() - CHAT_MAX;
                            sess.messages.drain(0..drop_n);
                        }
                    }
                }
                crate::session::persist(ctx);
                let out = ctx
                    .sessions
                    .lock()
                    .unwrap()
                    .sessions
                    .iter()
                    .find(|s| s.id == target)
                    .map(|s| s.messages.clone())
                    .unwrap_or_default();
                emit(json!({ "type": "final", "messages": out.clone() }));
                return Ok(out);
            }
        }
    }
    let err = "工具调用轮次超出上限（5 轮）".to_string();
    emit(json!({ "type": "error", "error": err.clone() }));
    Err(err)
}

/// 从回复里去掉裸 JSON 工具调用块，只保留 AI 的自然语言说明
fn strip_tool_json(reply: &str) -> String {
    let trimmed = reply.trim();
    // 若整段就是 JSON 数组，返回空（工具卡片已展示内容）
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return String::new();
    }
    // 否则去掉其中的 ```json ... ``` 或裸 [ ... ] 片段
    let mut out = String::new();
    let mut depth = 0i32;
    let mut in_fence = false;
    for line in reply.lines() {
        let l = line.trim();
        if l.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if l.starts_with('[') {
            depth += 1;
        }
        if depth > 0 {
            if l.ends_with(']') {
                depth -= 1;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// 只有元素都带 tool 字段才视为工具调用，避免把普通数组输出误判
fn looks_like_tool_calls(calls: &[serde_json::Value]) -> bool {
    calls.iter().all(|c| c.get("tool").and_then(|v| v.as_str()).is_some())
}
