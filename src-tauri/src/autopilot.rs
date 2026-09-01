use serde_json::json;
use std::sync::Arc;

use crate::ai::{self, ChatMessage};
use crate::state::Ctx;

/// Autopilot：由前端小圆片播放/暂停控制的自主循环。
/// 播放时周期性地：
/// 1. 【自主行动】围绕活跃目标/未完成待办，让 AI 自己调用工具、写代码、沉淀技能
/// 2. 【记忆总结】把积压的原始记忆压缩为总结记忆
/// 3. 【技能提炼】从总结记忆中提炼可复用技能（SKILL）
pub async fn run(ctx: Arc<Ctx>) {
    // 基础心跳 10 秒；自主行动更耗时/耗 token，按更长间隔触发
    let mut ticks: u64 = 0;
    const AUTONOMOUS_EVERY: u64 = 6; // 约 60 秒一次自主行动
    loop {
        let running = ctx.autopilot_running.load(std::sync::atomic::Ordering::SeqCst);
        if running {
            // 自主行动：仅在有活跃目标/未完成待办、且到达间隔时执行
            if ticks % AUTONOMOUS_EVERY == 0 {
                if let Err(e) = autonomous_step(&ctx).await {
                    crate::audit::record(&ctx, "autopilot", "autopilot.error", "autonomous", json!({"error": e}), false);
                }
            }
            // 记忆总结 / 技能提炼
            if let Err(e) = tick(&ctx).await {
                crate::audit::record(&ctx, "autopilot", "autopilot.error", "cycle", json!({"error": e}), false);
            }
        }
        ticks = ticks.wrapping_add(1);
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

/// 立即执行一次总结周期（供「立即总结」按钮远程/本地触发）
pub async fn tick_public(ctx: &Arc<Ctx>) -> Result<(), String> {
    tick(ctx).await
}

/// 自主行动：AI 围绕当前目标/待办，自己决定并调用工具（写代码 / 注册工具 / 沉淀技能 / 更新待办）。
/// 复用 agent::execute_tool_call 的全套能力，与用户在对话框里手动驱动完全一致，
/// 区别只是这里由播放开关自动触发、无需用户逐条发消息。
async fn autonomous_step(ctx: &Arc<Ctx>) -> Result<(), String> {
    // 无 AI 配置则跳过（避免空转报错刷屏）
    if !ctx.ai_config.lock().unwrap().is_configured() {
        return Ok(());
    }
    // 没有活跃目标也没有未完成待办时，不主动打扰
    let has_goal = ctx.goals.lock().unwrap().iter().any(|g| g.status == "active");
    let has_todo = ctx.todos.lock().unwrap().iter().any(|t| t.status != "completed");
    if !has_goal && !has_todo {
        return Ok(());
    }

    // 组织一轮自主提示：让 AI 挑一件当前最该推进的事去做
    let system = ai::system_prompt(ctx, None);
    let user = "现在处于 Autopilot 自主模式。请审视上面的目标与待办，挑选当前最该推进的【一件】事去做：\
        可以调用工具、用 run_script/write_tool 写代码、write_plugin 写插件、add_skill 沉淀技能、\
        todo_update 标记进度。若需要行动就【只输出】工具调用 JSON 数组；若当前无事可做或全部完成，\
        直接输出一句简短说明（不要输出 JSON）。";
    let messages = vec![
        ChatMessage::system(system),
        ChatMessage::user(user),
    ];

    let reply = ai::chat(ctx, &messages).await?;

    // 解析工具调用；不是工具调用就当作一句思考记录下来即可
    match parse_json_array(&reply) {
        Some(calls) if !calls.is_empty() && calls.iter().all(|c| c.get("tool").and_then(|v| v.as_str()).is_some()) => {
            let mut done = 0;
            for call in calls.iter().take(4) {
                let name = call.get("tool").and_then(|v| v.as_str()).unwrap_or_default();
                let params = call.get("params").cloned().unwrap_or(json!({}));
                if name.is_empty() {
                    continue;
                }
                match crate::agent::execute_tool_call(ctx, name, &params, None).await {
                    Ok(_) => done += 1,
                    Err(e) => {
                        crate::audit::record(ctx, "autopilot", "autonomous.tool_error", name, json!({ "error": e }), false);
                    }
                }
            }
            crate::audit::record(ctx, "autopilot", "autonomous.step", "act", json!({ "calls": done }), true);
        }
        _ => {
            // 纯文本：AI 认为当前无需行动，记为一条自主思考
            let note = reply.trim();
            if !note.is_empty() {
                crate::audit::record(ctx, "autopilot", "autonomous.idle", "think", json!({ "note": note.chars().take(200).collect::<String>() }), true);
            }
        }
    }
    Ok(())
}

async fn tick(ctx: &Arc<Ctx>) -> Result<(), String> {
    // 1. 记忆压缩：raw 记忆 >= 4 条时触发
    {
        let raws: Vec<crate::memory::Memory> = {
            let mem = ctx.memories.lock().unwrap();
            mem.iter().filter(|m| m.kind == "raw").cloned().collect()
        };
        if raws.len() >= 4 {
            let content = raws
                .iter()
                .map(|m| format!("- ({}) {}", m.source, m.content))
                .collect::<Vec<_>>()
                .join("\n");
            let prompt = format!(
                "以下是 BIT 最近积累的原始记忆，请压缩为一段不超过 120 字的中文总结，\
                 保留关键事实与偏好，直接输出总结正文，不要任何前后缀：\n{content}"
            );
            let summary = ai::chat(
                ctx,
                &[ChatMessage::user(prompt)],
            )
            .await?;
            let ids = raws.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
            let removed = crate::memory::compress_memories(ctx, &ids, &summary);
            crate::audit::record(
                ctx,
                "autopilot",
                "memory.summarize",
                "memories",
                json!({ "compressed": removed, "summary": summary }),
                true,
            );
        }
    }

    // 2. 技能提炼：存在总结记忆且技能数落后时触发
    {
        let summaries: Vec<String> = {
            let mem = ctx.memories.lock().unwrap();
            mem.iter().filter(|m| m.kind == "summary").map(|m| m.content.clone()).collect()
        };
        if !summaries.is_empty() {
            let prompt = format!(
                "根据以下总结记忆，提炼 0~3 条可复用的技能（SKILL）。\
                 输出 JSON 数组，元素为 {{\"name\":\"短名称\",\"summary\":\"一句话说明\"}}，无技能则输出 []：\n{}",
                summaries.join("\n")
            );
            let reply = ai::chat(
                ctx,
                &[ChatMessage::user(prompt)],
            )
            .await;
            if let Ok(reply) = reply {
                if let Some(skills) = parse_json_array(&reply) {
                    for s in skills.iter().take(3) {
                        if let (Some(name), Some(summary)) =
                            (s.get("name").and_then(|v| v.as_str()), s.get("summary").and_then(|v| v.as_str()))
                        {
                            if !name.is_empty() {
                                crate::memory::add_skill(ctx, name, summary, "autopilot");
                                crate::audit::record(
                                    ctx,
                                    "autopilot",
                                    "skill.extract",
                                    name,
                                    json!({ "summary": summary }),
                                    true,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// 从 AI 回复中提取 JSON 数组（容忍 ```json 代码块包裹）
pub fn parse_json_array(reply: &str) -> Option<Vec<serde_json::Value>> {
    let text = reply.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .map(|t| t.trim())
        .unwrap_or(text);
    let text = text.strip_suffix("```").map(|t| t.trim()).unwrap_or(text);

    let start = text.find('[')?;
    let end = text.rfind(']')?;
    serde_json::from_str(&text[start..=end]).ok()
}
