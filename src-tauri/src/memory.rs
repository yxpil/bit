// yxpil · BIT
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// persist_deferred 的合并标记：800ms 窗口内的多次写入只落一次盘
static PERSIST_QUEUED: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Deserialize, Clone)]
pub struct Memory {
    pub id: String,
    pub ts: String,
    pub kind: String, // raw | summary
    pub content: String,
    pub source: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Skill {
    pub id: String,
    pub ts: String,
    pub name: String,
    pub summary: String,
    pub source: String,
}

pub fn add_memory(ctx: &Arc<crate::state::Ctx>, content: &str, kind: &str, source: &str) -> Memory {
    let mut mem = ctx.memories.lock().unwrap();
    let m = Memory {
        id: crate::goal::next_short_id(mem.iter().map(|x| &x.id)),
        ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        kind: kind.to_string(),
        content: content.trim().to_string(),
        source: source.to_string(),
    };
    mem.push(m.clone());
    if mem.len() > 500 {
        let drop_n = mem.len() - 500;
        mem.drain(0..drop_n);
    }
    drop(mem);
    persist_deferred(ctx);
    m
}

pub fn add_skill(ctx: &Arc<crate::state::Ctx>, name: &str, summary: &str, source: &str) -> Skill {
    // 单次加锁内完成 id 分配 + 去重 + 插入：Mutex 不可重入，绝不在同线程二次 lock
    // （旧实现对 skills 连续 lock 两次 → 永久死锁，Autopilot 技能提炼 / skill save 直接卡死）
    let mut skills = ctx.skills.lock().unwrap();
    let s = Skill {
        id: crate::goal::next_short_id(skills.iter().map(|x| &x.id)),
        ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        name: name.trim().to_string(),
        summary: summary.trim().to_string(),
        source: source.to_string(),
    };
    // 同名技能去重：新总结覆盖旧的
    skills.retain(|x| !x.name.eq_ignore_ascii_case(&s.name));
    skills.push(s.clone());
    if skills.len() > 200 {
        let drop_n = skills.len() - 200;
        skills.drain(0..drop_n);
    }
    drop(skills);
    persist_deferred(ctx);
    s
}

fn persist(ctx: &Arc<crate::state::Ctx>) {
    let mem = ctx.memories.lock().unwrap();
    let _ = std::fs::write(
        ctx.data_dir.join("memories.json"),
        serde_json::to_string(&*mem).unwrap_or_default(),
    );
    drop(mem);
    let skills = ctx.skills.lock().unwrap();
    let _ = std::fs::write(
        ctx.data_dir.join("skills.json"),
        serde_json::to_string(&*skills).unwrap_or_default(),
    );
}

/// 延迟合并落盘：AI 一轮连存 N 条记忆/技能时（如批量提炼 14 条），
/// 800ms 窗口内的写入合并成一次全量落盘，避免 N×2 次文件写。
/// 最终回复落盘（session::persist）不等这个窗口，崩溃最多丢窗口内的沉淀
fn persist_deferred(ctx: &Arc<crate::state::Ctx>) {
    PERSIST_QUEUED.store(true, Ordering::SeqCst);
    let c = ctx.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        if PERSIST_QUEUED.swap(false, Ordering::SeqCst) {
            persist(&c);
        }
    });
}

/// 批量删除记忆，返回实际删除条数
pub fn delete_memories(ctx: &Arc<crate::state::Ctx>, ids: &[String]) -> usize {
    let mut mem = ctx.memories.lock().unwrap();
    let before = mem.len();
    mem.retain(|m| !ids.contains(&m.id));
    let removed = before - mem.len();
    drop(mem);
    persist(ctx);
    removed
}

/// 批量删除技能，返回实际删除条数
pub fn delete_skills(ctx: &Arc<crate::state::Ctx>, ids: &[String]) -> usize {
    let mut skills = ctx.skills.lock().unwrap();
    let before = skills.len();
    skills.retain(|s| !ids.contains(&s.id));
    let removed = before - skills.len();
    drop(skills);
    persist(ctx);
    removed
}

/// 清除原始记忆，写入一条总结记忆（由 AI 生成摘要）
pub fn compress_memories(ctx: &Arc<crate::state::Ctx>, raw_ids: &[String], summary: &str) -> usize {
    let mut mem = ctx.memories.lock().unwrap();
    let before = mem.len();
    mem.retain(|m| !raw_ids.contains(&m.id));
    let removed = before - mem.len();
    let new_id = crate::goal::next_short_id(mem.iter().map(|x| &x.id));
    mem.push(Memory {
        id: new_id,
        ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        kind: "summary".into(),
        content: summary.trim().to_string(),
        source: "autopilot".into(),
    });
    drop(mem);
    persist(ctx);
    removed
}
