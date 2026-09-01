use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone)]
pub struct Goal {
    pub id: String,
    pub ts: String,
    pub updated_ts: String,
    pub title: String,
    pub detail: String,
    /// active | achieved | abandoned
    pub status: String,
    pub source: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Todo {
    pub id: String,
    pub ts: String,
    /// 关联目标（可为空，表示独立待办）
    #[serde(default)]
    pub goal_id: Option<String>,
    pub content: String,
    /// pending | in_progress | completed
    pub status: String,
    pub source: String,
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn persist(ctx: &Arc<crate::state::Ctx>) {
    let goals = ctx.goals.lock().unwrap();
    let _ = std::fs::write(
        ctx.data_dir.join("goals.json"),
        serde_json::to_string(&*goals).unwrap_or_default(),
    );
    drop(goals);
    let todos = ctx.todos.lock().unwrap();
    let _ = std::fs::write(
        ctx.data_dir.join("todos.json"),
        serde_json::to_string(&*todos).unwrap_or_default(),
    );
}

// ---------- Goal ----------

pub fn create_goal(ctx: &Arc<crate::state::Ctx>, title: &str, detail: &str, source: &str) -> Result<Goal, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("目标标题不能为空".into());
    }
    let g = Goal {
        id: uuid::Uuid::new_v4().simple().to_string(),
        ts: now(),
        updated_ts: now(),
        title: title.to_string(),
        detail: detail.trim().to_string(),
        status: "active".into(),
        source: source.to_string(),
    };
    let mut goals = ctx.goals.lock().unwrap();
    goals.push(g.clone());
    drop(goals);
    persist(ctx);
    Ok(g)
}

/// 目标状态归一化：接受模型的常见同义词 / 大小写 / 中英文变体，别动不动就打回
fn normalize_goal_status(s: &str) -> Option<&'static str> {
    match s.trim().to_lowercase().as_str() {
        "active" | "in_progress" | "in progress" | "doing" | "进行中" | "执行中" => Some("active"),
        "achieved" | "completed" | "complete" | "done" | "finished" | "已达成" | "完成" | "已完成" => Some("achieved"),
        "abandoned" | "cancelled" | "canceled" | "放弃" | "已放弃" | "取消" | "已取消" => Some("abandoned"),
        _ => None,
    }
}

/// 待办状态归一化：同上
pub fn normalize_todo_status(s: &str) -> Option<&'static str> {
    match s.trim().to_lowercase().as_str() {
        "pending" | "todo" | "to_do" | "未开始" | "待办" | "待处理" => Some("pending"),
        "in_progress" | "in progress" | "doing" | "active" | "进行中" | "执行中" => Some("in_progress"),
        "completed" | "complete" | "done" | "finished" | "achieved" | "成功" | "已完成" | "完成" => Some("completed"),
        _ => None,
    }
}

pub fn update_goal_status(ctx: &Arc<crate::state::Ctx>, id: &str, status: &str) -> Result<Goal, String> {
    let status = normalize_goal_status(status)
        .ok_or("状态必须是 active / achieved / abandoned（也接受：进行中/已完成/放弃 等同义词）")?;
    let mut goals = ctx.goals.lock().unwrap();
    let g = goals.iter_mut().find(|g| g.id == id).ok_or("目标不存在")?;
    g.status = status.to_string();
    g.updated_ts = now();
    let out = g.clone();
    drop(goals);
    persist(ctx);
    Ok(out)
}

pub fn remove_goal(ctx: &Arc<crate::state::Ctx>, id: &str) -> Result<(), String> {
    let mut goals = ctx.goals.lock().unwrap();
    goals.retain(|g| g.id != id);
    drop(goals);
    // 级联删除关联待办
    let mut todos = ctx.todos.lock().unwrap();
    todos.retain(|t| t.goal_id.as_deref() != Some(id));
    drop(todos);
    persist(ctx);
    Ok(())
}

// ---------- Todo ----------

pub fn add_todo(
    ctx: &Arc<crate::state::Ctx>,
    goal_id: Option<String>,
    content: &str,
    source: &str,
) -> Result<Todo, String> {
    let content = content.trim();
    if content.is_empty() {
        return Err("待办内容不能为空".into());
    }
    if let Some(gid) = &goal_id {
        let goals = ctx.goals.lock().unwrap();
        if !goals.iter().any(|g| &g.id == gid) {
            return Err("关联目标不存在".into());
        }
    }
    let t = Todo {
        id: uuid::Uuid::new_v4().simple().to_string(),
        ts: now(),
        goal_id,
        content: content.to_string(),
        status: "pending".into(),
        source: source.to_string(),
    };
    let mut todos = ctx.todos.lock().unwrap();
    todos.push(t.clone());
    drop(todos);
    persist(ctx);
    Ok(t)
}

pub fn update_todo_status(ctx: &Arc<crate::state::Ctx>, id: &str, status: &str) -> Result<Todo, String> {
    let status = normalize_todo_status(status)
        .ok_or("状态必须是 pending / in_progress / completed（也接受：todo/done/进行中/已完成 等同义词）")?;
    let mut todos = ctx.todos.lock().unwrap();
    let t = todos.iter_mut().find(|t| t.id == id).ok_or("待办不存在")?;
    t.status = status.to_string();
    let out = t.clone();
    drop(todos);
    persist(ctx);
    Ok(out)
}

pub fn remove_todo(ctx: &Arc<crate::state::Ctx>, id: &str) -> Result<(), String> {
    let mut todos = ctx.todos.lock().unwrap();
    todos.retain(|t| t.id != id);
    drop(todos);
    persist(ctx);
    Ok(())
}

/// AI 批量写入待办（类似 TodoWrite：整体替换某个 goal 下或独立的待办列表）
pub fn rewrite_todos(
    ctx: &Arc<crate::state::Ctx>,
    goal_id: Option<String>,
    items: &[serde_json::Value],
    source: &str,
) -> Result<usize, String> {
    let mut todos = ctx.todos.lock().unwrap();
    // 清空同范围内旧待办
    match &goal_id {
        Some(gid) => todos.retain(|t| t.goal_id.as_deref() != Some(gid.as_str())),
        None => todos.retain(|t| t.goal_id.is_some()),
    }
    let mut count = 0;
    for item in items {
        let content = item.get("content").and_then(|v| v.as_str()).unwrap_or_default().trim().to_string();
        if content.is_empty() {
            continue;
        }
        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending");
        let status = normalize_todo_status(status).unwrap_or("pending").to_string();
        todos.push(Todo {
            id: uuid::Uuid::new_v4().simple().to_string(),
            ts: now(),
            goal_id: goal_id.clone(),
            content,
            status,
            source: source.to_string(),
        });
        count += 1;
    }
    let n = todos.len();
    drop(todos);
    let _ = n;
    persist(ctx);
    Ok(count)
}
