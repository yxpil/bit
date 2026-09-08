// yxpil · BIT
use futures_util::StreamExt;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::ai::{self, ChatMessage};
use crate::state::{Ctx, CHAT_MAX};

/// 同会话回合互斥守卫：Drop 时自动释放（覆盖错误路径与 panic），杜绝并发回合交错写会话历史
pub struct TurnGuard {
    ctx: Arc<Ctx>,
    sid: String,
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.ctx.turn_locks.lock().unwrap().remove(&self.sid);
    }
}

/// 进入回合前抢占会话锁：已有回合在跑时快速失败（而非交叠执行导致历史错乱/僵尸流）。
/// 子任务（registry 的 task 工具）使用独立新会话 id，不受影响
fn acquire_turn(ctx: &Arc<Ctx>, sid: &str) -> Result<TurnGuard, String> {
    let mut locks = ctx.turn_locks.lock().unwrap();
    if locks.contains_key(sid) {
        return Err("该会话已有正在执行的回合，请等待完成后再发送新消息".into());
    }
    locks.insert(sid.to_string(), ());
    Ok(TurnGuard { ctx: ctx.clone(), sid: sid.to_string() })
}

/// 会话中断：注册标志（chat_interrupt 置位后，执行循环在各检查点停止）
fn register_interrupt(ctx: &Arc<Ctx>, target: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    ctx.interrupts
        .lock()
        .unwrap()
        .insert(target.to_string(), flag.clone());
    flag
}

pub fn clear_interrupt(ctx: &Arc<Ctx>, target: &str, flag: &Arc<AtomicBool>) {
    let mut map = ctx.interrupts.lock().unwrap();
    // 仅当映射中仍是本回合注册的标志时才摘除：防止误删并发/更新回合刚注册的
    // 标志（误删后新回合将永远收不到中断请求，变成打不断的僵尸回合）
    if map
        .get(target)
        .map(|f| Arc::ptr_eq(f, flag))
        .unwrap_or(false)
    {
        map.remove(target);
    }
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

/// 给 `round_tools_starting` 事件生成工具的简短描述（前端 spinner 卡片显示）
fn tool_summary(name: &str, args: &serde_json::Value) -> String {
    let p = args.as_object();
    match name {
        "shell" => p.and_then(|o| o.get("command")).and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_default(),
        "write_file" | "edit" | "send_file" | "view_image" => p.and_then(|o| o.get("path")).and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_default(),
        "plan" => p.and_then(|o| o.get("goal")).and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// 把已流出的部分回复落库（中断 / 上游中途断流时调用），返回清洗后的可见文本。
/// 空文本不落库返回空串；可见文本为空时保留原文，避免什么都不剩
fn persist_partial(
    ctx: &Arc<Ctx>,
    target: &str,
    partial: String,
    thinking: &Arc<std::sync::Mutex<String>>,
) -> String {
    let p = partial.trim().to_string();
    if p.is_empty() {
        return String::new();
    }
    let cleaned = strip_tool_json(&strip_think_blocks(&p));
    let visible = if cleaned.is_empty() { p } else { cleaned };
    let mut msg = ChatMessage::assistant(visible.clone());
    msg.thinking = opt_thinking(thinking);
    let mut store = ctx.sessions.lock().unwrap();
    if let Some(sess) = store.get_mut(target) {
        sess.messages.push(msg);
        sess.touch();
        if sess.messages.len() > CHAT_MAX {
            let drop_n = sess.messages.len() - CHAT_MAX;
            sess.messages.drain(0..drop_n);
        }
    }
    drop(store);
    crate::session::persist(ctx);
    visible
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
        // 上游从未上报缓存字段时 hit_rate 无意义（中转剥掉 usage 细节），前端显示「未知」
        "cache_known": stats.cache_known_requests > 0,
    })
}

/// 原生探测缓存的键 = 激活提供方 id：能否原生调用工具是「哪家端点」的属性而不是
/// 会话的属性——按会话记会让多家提供方互相污染，也无法回答"是哪家降的级"
fn probe_key(ctx: &Arc<Ctx>) -> String {
    ctx.ai_config
        .lock()
        .unwrap()
        .active()
        .map(|p| p.id.clone())
        .unwrap_or_default()
}

/// 原生探测失败（端点明确拒绝 tools 参数）时的降级决策：
/// ① 开关判断——只有该提供方「文本协议降级」开关打开才允许自动降级（默认关）；
/// ② 按提供方记忆探测结果（后续会话不再重复探测）；
/// ③ 审计如实记录是哪家提供方发生了降级。
/// 返回 Err = 开关未开：直接报错告知用户是哪家端点、去哪里开，不做静默降级
fn handle_unsupported(ctx: &Arc<Ctx>, err: &str) -> Result<(), String> {
    let provider = ctx.ai_config.lock().unwrap().active().cloned();
    let Some(p) = provider else {
        return Err("未配置任何 AI 提供方".into());
    };
    if !p.text_fallback {
        return Err(format!(
            "提供方「{}」不支持原生工具调用（{}）。如确认该端点仅支持文本约定，可在「AI 设置」中开启它的「文本协议降级」开关后重试",
            p.name,
            ai::user_err(err)
        ));
    }
    ctx.native_probe.lock().unwrap().insert(p.id.clone(), false);
    crate::audit::record(
        ctx,
        "system",
        "ai.native.degrade",
        &p.name,
        json!({ "provider_id": p.id, "error": err }),
        true,
    );
    Ok(())
}

/// 工具审批：弹出询问卡片等待用户应答（120 秒超时自动拒绝）。
/// 是否需要询问由 auto_pass() 在调用方判定，这里只负责"问"。
/// 等待期间每 500ms 轮询一次会话中断标志：用户点「停止」可立即取消审批中的工具
/// 应答渠道：本地 UI（tool-approval 事件）或远程 POST /api/approvals/{id}
pub(crate) async fn request_approval(
    ctx: &Arc<Ctx>,
    tool: &str,
    params: &serde_json::Value,
    session_id: Option<&str>,
    actor: &str,
) -> Result<(), String> {
    use tauri::Emitter;
    let id = format!("ap-{}", ctx.approval_seq.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    ctx.approvals.lock().unwrap().insert(
        id.clone(),
        crate::state::PendingApproval {
            tx,
            tool: tool.to_string(),
            params: params.clone(),
            created: std::time::Instant::now(),
        },
    );
    let _ = ctx.app.emit(
        "tool-approval",
        json!({ "id": id, "tool": tool, "params": params }),
    );
    crate::audit::record(
        ctx,
        actor,
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
pub(crate) fn auto_pass(mode: &str, tool: &str) -> bool {
    mode == "allow_all" || (mode == "auto" && is_safe_tool(tool))
}

fn is_safe_tool(tool: &str) -> bool {
    const SAFE: &[&str] = &[
        "add_memory",
        "add_skill",
        "plan_update",
        "write_plugin",
        "write_tool",
    ];
    if SAFE.iter().any(|s| s.eq_ignore_ascii_case(tool)) {
        return true;
    }
    // 只读类判定必须锚定词首（starts_with）：contains 会把 "bread_maker"、"already_exec"
    // 之类名字误判为安全工具，AI 给自建工具起个含关键词的名字即可绕过审批执行任意代码
    let lower = tool.to_lowercase();
    lower == "read"
        || lower == "list"
        || lower == "search"
        || lower == "get"
        || lower == "view"
        || lower == "query"
        || ["read_", "list_", "search_", "get_", "view_", "query_"]
            .iter()
            .any(|k| lower.starts_with(k))
}

/// 执行 AI 输出的单个工具调用（含 AI 自写插件能力）。
/// session_id 用于审批等待期间响应会话中断；外部调用（远程/自动驾驶）传 None
pub async fn execute_tool_call(
    ctx: &Arc<Ctx>,
    name: &str,
    params: &serde_json::Value,
    session_id: Option<&str>,
) -> Result<serde_json::Value, String> {
    // 模型偶尔输出带空白的工具名（" shell "）：去空白再匹配
    let name = name.trim();
    if !auto_pass(&ctx.config.lock().unwrap().tool_approval.clone(), name) {
        request_approval(ctx, name, params, session_id, "ai-self").await?;
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
        // ---- 已注册工具（内置 / 远程 / AI 自写脚本插件） ----
        other => {
            let tool = {
                let tools = ctx.tools.lock().unwrap();
                match tools.iter().find(|t| t.name.eq_ignore_ascii_case(other)).cloned() {
                    Some(t) => t,
                    None => {
                        // 未知工具 → 不报错，而是返回可用工具列表让模型自动纠正
                        let available: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
                        return Ok(json!({
                            "error": format!("Unknown tool '{other}'. Available: {}", available.join(", ")),
                            "hint": "Call one of the available tools above instead."
                        }));
                    }
                }
            };
            crate::registry::invoke(ctx, &tool.id, params.clone(), "ai-self", session_id).await
        }
    }
}

/// 幻觉防护备注：工具轮熔断 / 单词异常重复时在回复尾部追加标记行。
/// 标记同时作为 auto-drive 的暂停信号（幻觉循环里继续自驱只会空烧 token）。
fn guard_suffix(ctx: &Arc<Ctx>, visible: &str, tool_capped: bool, tool_rounds: usize, loop_max: u32) -> String {
    let mut out = visible.to_string();
    if tool_capped {
        let note = format!("[tool-loop-guard] stopped after {tool_rounds} tool rounds (limit {loop_max}).");
        if out.trim().is_empty() {
            out = note;
        } else {
            out.push_str("\n\n");
            out.push_str(&note);
        }
    }
    let wmax = ctx.config.lock().unwrap().word_repeat_max;
    if let Some((w, n)) = crate::repetition::find_repeat(&out, wmax) {
        out.push_str(&format!(
            "\n\n[repetition-guard] word \"{w}\" repeated {n} times (limit {wmax}); auto-continue paused."
        ));
    }
    out
}

/// 目标自动推进判定：回合结束后调用。返回 Some(下一回合合成消息) 继续跑，None 停止。
/// 停止条件：AI 回复以 [WAIT] 开头（请求用户决策/输入）、连续两轮回复完全一致（空转）、
/// 本会话没有 active 目标、或该目标自动推进已达 AUTO_DRIVE_MAX 轮。
fn auto_drive_next(ctx: &Arc<Ctx>, sid: &str, reply: &str, last_reply: &str) -> Option<String> {
    // 幻觉防护命中（工具死循环 / 词重复刷屏）：停止自驱，等用户介入
    if reply.contains("[tool-loop-guard]") || reply.contains("[repetition-guard]") {
        return None;
    }
    if reply.trim_start().starts_with("[WAIT]") {
        return None;
    }
    if !last_reply.is_empty() && last_reply == reply {
        return None;
    }
    // 本会话的 active 目标（用户手动建的全局目标 session_id=None 不自动推进）
    let goals = ctx.goals.lock().unwrap();
    let todos = ctx.todos.lock().unwrap();
    let goal = goals
        .iter()
        .find(|g| g.status == "active" && g.session_id.as_deref() == Some(sid))?;
    let gid = goal.id.clone();
    let title = goal.title.clone();
    let all: Vec<crate::goal::Todo> = todos
        .iter()
        .filter(|t| t.goal_id.as_deref() == Some(gid.as_str()))
        .cloned()
        .collect();
    drop(todos);
    drop(goals);
    let pending: Vec<&crate::goal::Todo> = all.iter().filter(|t| t.status != "completed").collect();
    // 安全上限：同一目标最多 AUTO_DRIVE_MAX 轮自动推进
    let mut counts = ctx.auto_drive_counts.lock().unwrap();
    let n = counts.entry(gid).or_insert(0);
    *n += 1;
    if *n > crate::config::AUTO_DRIVE_MAX {
        return None;
    }
    drop(counts);
    let done = all.len() - pending.len();
    let next_step = match pending.first() {
        Some(t) => format!("下一步：「{}」。请执行这一步。", t.content),
        // 待办全部完成但目标还挂着：让 AI 收尾（标记 achieved 并总结）
        None => "所有待办均已完成。请调用 plan_update(goal_status=achieved) 将本目标标记为 achieved，并简要总结成果。".to_string(),
    };
    Some(format!(
        "继续（自动推进）：目标「{title}」尚未完成（待办 {done}/{}）。{next_step}全部完成后调用 plan_update(goal_status=achieved) 将目标标记为 achieved。若必须等用户决策或输入才能继续，回复以 [WAIT] 开头并说明需要什么。",
        all.len()
    ))
}

/// 带目标自动推进的对话回合（非流式）：回合成功结束后，本会话存在 active 目标且未完成时
/// 自动把规划的下一步作为新回合发给 AI，直到目标完成 / [WAIT] / 空转 / 达到上限。
pub async fn chat_turn_auto(
    ctx: &Arc<Ctx>,
    session_id: &str,
    user_input: &str,
    images: Vec<String>,
) -> Result<Vec<ChatMessage>, String> {
    if !ctx.config.lock().unwrap().auto_drive {
        return chat_turn(ctx, session_id, user_input, images).await;
    }
    let mut msg = user_input.to_string();
    let mut imgs = images;
    let mut last_reply = String::new();
    loop {
        let msgs = chat_turn(ctx, session_id, &msg, std::mem::take(&mut imgs)).await?;
        let reply = msgs
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        match auto_drive_next(ctx, session_id, &reply, &last_reply) {
            Some(next) => {
                last_reply = reply;
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                msg = next;
            }
            None => return Ok(msgs),
        }
    }
}

/// 带目标自动推进的对话回合（流式）：每个自动续跑的回合独立走完整流式事件，
/// 前端通过每回合的 final 事件刷新消息列表，合成用户消息照常落库展示。
pub async fn chat_turn_stream_auto(
    ctx: &Arc<Ctx>,
    session_id: &str,
    user_input: &str,
    event_name: &str,
    images: Vec<String>,
) -> Result<Vec<ChatMessage>, String> {
    if !ctx.config.lock().unwrap().auto_drive {
        return chat_turn_stream(ctx, session_id, user_input, event_name, images).await;
    }
    let mut msg = user_input.to_string();
    let mut imgs = images;
    let mut last_reply = String::new();
    loop {
        let msgs = chat_turn_stream(ctx, session_id, &msg, event_name, std::mem::take(&mut imgs)).await?;
        let reply = msgs
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        match auto_drive_next(ctx, session_id, &reply, &last_reply) {
            Some(next) => {
                last_reply = reply;
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                msg = next;
            }
            None => return Ok(msgs),
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
    // 0) 同会话回合互斥：先抢锁后写历史，被拒绝的并发请求不落任何消息
    let target = if session_id.is_empty() {
        ctx.sessions.lock().unwrap().active.clone()
    } else {
        session_id.to_string()
    };
    let _turn = acquire_turn(ctx, &target)?;
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

    let iflag = register_interrupt(ctx, &target);

    // 原生工具调用探测：按提供方只探测一次（内存缓存不持久化；改配置/切开关即失效重探）；
    // 探测过不支持的提供方直接用文本约定提示词
    let probe_key = probe_key(ctx);
    let mut native_mode = ctx.native_probe.lock().unwrap().get(&probe_key).copied() != Some(false);

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
    let mut continues = 0usize; // 自动续发（截断重试）计数：整个回合最多 3 次
    // 瞬态网络错误（流被截断/连接失败/超时）自动重试本轮的剩余次数
    let mut net_retries = 2usize;
    // 幻觉防护：本回合已执行的「工具调用轮」计数（tool_loop_max 熔断）
    let mut tool_rounds = 0usize;
    loop {
        round += 1;
        if interrupted(ctx, &target) {
            clear_interrupt(ctx, &target, &iflag);
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
                clear_interrupt(ctx, &target, &iflag);
                return Err("对话已中断".into());
            }
            match attempt {
                Ok(r) => {
                    ctx.native_probe.lock().unwrap().insert(probe_key.clone(), true);
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
                Err(ai::NativeErr::Unsupported(e)) => {
                    // 端点拒绝 tools 参数：是否允许自动降级由该提供方的「文本协议降级」开关决定
                    // （默认关：报错并告知是哪家、去哪里开），降级发生时审计记录
                    handle_unsupported(ctx, &e)?;
                    ctx.native_probe.lock().unwrap().insert(probe_key.clone(), false);
                    native_mode = false;
                    native_exchanges.clear();
                    convo[0] = ChatMessage::system(ai::system_prompt(ctx, Some(&target)));
                    round -= 1; // 重走本轮（保留第一轮携带图片的语义）
                    continue;
                }
                // 瞬态网络错误（流截断/连接失败）：自动重走本轮，业务错误不重试
                Err(ai::NativeErr::Other(ref e)) if ai::is_transient_net_error(e) && net_retries > 0 => {
                    net_retries -= 1;
                    round -= 1;
                    tokio::time::sleep(std::time::Duration::from_millis(600 * (2 - net_retries) as u64)).await;
                    continue;
                }
                Err(ai::NativeErr::Other(e)) => return Err(ai::user_err(&e)),
            }
        } else {
            // 文本协议统一走 SSE 流式（与桌面端一致）：非流式请求会被仅支持流式的端点拒绝
            // 思考过程增量（reasoning/thinking）累积进 round_thinking，随消息落库
            let think_buf = round_thinking.clone();
            let stream = ai::chat_stream_with_images(ctx, &convo, round_images, move |kind, tok| {
                if kind == ai::TokenKind::Think {
                    think_buf.lock().unwrap().push_str(tok);
                }
                true
            })
            .await;
            let (reply, usage) = match stream {
                Ok(r) => r,
                // 瞬态网络错误（流截断/连接失败）：自动重走本轮，业务错误不重试
                Err(ref e) if ai::is_transient_net_error(e) && net_retries > 0 => {
                    net_retries -= 1;
                    round -= 1;
                    tokio::time::sleep(std::time::Duration::from_millis(600 * (2 - net_retries) as u64)).await;
                    continue;
                }
                Err(e) => return Err(ai::user_err(&e)),
            };
            // 记录本轮用量并推送缓存命中率统计
            let payload = record_and_payload(ctx, &target, &usage);
            let _ = ctx.app.emit("chat-usage", json!({ "session": target, "usage": payload }));
            let calls = match parse_tool_calls(&reply) {
                Some(tc) if !tc.is_empty() && looks_like_tool_calls(&tc) => text_calls_to_native(&tc),
                _ => Vec::new(),
            };
            (reply, calls)
        };

        // 幻觉防护：工具调用轮达到配置上限时不再执行（回合走收尾，附熔断说明，0=不设限）
        let loop_max = ctx.config.lock().unwrap().tool_loop_max;
        let tool_capped = loop_max > 0 && tool_rounds >= loop_max as usize;
        if !native_calls.is_empty() && !tool_capped {
            tool_rounds += 1;
            // 全量执行全部工具调用（不静默丢弃超限调用），并发上限 16，结果仍按调用顺序回喂
            let calls: Vec<ai::NativeToolCall> = native_calls.clone();

            let outcomes: Vec<Result<serde_json::Value, String>> = futures_util::stream::iter(calls.iter().cloned())
                .map(|call| {
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
                })
                .buffered(16)
                .collect()
                .await;
            if interrupted(ctx, &target) {
                clear_interrupt(ctx, &target, &iflag);
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

        // 回复不完整（截断/纯思考残渣）：不结束回合，自动替用户补发「继续」。
        // 与下方原生分支同理：此处到达时本轮未发起工具调用，补发协议合法。
        // 续发上限 3 次：模型反复输出不完整回复时（如永远撑爆 max_tokens 的超大 JSON），
        // 无限续发会无限烧 token——超限后落库可见部分并按普通回复结束回合
        if looks_truncated(&reply) && continues < 3 {
            continues += 1;
            // 自动接续时去掉「可回复继续」标注：程序替用户续发，提示已无意义
            let visible = strip_tool_json(&strip_think_blocks(&reply))
                .replace(crate::ai::TRUNCATION_NOTICE, "")
                .trim()
                .to_string();
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
            // 空回复（纯思考残渣）不压入上下文：部分端点（如 DeepSeek）拒收空 content
            // 的 assistant 消息，会让续发轮直接 400 断线
            if !reply.trim().is_empty() {
                convo.push(ChatMessage::assistant(reply.clone()));
            }
            convo.push(ChatMessage::user(CONTINUE_PROMPT));
            continue;
        }
        // 纯文本回复：存入会话并结束（同时去掉思考块残渣）
        let visible = strip_tool_json(&strip_think_blocks(&reply));
        let visible = if visible.is_empty() { reply.clone() } else { visible };
        // 幻觉防护备注：工具熔断 / 词重复（标记同时作为 auto-drive 的暂停信号）
        let visible = guard_suffix(ctx, &visible, tool_capped, tool_rounds, loop_max);
        {
            // 思考过程随最终回复落库（与流式路径一致，否则 TUI/远程对话重启后思考丢失）
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
        clear_interrupt(ctx, &target, &iflag);
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

    // 0) 同会话回合互斥：先抢锁后写历史，被拒绝的并发请求不落任何消息；
    // 中断标志注册到映射，各检查点查询、停止后按指针比对摘除，不留脏标志影响下一回合
    let target = if session_id.is_empty() {
        ctx.sessions.lock().unwrap().active.clone()
    } else {
        session_id.to_string()
    };
    let _turn = acquire_turn(ctx, &target)?;
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

    let iflag = register_interrupt(ctx, &target);

    // 原生工具调用探测：按提供方只探测一次（内存缓存不持久化；改配置/切开关即失效重探）；
    // 提示词随探测结果切换：原生模式不教文本调用格式，避免两种约定互相干扰
    let probe_key = probe_key(ctx);
    let mut native_mode = ctx.native_probe.lock().unwrap().get(&probe_key).copied() != Some(false);

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
    let mut continues = 0usize; // 自动续发（截断重试）计数：整个回合最多 3 次
    // 瞬态网络错误（流被截断/连接失败/超时）自动重试本轮的剩余次数
    let mut net_retries = 2usize;
    // 幻觉防护：本回合已执行的「工具调用轮」计数（tool_loop_max 熔断）
    let mut tool_rounds = 0usize;
    loop {
        round += 1;
        if interrupted(ctx, &target) {
            clear_interrupt(ctx, &target, &iflag);
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
                clear_interrupt(ctx, &target, &iflag);
                let e = "对话已中断".to_string();
                emit(json!({ "type": "error", "error": e.clone(), "interrupted": true }));
                return Err(e);
            }
            match attempt {
                Ok(r) => {
                    ctx.native_probe.lock().unwrap().insert(probe_key.clone(), true);
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
                Err(ai::NativeErr::Unsupported(e)) => {
                    // 端点拒绝 tools 参数：是否允许自动降级由该提供方的「文本协议降级」开关决定
                    handle_unsupported(ctx, &e)?;
                    ctx.native_probe.lock().unwrap().insert(probe_key.clone(), false);
                    native_mode = false;
                    native_exchanges.clear();
                    convo[0] = ChatMessage::system(ai::system_prompt(ctx, Some(&target)));
                    round -= 1;
                    continue;
                }
                // 瞬态网络错误（流截断/连接失败）：自动重走本轮；本轮已流出的半截内容由前端清空
                Err(ai::NativeErr::Other(ref e)) if ai::is_transient_net_error(e) && net_retries > 0 => {
                    net_retries -= 1;
                    round -= 1;
                    emit(json!({ "type": "net_retry", "attempt": 2 - net_retries, "error": ai::user_err(e) }));
                    tokio::time::sleep(std::time::Duration::from_millis(600 * (2 - net_retries) as u64)).await;
                    continue;
                }
                Err(ai::NativeErr::Other(e)) => {
                    clear_interrupt(ctx, &target, &iflag);
                    let e = ai::user_err(&e);
                    emit(json!({ "type": "error", "error": e.clone() }));
                    return Err(e);
                }
            }
        } else {
            // 流式过程中若收到中断请求：回调返回 false 立即断开 SSE 读取（不再等流自然结束）。
            // 直接查 interrupts 映射而非开局捕获的 Arc：同会话新回合会替换映射条目，
            // 旧回合若还持有旧 Arc 就永远收不到置位（僵尸流持续推事件、无法停止）
            let stream_cancelled = Arc::new(AtomicBool::new(false));
            // 服务器侧同步累积已流出的正文：中断/上游断流时把部分回复落库，避免内容"消失"
            let text_buf = Arc::new(std::sync::Mutex::new(String::new()));
            let result = {
                let emit_ref = &emit;
                let sc = stream_cancelled.clone();
                let icheck_ctx = ctx.clone();
                let icheck_target = target.clone();
                let think_buf = round_thinking.clone();
                let text_acc = text_buf.clone();
                ai::chat_stream_with_images(ctx, &convo, round_images, move |kind, tok| {
                    if interrupted(&icheck_ctx, &icheck_target) {
                        sc.store(true, Ordering::Relaxed);
                        return false;
                    }
                    match kind {
                        // 思考过程增量：实时推给前端展示 + 累积缓冲（结束后随消息落库）
                        ai::TokenKind::Think => {
                            think_buf.lock().unwrap().push_str(tok);
                            emit_ref(json!({ "type": "think", "text": tok }));
                        }
                        ai::TokenKind::Text => {
                            text_acc.lock().unwrap().push_str(tok);
                            emit_ref(json!({ "type": "delta", "text": tok }));
                        }
                    }
                    true
                })
                .await
            };
            if interrupted(ctx, &target) || stream_cancelled.load(Ordering::Relaxed) {
                clear_interrupt(ctx, &target, &iflag);
                // 中断时已流出的部分回复同样落库：用户点停止前的半截答案保留在历史里
                let _ = persist_partial(ctx, &target, text_buf.lock().unwrap().clone(), &round_thinking);
                let e = "对话已中断".to_string();
                emit(json!({ "type": "error", "error": e.clone(), "interrupted": true }));
                return Err(e);
            }
            let (reply, round_usage) = match result {
                Ok(r) => r,
                // 瞬态网络错误（流截断/连接失败）：自动重走本轮，本轮半截内容直接丢弃
                // （历史未落库、前端在 net_retry 事件里清空实时区），业务错误不重试
                Err(ref e) if ai::is_transient_net_error(e) && net_retries > 0 => {
                    net_retries -= 1;
                    round -= 1;
                    emit(json!({ "type": "net_retry", "attempt": 2 - net_retries, "error": ai::user_err(e) }));
                    tokio::time::sleep(std::time::Duration::from_millis(600 * (2 - net_retries) as u64)).await;
                    continue;
                }
                Err(e) => {
                    // 上游中途断流：部分回复落库并随 error 事件带回，前端保住已显示的内容
                    let visible = persist_partial(ctx, &target, text_buf.lock().unwrap().clone(), &round_thinking);
                    clear_interrupt(ctx, &target, &iflag);
                    let e = ai::user_err(&e);
                    let mut ev = json!({ "type": "error", "error": e.clone() });
                    if !visible.is_empty() {
                        ev["partial"] = json!(visible);
                    }
                    emit(ev);
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

        // 幻觉防护：工具调用轮达到配置上限时不再执行（回合走收尾，附熔断说明，0=不设限）
        let loop_max = ctx.config.lock().unwrap().tool_loop_max;
        let tool_capped = loop_max > 0 && tool_rounds >= loop_max as usize;
        if !native_calls.is_empty() && !tool_capped {
            tool_rounds += 1;
            // 全量执行全部工具调用（不静默丢弃超限调用），并发上限 16，结果仍按调用顺序回喂
            let calls: Vec<ai::NativeToolCall> = native_calls.clone();
            // 通知前端：本轮有 N 个工具要执行 → 显示灰色 spinner 占位
            let pending_list: Vec<serde_json::Value> = calls
                .iter()
                .map(|c| json!({ "tool": c.name, "summary": tool_summary(&c.name, &c.args) }))
                .collect();
            emit(json!({ "type": "round_tools_starting", "count": pending_list.len(), "pending": pending_list }));

            let outcomes: Vec<Result<serde_json::Value, String>> = futures_util::stream::iter(calls.iter().cloned())
                .map(|call| {
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
                })
                .buffered(16)
                .collect()
                .await;
            if interrupted(ctx, &target) {
                clear_interrupt(ctx, &target, &iflag);
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

        // 回复不完整（截断/纯思考残渣）：不结束回合，自动补发「继续」。
        // 文本约定模式与原生 function calling 模式都启用：原生模式到达此分支
        // 意味着本轮没有发起工具调用（有调用的分支在上面已 continue），
        // 此时上下文末尾是普通 assistant/user 消息，补发「继续」协议合法。
        // 续发上限 3 次：模型反复截断时超限落库可见部分并结束回合，避免无限烧 token
        if looks_truncated(&reply) && continues < 3 {
            continues += 1;
            // 自动接续时去掉「可回复继续」标注：程序替用户续发，提示已无意义
            let visible = strip_tool_json(&strip_think_blocks(&reply))
                .replace(crate::ai::TRUNCATION_NOTICE, "")
                .trim()
                .to_string();
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
            // 空回复不压入上下文（DeepSeek 等端点拒收空 content 的 assistant 消息）
            if !reply.trim().is_empty() {
                convo.push(ChatMessage::assistant(reply.clone()));
            }
            convo.push(ChatMessage::user(CONTINUE_PROMPT));
            continue;
        }
        // 纯文本回复：存入会话并结束（同时去掉思考块残渣）
        let visible = strip_tool_json(&strip_think_blocks(&reply));
        let visible = if visible.is_empty() { reply.clone() } else { visible };
        // 幻觉防护备注：工具熔断 / 词重复（标记同时作为 auto-drive 的暂停信号）
        let visible = guard_suffix(ctx, &visible, tool_capped, tool_rounds, loop_max);
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
        clear_interrupt(ctx, &target, &iflag);
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
    // finish_reason=length 的显式截断标注（ai 层追加）：最可靠信号。
    // 截断常落在句号等"看起来完整"的位置，其他启发式全部漏判——官方 API
    // （DeepSeek/千问）老老实实返回 length，此前就因漏判这里而要用户手动发「继续」
    if t.contains(crate::ai::TRUNCATION_NOTICE) {
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
const CONTINUE_PROMPT: &str = "继续（你上一条回复未输出完整就被截断了，从中断处直接往下写，不要重复已输出的内容；若要调用工具请按协议单独一行输出 JSON 数组；如需写入大文件，请拆成多次较小的写入避免单次输出过长；若已完成请直接给出最终答案）";

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

/// 归一化一条文本协议工具调用（兼容模型跑偏的输出形态）：
/// ① 协议标准 {"tool":"name","params":{...}}；② OpenAI 风格 {"name":..,"arguments":..}
/// （arguments 可为内嵌 JSON 字符串或对象）；③ 嵌套 {"function":{"name","arguments"}}；
/// ④ params 缺失/null 补 {}；⑤ params 写成字符串时尝试按 JSON 解析，失败包成 {"input":..}；
/// ⑥ 工具名去首尾空白。仅凭 "name" 字段不认（散文里的 {"name":"John"} 会误判），
/// name 风格必须伴随 arguments/params/function 字段
fn normalize_tool_call(v: &serde_json::Value) -> Option<(String, serde_json::Value)> {
    if v.get("tool").is_none()
        && v.get("arguments").is_none()
        && v.get("function").is_none()
        && v.get("params").is_none()
    {
        return None;
    }
    let name = v
        .get("tool")
        .or_else(|| v.get("name"))
        .or_else(|| v.pointer("/function/name"))
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    let mut params = v
        .get("params")
        .or_else(|| v.get("arguments"))
        .or_else(|| v.pointer("/function/arguments"))
        .cloned()
        .unwrap_or(serde_json::json!({}));
    if params.is_null() {
        params = serde_json::json!({});
    }
    if let Some(s) = params.as_str() {
        params = serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .filter(|p| p.is_object())
            .unwrap_or(serde_json::json!({ "input": s }));
    }
    Some((name, params))
}

fn normalize_calls(calls: &[serde_json::Value]) -> Vec<serde_json::Value> {
    calls
        .iter()
        .filter_map(|c| {
            normalize_tool_call(c).map(|(name, params)| serde_json::json!({ "tool": name, "params": params }))
        })
        .collect()
}

/// 把模型跑偏的 JSON「修」回可解析形态（仅在标准解析失败时作兜底重试）：
/// ① 智能引号 “ ” ‘ ’ → ASCII 引号；② 结构位置的全角 ： ， → 半角；③ ] } 前的尾逗号删除。
/// 字符串内部的字符一律不动（避免破坏命令/文件内容语义）。修不出平衡结构返回 None
fn jsonish_repair(reply: &str) -> Option<String> {
    let chars: Vec<char> = reply.chars().collect();
    let mut out = String::with_capacity(reply.len());
    let mut str_open: Option<char> = None; // 开引号（'"' 为 ASCII 串，转义感知）
    let mut esc = false;
    let mut depth = 0i32;
    for (idx, &c) in chars.iter().enumerate() {
        if let Some(open) = str_open {
            if open == '"' {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    str_open = None;
                }
                out.push(c);
                continue;
            }
            // 智能引号字符串：遇匹配闭引号或 ASCII 引号收束，闭引号统一改写为 "
            let closes = matches!(
                (open, c),
                ('“', '”') | ('“', '"') | ('‘', '’') | ('‘', '\'')
            );
            if closes {
                str_open = None;
                out.push('"');
            } else {
                out.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                str_open = Some('"');
                out.push('"');
            }
            '“' | '”' => {
                str_open = Some('“');
                out.push('"');
            }
            '\'' => {
                str_open = Some('\'');
                out.push('\'');
            }
            '‘' | '’' => {
                str_open = Some('‘');
                out.push('\'');
            }
            '：' => out.push(':'),
            '，' | ',' => {
                // 尾逗号：结构位置上后面（跳过空白）紧跟 ] 或 } 则删除
                let next = chars[idx + 1..].iter().find(|x| !x.is_whitespace());
                if !matches!(next, Some(']') | Some('}')) {
                    out.push(',');
                }
            }
            '{' | '[' => {
                depth += 1;
                out.push(c);
            }
            '}' | ']' => {
                depth -= 1;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    // 修不出平衡结构（字符串未闭合 / 括号失衡）：交还上层按截断等路径处理
    if str_open.is_some() || depth != 0 {
        return None;
    }
    if out == reply {
        None
    } else {
        Some(out)
    }
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
            if normalize_tool_call(&v).is_some() {
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
/// 兼容模型散落的单个 {"tool":...} 对象（可带 <xxx_function_call> 自创标记）；
/// 标准解析失败时用 jsonish_repair 兜底重试（智能引号/全角标点/尾逗号）；
/// 全部结果经 normalize_tool_call 归一化为 {"tool","params"}
fn parse_tool_calls(reply: &str) -> Option<Vec<serde_json::Value>> {
    for text in std::iter::once(reply).chain(jsonish_repair(reply).iter().map(|s| s.as_str())) {
        if let Some(calls) = crate::autopilot::parse_json_array(text) {
            let norm = normalize_calls(&calls);
            if !norm.is_empty() {
                return Some(norm);
            }
        }
        let objs = find_tool_objects(text);
        if !objs.is_empty() {
            let vals: Vec<serde_json::Value> = objs.into_iter().map(|(v, _, _)| v).collect();
            let norm = normalize_calls(&vals);
            if !norm.is_empty() {
                return Some(norm);
            }
        }
    }
    None
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

    /// ai.rs 原生调用桥接（OpenAI tool_calls / Claude tool_use / Gemini functionCall
    /// → 文本协议 JSON 行）的产物必须能被解析端接住执行：
    /// 正文与调用行分离、键序按 serde_json 字母序（params 在前）、纯调用行无正文
    #[test]
    fn test_parse_bridged_native_call_line() {
        // 正文 + 桥接行（DeepSeek/千问 "宣告了工具却没下文" 场景的修复产物）
        let reply = "让我们来查看：\n[{\"params\":{\"command\":\"echo hi\"},\"tool\":\"shell\"}]";
        let calls = parse_tool_calls(reply).unwrap();
        assert!(looks_like_tool_calls(&calls), "桥接行应识别为工具调用");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["tool"], "shell");
        assert_eq!(calls[0]["params"]["command"], "echo hi");
        // 用户可见正文不残留 JSON
        assert_eq!(strip_tool_json(reply), "让我们来查看：");
        // 纯 functionCall 无正文的桥接产物（Gemini）
        let solo = "[{\"params\":{\"command\":\"echo solo\"},\"tool\":\"shell\"}]";
        let calls = parse_tool_calls(solo).unwrap();
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

    /// 只读类关键词判定必须锚定词首：含关键词子串的恶意命名不得绕过审批
    #[test]
    fn test_safe_tool_keyword_prefix_only() {
        // 词首命中：仍然放行（保持原有 UX）
        assert!(auto_pass("auto", "read_file"));
        assert!(auto_pass("auto", "list_tools"));
        assert!(auto_pass("auto", "get_weather"));
        assert!(auto_pass("auto", "query_db"));
        assert!(auto_pass("auto", "search_web"));
        assert!(auto_pass("auto", "view_image"));
        // 裸名：仍然放行
        assert!(auto_pass("auto", "read"));
        assert!(auto_pass("auto", "query"));
        // 关键词出现在词中/词尾：必须询问（此前 contains 会误放行）
        assert!(!auto_pass("auto", "bread_maker"));
        assert!(!auto_pass("auto", "already_exec"));
        assert!(!auto_pass("auto", "readable_stats"));
        assert!(!auto_pass("auto", "thread_viewer_kill"));
        assert!(!auto_pass("auto", "budget_get_shell"));
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

    /// OpenAI 风格 name/arguments（arguments 为内嵌 JSON 字符串）——部分模型文本模式跑偏常见形态
    #[test]
    fn test_parse_openai_function_style() {
        let reply = r#"我来执行：{"name": "shell", "arguments": "{\"command\": \"echo hi\"}"}"#;
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["tool"], "shell");
        assert_eq!(calls[0]["params"]["command"], "echo hi");
    }

    /// 嵌套 function 对象风格
    #[test]
    fn test_parse_nested_function_style() {
        let reply = r#"{"function": {"name": "shell", "arguments": {"command": "dir"}}}"#;
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls[0]["tool"], "shell");
        assert_eq!(calls[0]["params"]["command"], "dir");
    }

    /// 裸 {"name":...}（无 arguments/params/function 字段）不得误判为工具调用（散文数据对象）
    #[test]
    fn test_reject_bare_name_object() {
        assert!(parse_tool_calls(r#"用户是 {"name": "John", "age": 30}，请记住"#).is_none());
    }

    /// 智能引号 + 全角冒号：jsonish_repair 兜底（标准解析失败后重试）
    #[test]
    fn test_parse_smart_quotes_and_fullwidth() {
        let reply = "[{“tool”：“shell”, “params”：{“command”：“echo smart-ok”}}]";
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls[0]["tool"], "shell");
        assert_eq!(calls[0]["params"]["command"], "echo smart-ok");
    }

    /// 尾逗号（对象内部）：find_tool_objects 的 serde 解析失败 → repair 兜底
    #[test]
    fn test_parse_trailing_comma() {
        let reply = r#"[{"tool":"shell","params":{"command":"echo a",}}]"#;
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls[0]["params"]["command"], "echo a");
    }

    /// params 写成字符串：尝试按 JSON 解析为对象
    #[test]
    fn test_parse_params_as_string() {
        let reply = r#"[{"tool":"shell","params":"{\"command\":\"echo x\"}"}]"#;
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls[0]["params"]["command"], "echo x");
    }

    /// 工具名带空白：归一化去空白
    #[test]
    fn test_parse_tool_name_whitespace() {
        let reply = r#"[{"tool":" shell ","params":{"command":"echo ws"}}]"#;
        let calls = parse_tool_calls(reply).unwrap();
        assert_eq!(calls[0]["tool"], "shell");
    }

    /// 修复不动字符串内部的全角字符（命令/内容语义不被破坏）
    #[test]
    fn test_repair_keeps_string_content() {
        let reply = "{“tool”：“shell”，“params”：{“command”：“echo a：b”}}";
        let repaired = jsonish_repair(reply).unwrap();
        let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v.pointer("/params/command").and_then(|x| x.as_str()), Some("echo a：b"));
    }

    /// 修不出平衡结构（字符串未闭合）返回 None，交还上层按截断处理
    #[test]
    fn test_repair_rejects_unbalanced() {
        assert!(jsonish_repair("{“tool”: \"shell\"").is_none());
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

    /// finish_reason=length 的显式截断标注必须判为截断（官方 API 的
    /// length 截断常落在句号收尾处，其他启发式全部漏判 → 用户被迫手动发「继续」）
    #[test]
    fn test_truncation_notice_is_truncated() {
        // 句号收尾 + 标注：此前漏判的真实场景
        assert!(looks_truncated(&format!(
            "前半部分正常收尾。{}",
            crate::ai::TRUNCATION_NOTICE
        )));
        assert!(looks_truncated(&format!(
            "代码如下：\n```python\nprint(1)\n```{}",
            crate::ai::TRUNCATION_NOTICE
        )));
        // 干净回复（无标注）不得误判
        assert!(!looks_truncated("清理完成，共删除 12 个文件。"));
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
