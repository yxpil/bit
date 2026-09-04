use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::ai::{self, ChatMessage};
use crate::state::{Ctx, CHAT_MAX};

/// 会话中断：注册标志（chat_interrupt 置位后，执行循环在各检查点停止）
fn register_interrupt(ctx: &Arc<Ctx>, target: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    ctx.interrupts
        .lock()
        .unwrap()
        .insert(target.to_string(), flag.clone());
    flag
}

pub fn clear_interrupt(ctx: &Arc<Ctx>, target: &str) {
    ctx.interrupts.lock().unwrap().remove(target);
}

pub fn interrupted(ctx: &Arc<Ctx>, target: &str) -> bool {
    ctx.interrupts
        .lock()
        .unwrap()
        .get(target)
        .map(|f| f.load(Ordering::Relaxed))
        .unwrap_or(false)
}

/// 中断等待：每 150ms 轮询一次标志。配合 tokio::select! 让长请求（原生模式整段生成）
/// 和长工具执行随时可被 chat_interrupt 打断，前端无需等请求自然结束
async fn wait_interrupt(ctx: &Arc<Ctx>, target: &str) {
    loop {
        if interrupted(ctx, target) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

/// 历史窗口：总量超限时按 8 条为步长对齐回退（而非每轮滑动一条），
/// 使相邻多轮请求的消息前缀逐字节一致 → 命中各家提示词缓存（openai/gemini 自动、deepseek/kimi 等兼容端同样有效）
fn history_window<'a>(msgs: &'a [ChatMessage]) -> &'a [ChatMessage] {
    // 96 条窗口（8 步对齐保缓存命中）：长任务不被过早截断历史
    const WINDOW: usize = 96;
    let total = msgs.len();
    if total <= WINDOW {
        return msgs;
    }
    let skip = ((total - WINDOW).div_ceil(8)) * 8;
    &msgs[skip..]
}

/// 思考过程非空才随消息落库（None 序列化时跳过，兼容旧数据）
fn opt_thinking(t: &std::sync::Mutex<String>) -> Option<String> {
    let s = t.lock().unwrap().trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// 累计本会话 token 用量，返回带命中率的 usage 事件负载
fn record_and_payload(ctx: &Arc<Ctx>, session: &str, usage: &ai::TokenUsage) -> serde_json::Value {
    let stats = crate::state::record_usage(ctx, session, usage);
    json!({
        "requests": stats.requests,
        "prompt_tokens": stats.prompt_tokens,
        "cache_read_tokens": stats.cache_read_tokens,
        "cache_write_tokens": stats.cache_write_tokens,
        "completion_tokens": stats.completion_tokens,
        "hit_rate": (stats.hit_rate() * 1000.0).round() / 1000.0,
    })
}

/// 工具审批：弹出询问卡片等待用户应答（120 秒超时自动拒绝）。
/// 是否需要询问由 auto_pass() 在调用方判定，这里只负责"问"。
/// 等待期间每 500ms 轮询一次会话中断标志：用户点「停止」可立即取消审批中的工具
async fn request_approval(
    ctx: &Arc<Ctx>,
    tool: &str,
    params: &serde_json::Value,
    session_id: Option<&str>,
) -> Result<(), String> {
    use tauri::Emitter;
    let id = format!("ap-{}", ctx.approval_seq.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    ctx.approvals.lock().unwrap().insert(id.clone(), tx);
    let _ = ctx.app.emit(
        "tool-approval",
        json!({ "id": id, "tool": tool, "params": params }),
    );
    crate::audit::record(
        ctx,
        "ai-self",
        "tool.approval_request",
        tool,
        json!({ "params": params }),
        true,
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut rx = rx;
    let outcome = loop {
        tokio::select! {
            r = &mut rx => {
                break match r {
                    Ok(true) => Ok(()),
                    Ok(false) => Err(format!("User rejected the tool call `{tool}`")),
                    Err(_) => Err("Approval channel closed; auto-rejected".into()),
                };
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                // 等待期间用户中断了会话：立即取消审批
                if session_id
                    .map(|sid| interrupted(ctx, sid))
                    .unwrap_or(false)
                {
                    break Err("对话已中断".into());
                }
                if std::time::Instant::now() >= deadline {
                    break Err("Approval timed out (120s); auto-rejected".into());
                }
            }
        }
    };
    // 无论结果如何都清理审批表（若前端此刻才应答，tool_approve 端 remove 不到即为空操作）
    ctx.approvals.lock().unwrap().remove(&id);
    match &outcome {
        Ok(()) => crate::audit::record(ctx, "user", "tool.approved", tool, json!({}), true),
        Err(e) => crate::audit::record(
            ctx,
            "user",
            if e.contains("拒绝") { "tool.rejected" } else { "tool.approval_cancelled" },
            tool,
            json!({ "reason": e }),
            false,
        ),
    }
    outcome
}

/// auto 模式下自动通过的工具：沉淀类（记忆/技能/目标/待办/AI 自写工具）与只读类查询
/// 审批判定：该模式下此工具是否免询问自动放行
/// - allow_all：全部放行
/// - auto：安全工具（沉淀类 / 只读类）放行，其余询问
/// - ask（及其他值）：一律询问
fn auto_pass(mode: &str, tool: &str) -> bool {
    mode == "allow_all" || (mode == "auto" && is_safe_tool(tool))
}

fn is_safe_tool(tool: &str) -> bool {
    const SAFE: &[&str] = &[
        "add_memory",
        "add_skill",
        "goal_create",
        "goal_update",
        "todo_add",
        "todo_update",
        "todo_write",
        "write_plugin",
        "write_tool",
    ];
    if SAFE.iter().any(|s| s.eq_ignore_ascii_case(tool)) {
        return true;
    }
    let lower = tool.to_lowercase();
    ["read", "list", "search", "get", "view", "query"]
        .iter()
        .any(|k| lower.contains(k))
}

/// 执行 AI 输出的单个工具调用（含 AI 自写插件能力）。
/// session_id 用于审批等待期间响应会话中断；外部调用（远程/自动驾驶）传 None
pub async fn execute_tool_call(
    ctx: &Arc<Ctx>,
    name: &str,
    params: &serde_json::Value,
    session_id: Option<&str>,
) -> Result<serde_json::Value, String> {
    if !auto_pass(&ctx.config.lock().unwrap().tool_approval.clone(), name) {
        request_approval(ctx, name, params, session_id).await?;
    }
    match name {
        // ---- AI 基础能力：为自己写插件并注册 ----
        "write_plugin" => {
            let (pname, desc, code) = (
                params.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                params.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                params.get("code").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            );
            if pname.is_empty() || code.is_empty() {
                return Err("write_plugin requires name and code parameters".into());
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
                return Err("run_script requires runtime and code parameters".into());
            }
            let ctx_cloned = ctx.clone();
            let handle = tauri::async_runtime::spawn_blocking(move || {
                crate::script_runtime::run(&ctx_cloned, &runtime, &code, &script_params)
            });
            let out = match tokio::time::timeout(std::time::Duration::from_secs(30), handle).await {
                Ok(res) => res.map_err(|e| format!("脚本任务失败: {e}"))?,
                Err(_) => Err("Script execution timed out (30s)".into()),
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
                return Err("write_tool requires name / runtime / code parameters".into());
            }
            match crate::runtime::get(ctx, &runtime) {
                None => return Err(format!("Interpreter `{runtime}` is not registered")),
                Some(rt) if !rt.enabled => return Err(format!("Interpreter `{runtime}` is paused; cannot be used for a new tool")),
                _ => {}
            }
            let tool = crate::registry::register_opts(
                ctx,
                &tname,
                &desc,
                json!({"type": "object", "properties": {}, "additionalProperties": true}),
                crate::registry::ToolKind::Interpreter { runtime: runtime.clone(), code },
                "ai-self",
                true, // 同名自建工具覆盖更新（修正错误实现）
            )?;
            crate::audit::record(ctx, "ai-self", "tool.write", &tool.name, json!({ "runtime": runtime }), true);
            Ok(json!({ "registered": tool.name, "id": tool.id }))
        }
        "add_memory" => {
            let content = params.get("content").and_then(|v| v.as_str()).unwrap_or_default();
            let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("raw");
            if content.is_empty() {
                return Err("add_memory requires the content parameter".into());
            }
            crate::memory::add_memory(ctx, content, kind, "ai");
            Ok(json!({ "saved": content }))
        }
        // ---- AI 基础能力：技能沉淀 ----
        "add_skill" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let summary = params.get("summary").and_then(|v| v.as_str()).unwrap_or_default();
            if name.is_empty() || summary.is_empty() {
                return Err("add_skill requires name and summary parameters".into());
            }
            crate::memory::add_skill(ctx, name, summary, "ai");
            Ok(json!({ "skill": name }))
        }
        // ---- AI 基础能力：目标 ----
        "goal_create" => {
            let title = params.get("title").and_then(|v| v.as_str()).unwrap_or_default();
            let detail = params.get("detail").and_then(|v| v.as_str()).unwrap_or_default();
            let g = crate::goal::create_goal(ctx, title, detail, "ai", session_id)?;
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
            let t = crate::goal::add_todo(ctx, goal_id, content, "ai", session_id)?;
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
            let n = crate::goal::rewrite_todos(ctx, goal_id, &items, "ai", session_id)?;
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
            crate::registry::invoke(ctx, &tool.id, params.clone(), "ai-self", session_id).await
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
    use tauri::Emitter;
    // 1) 追加用户消息到目标会话
    {
        let mut store = ctx.sessions.lock().unwrap();
        let sess = if session_id.is_empty() {
            store.active_mut()
        } else {
            store.get_mut(session_id).ok_or("会话不存在")?
        };
        // 首条用户消息用于自动命名会话（仅当仍是默认标题：不覆盖 sub_agent 传入的标题与用户手动改名）
        if sess.title.trim().is_empty() || sess.title == "新对话" {
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
    register_interrupt(ctx, &target);

    // 原生工具调用探测：每个会话只探测一次（内存缓存不持久化，新会话自动重新探测，
    // 模型/接口更新后自适应）；探测过不支持的会话直接用文本约定提示词
    let mut native_mode = ctx.native_probe.lock().unwrap().get(&target).copied() != Some(false);

    // 2) 构造发给模型的对话（system + 历史 + 每轮追加的工具反馈）
    // 提示词随探测结果切换：原生模式不教文本调用格式，避免两种约定互相干扰
    let mut convo: Vec<ChatMessage> = {
        let store = ctx.sessions.lock().unwrap();
        let sess = store.sessions.iter().find(|s| s.id == target).ok_or("会话不存在")?;
        let sys = if native_mode {
            ai::system_prompt_native(ctx, Some(&target))
        } else {
            ai::system_prompt(ctx, Some(&target))
        };
        let mut v = vec![ChatMessage::system(sys)];
        // 只取最近若干条，且剥离 tool_calls（模型请求只需 role/content）
        for m in history_window(&sess.messages) {
            v.push(ChatMessage { role: m.role.clone(), content: m.content.clone(), tool_calls: Vec::new(), thinking: None });
        }
        v
    };

    // 工具调用轮次不设上限：链式任务可能需要任意多轮，由模型自行决定何时给出最终答案
    let mut native_exchanges: Vec<ai::ToolExchange> = Vec::new();
    let mut pending_images: Vec<String> = Vec::new();
    let mut round = 0usize;
    loop {
        round += 1;
        if interrupted(ctx, &target) {
            clear_interrupt(ctx, &target);
            return Err("对话已中断".into());
        }
        // 图片只在第一轮（真正的用户轮）随请求发送；view_image 看过的图从第二轮起随请求注入
        let round_images: &[String] = if round == 1 { &images } else { &pending_images };

        // 拿到本轮回复：原生 function calling 优先，未探测过/已支持时尝试；失败降级文本约定
        let round_thinking = Arc::new(std::sync::Mutex::new(String::new()));
        let (reply, native_calls): (String, Vec<ai::NativeToolCall>) = if native_mode {
            // 流式原生请求（文本/思考增量实时到达；HTTP API 无事件通道，回调仅作聚合）
            let attempt = tokio::select! {
                r = ai::chat_native_round_stream(ctx, &convo, round_images, &native_exchanges, |_ev| true) => r,
                _ = wait_interrupt(ctx, &target) => Err(ai::NativeErr::Other(String::new())),
            };
            if interrupted(ctx, &target) {
                clear_interrupt(ctx, &target);
                return Err("对话已中断".into());
            }
            match attempt {
                Ok(r) => {
                    ctx.native_probe.lock().unwrap().insert(target.clone(), true);
                    *round_thinking.lock().unwrap() = r.thinking;
                    // 记录本轮用量并推送缓存命中率统计
                    let payload = record_and_payload(ctx, &target, &r.usage);
                    let _ = ctx.app.emit("chat-usage", json!({ "session": target, "usage": payload }));
                    // 结构化调用为空时兜底解析文本约定（有的模型在原生模式下仍爱手写 TOOL: 行）
                    let calls = if !r.calls.is_empty() {
                        r.calls
                    } else if let Some(tc) = parse_tool_calls(&r.content) {
                        if !tc.is_empty() && looks_like_tool_calls(&tc) {
                            text_calls_to_native(&tc)
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    };
                    (r.content, calls)
                }
                Err(ai::NativeErr::Unsupported(_)) => {
                    // 端点拒绝 tools 参数：本会话降级文本约定（提示词同步切换），下个会话重新探测
                    ctx.native_probe.lock().unwrap().insert(target.clone(), false);
                    native_mode = false;
                    native_exchanges.clear();
                    convo[0] = ChatMessage::system(ai::system_prompt(ctx, Some(&target)));
                    round -= 1; // 重走本轮（保留第一轮携带图片的语义）
                    continue;
                }
                Err(ai::NativeErr::Other(e)) => return Err(e),
            }
        } else {
            // 文本协议统一走 SSE 流式（与桌面端一致）：非流式请求会被仅支持流式的端点拒绝
            // 思考过程增量（reasoning/thinking）累积进 round_thinking，随消息落库
            let think_buf = round_thinking.clone();
            let (reply, usage) = ai::chat_stream_with_images(ctx, &convo, round_images, move |kind, tok| {
                if kind == ai::TokenKind::Think {
                    think_buf.lock().unwrap().push_str(tok);
                }
                true
            })
            .await?;
            // 记录本轮用量并推送缓存命中率统计
            let payload = record_and_payload(ctx, &target, &usage);
            let _ = ctx.app.emit("chat-usage", json!({ "session": target, "usage": payload }));
            let calls = match parse_tool_calls(&reply) {
                Some(tc) if !tc.is_empty() && looks_like_tool_calls(&tc) => text_calls_to_native(&tc),
                _ => Vec::new(),
            };
            (reply, calls)
        };

        if !native_calls.is_empty() {
            // 并发执行全部工具调用（上限 16）：互不依赖的工具同时跑，结果仍按调用顺序回喂
            let calls: Vec<ai::NativeToolCall> = native_calls.iter().take(16).cloned().collect();
            let outcomes = futures_util::future::join_all(calls.iter().map(|call| {
                let target = target.clone();
                async move {
                    if call.name.is_empty() {
                        Err("Missing tool field".to_string())
                    } else {
                        tokio::select! {
                            r = execute_tool_call(ctx, &call.name, &call.args, Some(&target)) => r,
                            _ = wait_interrupt(ctx, &target) => Err(String::new()),
                        }
                    }
                }
            }))
            .await;
            if interrupted(ctx, &target) {
                clear_interrupt(ctx, &target);
                return Err("对话已中断".into());
            }
            let mut records: Vec<crate::ai::ToolCallRecord> = Vec::new();
            for (call, outcome) in calls.iter().zip(outcomes.into_iter()) {
                let ok = outcome.is_ok();
                let mut result = outcome.unwrap_or_else(|e| json!(e));
                // view_image：把 data_url 抽出注入下一轮请求（视觉模型看图），记录本身脱敏（base64 不回喂不落库）
                if call.name == "view_image" {
                    if ok {
                        if let Some(url) = result.get("data_url").and_then(|x| x.as_str()) {
                            pending_images.push(url.to_string());
                        }
                    }
                    if let Some(obj) = result.as_object_mut() {
                        obj.remove("data_url");
                    }
                }
                records.push(crate::ai::ToolCallRecord {
                    tool: call.name.clone(),
                    params: call.args.clone(),
                    ok,
                    result,
                });
            }

            // 存一条带工具调用可视化的 assistant 消息（content 去掉思考块与裸 JSON，仅留说明文字）
            let visible = strip_tool_json(&strip_think_blocks(&reply));
            let mut msg = ChatMessage::assistant(visible);
            msg.tool_calls = records.clone();
            msg.thinking = opt_thinking(&round_thinking);
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

            // 把结果回喂给模型：原生模式按协议回传（tool 消息/tool_result/functionResponse），
            // 文本模式拼 feedback 用户消息（仅用于本轮请求，不落库为可见气泡）
            if native_mode {
                native_exchanges.push(ai::ToolExchange {
                    calls: native_calls.clone(),
                    results: records
                        .iter()
                        .map(|r| ai::ToolResult { ok: r.ok, value: r.result.clone() })
                        .collect(),
                });
            } else {
                let feedback = format!(
                    "Tool result(s) - continue your reply; if everything is done, output the final answer directly:\n{}",
                    serde_json::to_string_pretty(&records.iter().map(|r| json!({
                        "tool": r.tool, "ok": r.ok, "result": r.result
                    })).collect::<Vec<_>>()).unwrap_or_default()
                );
                convo.push(ChatMessage::assistant(reply.clone()));
                convo.push(ChatMessage::user(feedback));
            }
            continue;
        }

        // 回复不完整（截断/纯思考残渣）：不结束回合，自动替用户补发「继续」，次数不限制（仅文本约定模式）
        if !native_mode && looks_truncated(&reply) {
            let visible = strip_tool_json(&strip_think_blocks(&reply));
            if !visible.is_empty() {
                let mut msg = ChatMessage::assistant(visible);
                msg.thinking = opt_thinking(&round_thinking);
                let mut store = ctx.sessions.lock().unwrap();
                if let Some(sess) = store.get_mut(&target) {
                    sess.messages.push(msg);
                    sess.touch();
                    if sess.messages.len() > CHAT_MAX {
                        let drop_n = sess.messages.len() - CHAT_MAX;
                        sess.messages.drain(0..drop_n);
                    }
                }
                drop(store);
                crate::session::persist(ctx);
            }
            convo.push(ChatMessage::assistant(reply.clone()));
            convo.push(ChatMessage::user(CONTINUE_PROMPT));
            continue;
        }
        // 纯文本回复：存入会话并结束（同时去掉思考块残渣）
        let visible = strip_tool_json(&strip_think_blocks(&reply));
        let visible = if visible.is_empty() { reply.clone() } else { visible };
        {
            let mut store = ctx.sessions.lock().unwrap();
            if let Some(sess) = store.get_mut(&target) {
                sess.messages.push(ChatMessage::assistant(visible));
                sess.touch();
                if sess.messages.len() > CHAT_MAX {
                    let drop_n = sess.messages.len() - CHAT_MAX;
                    sess.messages.drain(0..drop_n);
                }
            }
        }
        crate::session::persist(ctx);
        clear_interrupt(ctx, &target);
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

/// 把文本约定解析出的调用（[{tool,params}]）统一转成原生调用结构，两条路径共用执行逻辑
fn text_calls_to_native(calls: &[serde_json::Value]) -> Vec<ai::NativeToolCall> {
    calls
        .iter()
        .enumerate()
        .map(|(i, c)| ai::NativeToolCall {
            id: format!("txt-{i}"),
            name: c.get("tool").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            args: c.get("params").cloned().unwrap_or(json!({})),
        })
        .collect()
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
        // 首条用户消息用于自动命名会话（仅当仍是默认标题：不覆盖 sub_agent 传入的标题与用户手动改名）
        if sess.title.trim().is_empty() || sess.title == "新对话" {
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
    let iflag = register_interrupt(ctx, &target);

    // 原生工具调用探测：每个会话只探测一次（内存缓存不持久化，新会话自动重新探测）；
    // 提示词随探测结果切换：原生模式不教文本调用格式，避免两种约定互相干扰
    let mut native_mode = ctx.native_probe.lock().unwrap().get(&target).copied() != Some(false);

    let mut convo: Vec<ChatMessage> = {
        let store = ctx.sessions.lock().unwrap();
        let sess = store.sessions.iter().find(|s| s.id == target).ok_or("会话不存在")?;
        let sys = if native_mode {
            ai::system_prompt_native(ctx, Some(&target))
        } else {
            ai::system_prompt(ctx, Some(&target))
        };
        let mut v = vec![ChatMessage::system(sys)];
        for m in history_window(&sess.messages) {
            v.push(ChatMessage { role: m.role.clone(), content: m.content.clone(), tool_calls: Vec::new(), thinking: None });
        }
        v
    };

    // 工具调用轮次不设上限：链式任务可能需要任意多轮，由模型自行决定何时给出最终答案
    // 端点不支持 tools 参数时立即降级文本约定；原生模式下本轮回复一次性下发
    let mut native_exchanges: Vec<ai::ToolExchange> = Vec::new();
    let mut pending_images: Vec<String> = Vec::new();
    let mut round = 0usize;
    loop {
        round += 1;
        if interrupted(ctx, &target) {
            clear_interrupt(ctx, &target);
            let e = "对话已中断".to_string();
            emit(json!({ "type": "error", "error": e.clone(), "interrupted": true }));
            return Err(e);
        }
        // 流式获取本轮回复，逐 token 推给前端
        emit(json!({ "type": "round_start" }));
        // 思考过程缓冲（reasoning/thinking）：流式增量实时推送 + 结束后随消息落库，每轮重置
        let round_thinking = Arc::new(std::sync::Mutex::new(String::new()));
        // 图片只在第一轮随请求发送；view_image 看过的图从第二轮起随请求注入
        let round_images: &[String] = if round == 1 { &images } else { &pending_images };

        // 拿到本轮回复：原生 function calling 优先，失败降级文本约定
        let (reply, native_calls): (String, Vec<ai::NativeToolCall>) = if native_mode {
            // 流式原生请求：文本/思考增量实时推送（tool_calls 增量在 ai 层聚合，结束后一次性返回）
            let emit_native = &emit;
            let think_native = round_thinking.clone();
            let attempt = tokio::select! {
                r = ai::chat_native_round_stream(ctx, &convo, round_images, &native_exchanges, move |ev| {
                    match ev {
                        ai::NativeEvent::Think(t) => {
                            think_native.lock().unwrap().push_str(t);
                            emit_native(json!({ "type": "think", "text": t }));
                        }
                        ai::NativeEvent::Text(t) => emit_native(json!({ "type": "delta", "text": t })),
                    }
                    true
                }) => r,
                _ = wait_interrupt(ctx, &target) => Err(ai::NativeErr::Other(String::new())),
            };
            if interrupted(ctx, &target) {
                clear_interrupt(ctx, &target);
                let e = "对话已中断".to_string();
                emit(json!({ "type": "error", "error": e.clone(), "interrupted": true }));
                return Err(e);
            }
            match attempt {
                Ok(r) => {
                    ctx.native_probe.lock().unwrap().insert(target.clone(), true);
                    *round_thinking.lock().unwrap() = r.thinking;
                    // 记录本轮用量并随流式通道推送缓存命中率统计
                    let mut payload = record_and_payload(ctx, &target, &r.usage);
                    payload["type"] = json!("usage");
                    emit(payload);
                    let calls = if !r.calls.is_empty() {
                        r.calls
                    } else if let Some(tc) = parse_tool_calls(&r.content) {
                        if !tc.is_empty() && looks_like_tool_calls(&tc) {
                            text_calls_to_native(&tc)
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    };
                    (r.content, calls)
                }
                Err(ai::NativeErr::Unsupported(_)) => {
                    ctx.native_probe.lock().unwrap().insert(target.clone(), false);
                    native_mode = false;
                    native_exchanges.clear();
                    convo[0] = ChatMessage::system(ai::system_prompt(ctx, Some(&target)));
                    round -= 1;
                    continue;
                }
                Err(ai::NativeErr::Other(e)) => {
                    clear_interrupt(ctx, &target);
                    emit(json!({ "type": "error", "error": e.clone() }));
                    return Err(e);
                }
            }
        } else {
            // 流式过程中若收到中断请求：回调返回 false 立即断开 SSE 读取（不再等流自然结束）
            let stream_cancelled = Arc::new(AtomicBool::new(false));
            let result = {
                let emit_ref = &emit;
                let sc = stream_cancelled.clone();
                let iflag2 = iflag.clone();
                let think_buf = round_thinking.clone();
                ai::chat_stream_with_images(ctx, &convo, round_images, move |kind, tok| {
                    if iflag2.load(Ordering::Relaxed) {
                        sc.store(true, Ordering::Relaxed);
                        return false;
                    }
                    match kind {
                        // 思考过程增量：实时推给前端展示 + 累积缓冲（结束后随消息落库）
                        ai::TokenKind::Think => {
                            think_buf.lock().unwrap().push_str(tok);
                            emit_ref(json!({ "type": "think", "text": tok }));
                        }
                        ai::TokenKind::Text => emit_ref(json!({ "type": "delta", "text": tok })),
                    }
                    true
                })
                .await
            };
            if interrupted(ctx, &target) || stream_cancelled.load(Ordering::Relaxed) {
                clear_interrupt(ctx, &target);
                let e = "对话已中断".to_string();
                emit(json!({ "type": "error", "error": e.clone(), "interrupted": true }));
                return Err(e);
            }
            let (reply, round_usage) = match result {
                Ok(r) => r,
                Err(e) => {
                    clear_interrupt(ctx, &target);
                    emit(json!({ "type": "error", "error": e.clone() }));
                    return Err(e);
                }
            };
            // 记录本轮用量并随流式通道推送缓存命中率统计
            let mut payload = record_and_payload(ctx, &target, &round_usage);
            payload["type"] = json!("usage");
            emit(payload);
            let calls = match parse_tool_calls(&reply) {
                Some(tc) if !tc.is_empty() && looks_like_tool_calls(&tc) => text_calls_to_native(&tc),
                _ => Vec::new(),
            };
            (reply, calls)
        };

        // 原生模式已走流式（增量实时推送）；端点不支持流式时由 chat_native_round_stream 整段补发，无需在此重复下发

        if !native_calls.is_empty() {
            // 并发执行全部工具调用（上限 16）：互不依赖的工具同时跑，结果仍按调用顺序回喂
            let calls: Vec<ai::NativeToolCall> = native_calls.iter().take(16).cloned().collect();
            let outcomes = futures_util::future::join_all(calls.iter().map(|call| {
                let target = target.clone();
                async move {
                    if call.name.is_empty() {
                        Err("Missing tool field".to_string())
                    } else {
                        tokio::select! {
                            r = execute_tool_call(ctx, &call.name, &call.args, Some(&target)) => r,
                            _ = wait_interrupt(ctx, &target) => Err(String::new()),
                        }
                    }
                }
            }))
            .await;
            if interrupted(ctx, &target) {
                clear_interrupt(ctx, &target);
                let e = "对话已中断".to_string();
                emit(json!({ "type": "error", "error": e.clone(), "interrupted": true }));
                return Err(e);
            }
            let mut records: Vec<crate::ai::ToolCallRecord> = Vec::new();
            for (call, outcome) in calls.iter().zip(outcomes.into_iter()) {
                let ok = outcome.is_ok();
                let mut result = outcome.unwrap_or_else(|e| json!(e));
                // view_image：把 data_url 抽出注入下一轮请求（视觉模型看图），记录本身脱敏（base64 不回喂不落库）
                if call.name == "view_image" {
                    if ok {
                        if let Some(url) = result.get("data_url").and_then(|x| x.as_str()) {
                            pending_images.push(url.to_string());
                        }
                    }
                    if let Some(obj) = result.as_object_mut() {
                        obj.remove("data_url");
                    }
                }
                records.push(crate::ai::ToolCallRecord {
                    tool: call.name.clone(),
                    params: call.args.clone(),
                    ok,
                    result,
                });
            }

            let visible = strip_tool_json(&strip_think_blocks(&reply));
            let mut msg = ChatMessage::assistant(visible.clone());
            msg.tool_calls = records.clone();
            msg.thinking = opt_thinking(&round_thinking);
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

            // 回喂：原生模式按协议回传，文本模式拼 feedback 用户消息
            if native_mode {
                native_exchanges.push(ai::ToolExchange {
                    calls: native_calls.clone(),
                    results: records
                        .iter()
                        .map(|r| ai::ToolResult { ok: r.ok, value: r.result.clone() })
                        .collect(),
                });
            } else {
                let feedback = format!(
                    "Tool result(s) - continue your reply; if everything is done, output the final answer directly:\n{}",
                    serde_json::to_string_pretty(&records.iter().map(|r| json!({
                        "tool": r.tool, "ok": r.ok, "result": r.result
                    })).collect::<Vec<_>>()).unwrap_or_default()
                );
                convo.push(ChatMessage::assistant(reply.clone()));
                convo.push(ChatMessage::user(feedback));
            }
            continue;
        }

        // 回复不完整（截断/纯思考残渣）：不结束回合，自动补发「继续」（仅文本约定模式）
        if !native_mode && looks_truncated(&reply) {
            let visible = strip_tool_json(&strip_think_blocks(&reply));
            if !visible.is_empty() {
                let mut msg = ChatMessage::assistant(visible.clone());
                msg.thinking = opt_thinking(&round_thinking);
                let mut store = ctx.sessions.lock().unwrap();
                if let Some(sess) = store.get_mut(&target) {
                    sess.messages.push(msg);
                    sess.touch();
                    if sess.messages.len() > CHAT_MAX {
                        let drop_n = sess.messages.len() - CHAT_MAX;
                        sess.messages.drain(0..drop_n);
                    }
                }
                drop(store);
                crate::session::persist(ctx);
            }
            // 通知前端：本轮是不完整回复 → 用清洗后的片段替换原始流式文本
            emit(json!({ "type": "continue", "visible": visible }));
            convo.push(ChatMessage::assistant(reply.clone()));
            convo.push(ChatMessage::user(CONTINUE_PROMPT));
            continue;
        }
        // 纯文本回复：存入会话并结束（同时去掉思考块残渣）
        let visible = strip_tool_json(&strip_think_blocks(&reply));
        let visible = if visible.is_empty() { reply.clone() } else { visible };
        {
            let mut msg = ChatMessage::assistant(visible);
            msg.thinking = opt_thinking(&round_thinking);
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
        clear_interrupt(ctx, &target);
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

/// 构造发给模型的完整上下文（system prompt + 最近消息 + 工具清单），对话与预览共用
pub fn build_context(
    ctx: &Arc<Ctx>,
    session_id: &str,
) -> Result<(Vec<ChatMessage>, serde_json::Value), String> {
    let target = if session_id.is_empty() {
        ctx.sessions.lock().unwrap().active.clone()
    } else {
        session_id.to_string()
    };
    let store = ctx.sessions.lock().unwrap();
    let sess = store
        .sessions
        .iter()
        .find(|s| s.id == target)
        .ok_or("会话不存在")?;
    let mut v = vec![ChatMessage::system(ai::system_prompt(ctx, Some(&target)))];
    for m in sess.messages.iter().rev().take(24).collect::<Vec<_>>().into_iter().rev() {
        v.push(ChatMessage { role: m.role.clone(), content: m.content.clone(), tool_calls: Vec::new(), thinking: None });
    }
    Ok((v, ai::tools_manifest(ctx)))
}

/// 从回复里去掉工具调用 JSON（裸数组 / ```json 围栏 / 散落的单对象）与
/// <xxx_function_call> 之类自创标记，只保留 AI 的自然语言说明
fn strip_tool_json(reply: &str) -> String {
    let trimmed = reply.trim();
    // 若整段就是 JSON 数组，返回空（工具卡片已展示内容）
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return String::new();
    }
    // 先截掉未闭合的工具 JSON 残尾（输出被截断时不会有完整对象可解析）
    let reply = match find_unterminated_tool_json(reply) {
        Some(start) => &reply[..start],
        None => reply,
    };
    // 先按字节区间删除散落的 {"tool":...} 对象；若对象外层的数组因此变空，连 [] 一起删
    let spans = find_tool_objects(reply);
    let base = if spans.is_empty() {
        reply.to_string()
    } else {
        let mut out = String::new();
        let mut last = 0;
        for (_, s, e) in &spans {
            let mut start = *s;
            let mut end = *e;
            let before = reply[..start].trim_end();
            let after = reply[end..].trim_start();
            if before.ends_with('[') && after.starts_with(']') {
                start = before.len() - 1;
                end = reply.len() - after.len() + 1;
            }
            if start >= last {
                out.push_str(&reply[last..start]);
                last = last.max(end);
            }
        }
        out.push_str(&reply[last..]);
        out
    };
    // 逐行清理：代码围栏残行、自创标记残行、多余空行
    let mut lines: Vec<&str> = Vec::new();
    for line in base.lines() {
        let l = line.trim();
        if l.starts_with("```") {
            continue;
        }
        // 只剩数组标点的残行（平衡对象 + 截断残尾混合后的遗留）
        if !l.is_empty() && l.chars().all(|c| matches!(c, '[' | ']' | ',' | ' ' | '\t')) {
            continue;
        }
        if l.starts_with('<') && (l.contains("function_call") || l.contains("tool_call")) {
            continue;
        }
        if l.is_empty() && lines.last().map(|p: &&str| p.trim().is_empty()).unwrap_or(true) {
            continue; // 跳过行首与连续空行
        }
        lines.push(line);
    }
    while lines.last().map(|p: &&str| p.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n").trim().to_string()
}

/// 只有元素都带 tool 字段才视为工具调用，避免把普通数组输出误判
fn looks_like_tool_calls(calls: &[serde_json::Value]) -> bool {
    calls.iter().all(|c| c.get("tool").and_then(|v| v.as_str()).is_some())
}

/// 去掉模型输出里混入的思考块：成对的 <think>...</think>、未闭合的 <think> 残尾、游离的 </think>
pub fn strip_think_blocks(reply: &str) -> String {
    let mut out = String::new();
    let mut rest = reply;
    loop {
        match (rest.find("<think>"), rest.find("</think>")) {
            (Some(o), close) => {
                // </think> 出现在 <think> 之前：游离闭合标记，仅删标记本身
                if close.map_or(false, |c| c < o) {
                    let c = close.unwrap();
                    out.push_str(&rest[..c]);
                    rest = &rest[c + "</think>".len()..];
                } else {
                    out.push_str(&rest[..o]);
                    let after = &rest[o + "<think>".len()..];
                    rest = match after.find("</think>") {
                        Some(c) => &after[c + "</think>".len()..],
                        // 未闭合的思考残尾：整体丢弃
                        None => "",
                    };
                }
            }
            (None, Some(c)) => {
                out.push_str(&rest[..c]);
                rest = &rest[c + "</think>".len()..];
            }
            (None, None) => {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

/// 判断回复是否「话没说完就被截断」——此时不结束回合，自动替用户补发「继续」。
/// 覆盖：空回复、纯思考残渣、<think> 未闭合、以冒号/省略号收尾、代码围栏未闭合
fn looks_truncated(reply: &str) -> bool {
    let t = reply.trim();
    if t.is_empty() {
        return true;
    }
    if strip_think_blocks(t).trim().is_empty() {
        return true;
    }
    if t.matches("<think>").count() > t.matches("</think>").count() {
        return true;
    }
    if let Some(last) = t.chars().last() {
        if matches!(last, ':' | '：' | '…') || t.ends_with("...") {
            return true;
        }
    }
    if t.lines().filter(|l| l.trim_start().starts_with("```")).count() % 2 == 1 {
        return true;
    }
    // 工具 JSON 写到一半被截断（大 content 撑爆输出上限的典型形态）
    if find_unterminated_tool_json(t).is_some() {
        return true;
    }
    false
}

/// 自动续发时补给模型的消息
const CONTINUE_PROMPT: &str = "继续（你上一条回复未输出完整就被截断了：若要调用工具请按协议单独一行输出 JSON 数组；如需写入大文件，请拆成多次较小的写入避免单次输出过长；若已完成请直接给出最终答案）";

/// 扫描 JSON 文本（对象/字符串状态机），到达末尾时是否仍未闭合
fn is_unbalanced_json(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for &c in bytes {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
        }
    }
    depth > 0 || in_str || esc
}

/// 查找未闭合的工具 JSON 残尾起始位置（含前置 `[`）。
/// 典型场景：write_file 的 content 太长，输出在 JSON 中途被截断——
/// 此时解析不出工具调用，原始 JSON 会漏到正文里，回合也错误地结束。
fn find_unterminated_tool_json(reply: &str) -> Option<usize> {
    let mut search_end = reply.len();
    while let Some(pos) = reply[..search_end].rfind("{\"tool\"") {
        if is_unbalanced_json(&reply[pos..]) {
            // 若对象前面紧邻 `[`，把数组起点一起纳入
            let before = reply[..pos].trim_end();
            let start = if before.ends_with('[') {
                before.len() - 1
            } else {
                pos
            };
            return Some(start);
        }
        search_end = pos;
    }
    None
}

/// 扫描文本中所有平衡的 {...} JSON 对象，返回（解析值, 起始字节, 结束字节）。
/// 只保留含字符串字段 "tool" 的对象——部分模型不按数组协议输出，而是每行一个
/// 裸对象、甚至自创 <xxx_function_call> 之类的标记前缀
fn find_tool_objects(reply: &str) -> Vec<(serde_json::Value, usize, usize)> {
    let bytes = reply.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let mut depth = 0i32;
        let mut in_str = false;
        let mut esc = false;
        let mut j = i;
        while j < bytes.len() {
            let c = bytes[j];
            if in_str {
                if esc {
                    esc = false;
                } else if c == b'\\' {
                    esc = true;
                } else if c == b'"' {
                    in_str = false;
                }
            } else {
                match c {
                    b'"' => in_str = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        if j >= bytes.len() || depth != 0 {
            break; // 剩余部分不平衡，放弃
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&reply[i..=j]) {
            if v.get("tool").and_then(|t| t.as_str()).is_some() {
                out.push((v, i, j + 1));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// 解析回复中的工具调用：标准 JSON 数组协议优先；
/// 兼容模型散落的单个 {"tool":...} 对象（可带 <xxx_function_call> 自创标记）
fn parse_tool_calls(reply: &str) -> Option<Vec<serde_json::Value>> {
    if let Some(calls) = crate::autopilot::parse_json_array(reply) {
        if !calls.is_empty() && looks_like_tool_calls(&calls) {
            return Some(calls);
        }
    }
    let objs = find_tool_objects(reply);
    if objs.is_empty() {
        None
    } else {
        Some(objs.into_iter().map(|(v, _, _)| v).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_standard_array() {
        let reply = r#"[{"tool":"shell","params":{"command":"echo hi"}}]"#;
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["tool"], "shell");
    }

    /// 审批模式判定：allow_all 全放行 / auto 仅安全工具 / ask 一律询问
    #[test]
    fn test_approval_modes() {
        // allow_all：任何工具（含危险操作）都放行
        assert!(auto_pass("allow_all", "shell"));
        assert!(auto_pass("allow_all", "edit"));
        assert!(auto_pass("allow_all", "delete_tool"));
        // auto：沉淀类与只读类放行，执行类询问
        assert!(auto_pass("auto", "add_skill"));
        assert!(auto_pass("auto", "add_memory"));
        assert!(auto_pass("auto", "list_tools"));
        assert!(auto_pass("auto", "view_image"));
        // skill 工具含 save 分支（有写入语义），保守处理：询问
        assert!(!auto_pass("auto", "skill"));
        assert!(!auto_pass("auto", "shell"));
        assert!(!auto_pass("auto", "edit"));
        assert!(!auto_pass("auto", "write_file"));
        assert!(!auto_pass("auto", "delete_tool"));
        // ask：全部询问
        assert!(!auto_pass("ask", "shell"));
        assert!(!auto_pass("ask", "add_skill"));
        assert!(!auto_pass("ask", "list_tools"));
        // 未知模式按 ask 处理（保守）
        assert!(!auto_pass("", "shell"));
        assert!(!auto_pass("unknown", "add_memory"));
    }

    #[test]
    fn test_parse_array_in_fence() {
        let reply = "好的，我来执行：\n```json\n[{\"tool\":\"write_file\",\"params\":{\"path\":\"C:\\\\a.txt\",\"content\":\"x\"}}]\n```";
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["tool"], "write_file");
    }

    /// 模型自创 <dots_function_call> 标记 + 每行一个裸对象（截图中的真实案例）
    #[test]
    fn test_parse_scattered_objects_with_markup() {
        let reply = "<dots_function_call> {\"tool\":\"shell\",\"params\":{\"command\":\"ls -la /d/\"}}\n\
                     <dots_function_call> {\"tool\":\"shell\",\"params\":{\"command\":\"du -sh /d/\"}}\n\
                     <dots_function_call> {\"tool\":\"shell\",\"params\":{\"command\":\"df -h /d/ 2>/dev/null || echo 'df not available'\"}}";
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0]["tool"], "shell");
        assert_eq!(calls[2]["params"]["command"], "df -h /d/ 2>/dev/null || echo 'df not available'");
    }

    #[test]
    fn test_parse_single_bare_object() {
        let reply = r#"我来查看：{"tool":"shell","params":{"command":"dir"}}"#;
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["params"]["command"], "dir");
    }

    /// 不含 tool 字段的 JSON 不应被误判为工具调用
    #[test]
    fn test_plain_json_not_tool_call() {
        let reply = r#"示例：{"name": "test", "value": 42}"#;
        assert!(parse_tool_calls(reply).is_none());
    }

    /// 中文字符串（多字节 UTF-8）不得导致字节扫描 panic 或解析失败
    #[test]
    fn test_utf8_safety() {
        let reply = "你好世界 {\"tool\":\"shell\",\"params\":{\"command\":\"echo 中文测试\"}} 完成";
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls[0]["params"]["command"], "echo 中文测试");
    }

    /// 字符串值里包含花括号/方括号不得破坏平衡扫描
    #[test]
    fn test_braces_inside_strings() {
        let reply = r#"{"tool":"shell","params":{"command":"echo '{not a json}' [ok]"}}"#;
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls[0]["params"]["command"], "echo '{not a json}' [ok]");
    }

    /// 不平衡的对象应整体放弃而不是 panic
    #[test]
    fn test_unbalanced_object() {
        let reply = r#"{"tool":"shell","params":{"command":"unbalanced"#;
        assert!(parse_tool_calls(reply).is_none());
    }

    /// 工具数组与普通数组混合：数组含非 tool 元素时回退散对象扫描
    #[test]
    fn test_mixed_array_falls_back() {
        let reply = r#"[{"a":1},{"tool":"shell","params":{"command":"x"}}]"#;
        // 数组元素不全带 tool → 数组协议不命中；散对象扫描命中 1 个
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["tool"], "shell");
    }

    #[test]
    fn test_strip_marked_objects() {
        let reply = "<dots_function_call> {\"tool\":\"shell\",\"params\":{\"command\":\"ls\"}}\n好的，开始整理 D 盘。";
        let visible = strip_tool_json(reply);
        assert!(!visible.contains("dots_function_call"));
        assert!(!visible.contains("\"tool\""));
        assert!(visible.contains("整理 D 盘"));
    }

    #[test]
    fn test_strip_pure_array_returns_empty() {
        let reply = r#"[{"tool":"shell","params":{}}]"#;
        assert_eq!(strip_tool_json(reply), "");
    }

    #[test]
    fn test_strip_keeps_plain_text() {
        let reply = "这是普通回答，没有任何工具调用。";
        assert_eq!(strip_tool_json(&strip_think_blocks(reply)), reply);
    }

    #[test]
    fn test_strip_think_paired() {
        let reply = "<think>推理过程</think>最终答案";
        assert_eq!(strip_think_blocks(reply), "最终答案");
    }

    /// 截图中的真实案例：游离 </think> + 成对 <think>，去掉后没有正文 → 视为截断
    #[test]
    fn test_think_residue_is_truncated() {
        let reply = "</think> </think> <think>让我检查回收站的内容，看看还有什么残留。 </think>";
        assert!(looks_truncated(reply));
        assert_eq!(strip_think_blocks(reply).trim(), "");
    }

    #[test]
    fn test_unclosed_think_is_truncated() {
        assert!(looks_truncated("<think>推理到一半"));
        assert!(!looks_truncated("<think>推理</think>做完了。"));
    }

    #[test]
    fn test_trailing_colon_is_truncated() {
        assert!(looks_truncated("让我先检查回收站里具体还有什么，然后针对性清理："));
        assert!(looks_truncated("让我尝试其他方法..."));
        assert!(!looks_truncated("清理完成，共删除 12 个文件。"));
    }

    #[test]
    fn test_unclosed_code_fence_is_truncated() {
        assert!(looks_truncated("代码如下：\n```python\nprint(1)"));
        assert!(!looks_truncated("代码如下：\n```python\nprint(1)\n```\n完成。"));
    }

    #[test]
    fn test_empty_reply_is_truncated() {
        assert!(looks_truncated(""));
        assert!(looks_truncated("   \n  "));
    }

    /// 截图中的真实案例：write_file 的 content 输出到一半被截断——
    /// 应判定为截断（自动续发），且残尾 JSON 不得漏进正文
    #[test]
    fn test_unterminated_tool_json_is_truncated() {
        let reply = concat!(
            "抱歉，让我完成创建DIY 3D打印机BOM清单：\n",
            r#"[{"tool":"write_file","params":{"path":"C:\\Users\\yxpil\\Desktop\\3D打印\\DIY_BOM清单.html","content":"<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"UTF-8\">"#,
        );
        assert!(looks_truncated(reply));
        // 完整对象不算截断残尾
        assert!(!looks_truncated(
            r#"执行：[{"tool":"shell","params":{"command":"dir"}}] 完成。"#
        ));
    }

    /// 截断的 JSON 残尾应从正文中删除，只保留自然语言部分
    #[test]
    fn test_strip_unterminated_tool_json() {
        let reply = concat!(
            "好的，我来写入文件：\n",
            r#"[{"tool":"write_file","params":{"path":"C:\\a.html","content":"<!DOCTYPE html>"#,
        );
        let visible = strip_tool_json(reply);
        assert!(visible.contains("好的，我来写入文件"));
        assert!(!visible.contains("tool"));
        assert!(!visible.contains("DOCTYPE"));
    }

    /// 平衡对象在前、截断残尾在后：平衡对象仍可解析执行，残尾从正文删除
    #[test]
    fn test_balanced_then_unterminated() {
        let reply = concat!(
            r#"[{"tool":"shell","params":{"command":"mkdir work"}},"#,
            r#"{"tool":"write_file","params":{"path":"C:\\b.html","content":"<html>"#,
        );
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["tool"], "shell");
        assert_eq!(strip_tool_json(reply), "");
    }
}
