// yxpil · BIT
//! 子代理调度（宿主管控）。
//!
//! 模型侧看不到 sub_agent 工具、也无法自行派生（见 ai::is_host_only_tool）。
//! 本模块是唯一的宿主入口：
//! - `spawn`：显式派生（UI 按钮 / 远程 API / 命令行）
//! - `auto_step`：Autopilot 周期里的自动委派策略（受 config.auto_delegate 开关控制）
//!
//! 生命周期由宿主掌握：派生 → subagent-lifecycle 事件广播 → 结束回写待办状态。

use serde_json::json;
use std::sync::Arc;

use crate::state::Ctx;

/// 并行子代理数的可配置上限（config.subagent_max 的可调范围 1..=MAX）。
/// 子代理会完整跑一遍 agent 循环（多轮工具 + token），数字越大越烧配额。
/// 默认 3，用户在设置页可调。
pub const MAX_CFG_SUBAGENTS: usize = 8;

/// 显式派生一个子代理（宿主入口）。阻塞直到子代理跑完（内部上限 15 分钟），
/// 返回 sub_agent 工具的原始结果 { session_id, final_answer, truncated, note }。
pub async fn spawn(
    ctx: &Arc<Ctx>,
    parent: Option<&str>,
    task: &str,
    title: Option<&str>,
) -> Result<serde_json::Value, String> {
    let task = task.trim();
    if task.is_empty() {
        return Err("任务描述不能为空".into());
    }
    let (running, limit) = {
        let cfg = ctx.config.lock().unwrap();
        (
            crate::registry::subagent_depth(),
            cfg.subagent_max.clamp(1, MAX_CFG_SUBAGENTS as u32) as usize,
        )
    };
    if running >= limit {
        return Err(format!(
            "已有 {running} 个子代理在跑（并行上限 {limit}，可在设置里调整），等有位置再派生"
        ));
    }
    let mut params = json!({ "task": task });
    if let Some(t) = title.map(str::trim).filter(|s| !s.is_empty()) {
        params["title"] = json!(t);
    }
    crate::registry::invoke(ctx, "builtin.sub_agent", params, "host", parent).await
}

/// 自动委派策略：挑一条「活跃目标下、尚未开始」的待办派生子代理。
///
/// 保守边界（防止失控）：
/// 1. 只在 config.auto_delegate 打开时生效；
/// 2. 并行在跑数不超过 config.subagent_max（1..=MAX_CFG_SUBAGENTS，设置页可调）；
/// 3. 派出的待办立刻置 in_progress，下一周期不会重复派生同一条；
/// 4. 子代理在后台跑，不阻塞 Autopilot 心跳；结束按成败回写待办状态。
///
/// 返回被派出的待办 id（None = 本次不派生）。
pub fn auto_step(ctx: &Arc<Ctx>) -> Option<String> {
    let limit = {
        let cfg = ctx.config.lock().unwrap();
        if !cfg.auto_delegate {
            return None;
        }
        cfg.subagent_max.clamp(1, MAX_CFG_SUBAGENTS as u32) as usize
    };
    let running = crate::registry::subagent_depth();
    if running >= limit {
        return None;
    }

    // 挑任务：活跃目标下最早的 pending 待办（无目标的独立待办不自动外派，避免误伤琐事）
    let picked = {
        let goals = ctx.goals.lock().unwrap();
        let active: std::collections::HashSet<&str> = goals
            .iter()
            .filter(|g| g.status == "active")
            .map(|g| g.id.as_str())
            .collect();
        let todos = ctx.todos.lock().unwrap();
        todos
            .iter()
            .find(|t| {
                t.status == "pending"
                    && t.goal_id
                        .as_deref()
                        .map(|gid| active.contains(gid))
                        .unwrap_or(false)
            })
            .map(|t| (t.id.clone(), t.content.clone(), t.goal_id.clone().unwrap_or_default()))
    }?;
    let (todo_id, todo_content, goal_id) = picked;

    let goal_title = ctx
        .goals
        .lock()
        .unwrap()
        .iter()
        .find(|g| g.id == goal_id)
        .map(|g| g.title.clone())
        .unwrap_or_default();
    let task = format!(
        "【宿主委派】请独立完成下面这条待办，产出可验证的结果。\n\n\
        目标：{goal}\n\
        待办：{todo}\n\n\
        要求：自主拆解并执行（可用全部工具：shell / 读写文件 / 脚本），完成后在结论里说明做了什么、结果是什么、以及验证方式。",
        goal = if goal_title.is_empty() { "(无)" } else { &goal_title },
        todo = todo_content,
    );
    let title: String = format!("子代理 · {}", {
        let c: String = todo_content.chars().take(20).collect();
        c
    });

    // 先置 in_progress：子代理跑得久，防止下一周期又把同一条派出去
    let _ = crate::goal::update_todo_status(ctx, &todo_id, "in_progress");
    crate::audit::record(
        ctx,
        "host",
        "subagent.delegate",
        &todo_id,
        json!({ "todo": todo_content, "goal": goal_title }),
        true,
    );

    let ctx2 = ctx.clone();
    let bg_id = todo_id.clone();
    tauri::async_runtime::spawn(async move {
        let res = spawn(&ctx2, None, &task, Some(&title)).await;
        match res {
            Ok(v) => {
                let _ = crate::goal::update_todo_status(&ctx2, &bg_id, "completed");
                crate::audit::record(
                    &ctx2,
                    "host",
                    "subagent.delegate_done",
                    &bg_id,
                    json!({
                        "session_id": v.get("session_id").cloned().unwrap_or(json!(null)),
                        "answer_chars": v
                            .get("final_answer")
                            .and_then(|s| s.as_str())
                            .map(|s| s.chars().count())
                            .unwrap_or(0),
                    }),
                    true,
                );
            }
            Err(e) => {
                crate::audit::record(
                    &ctx2,
                    "host",
                    "subagent.delegate_failed",
                    &bg_id,
                    json!({ "error": e }),
                    false,
                );
            }
        }
    });
    Some(todo_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 可配置并行上限必须为正，否则策略永远派不出去
    #[test]
    fn cfg_limit_sane() {
        assert!(MAX_CFG_SUBAGENTS >= 1);
    }
}
