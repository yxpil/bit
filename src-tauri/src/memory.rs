use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
    let m = Memory {
        id: uuid::Uuid::new_v4().simple().to_string(),
        ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        kind: kind.to_string(),
        content: content.trim().to_string(),
        source: source.to_string(),
    };
    let mut mem = ctx.memories.lock().unwrap();
    mem.push(m.clone());
    if mem.len() > 500 {
        let drop_n = mem.len() - 500;
        mem.drain(0..drop_n);
    }
    drop(mem);
    persist(ctx);
    m
}

pub fn add_skill(ctx: &Arc<crate::state::Ctx>, name: &str, summary: &str, source: &str) -> Skill {
    let s = Skill {
        id: uuid::Uuid::new_v4().simple().to_string(),
        ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        name: name.trim().to_string(),
        summary: summary.trim().to_string(),
        source: source.to_string(),
    };
    let mut skills = ctx.skills.lock().unwrap();
    // 同名技能去重：新总结覆盖旧的
    skills.retain(|x| !x.name.eq_ignore_ascii_case(&s.name));
    skills.push(s.clone());
    if skills.len() > 200 {
        let drop_n = skills.len() - 200;
        skills.drain(0..drop_n);
    }
    drop(skills);
    persist(ctx);
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

/// 清除原始记忆，写入一条总结记忆（由 AI 生成摘要）
pub fn compress_memories(ctx: &Arc<crate::state::Ctx>, raw_ids: &[String], summary: &str) -> usize {
    let mut mem = ctx.memories.lock().unwrap();
    let before = mem.len();
    mem.retain(|m| !raw_ids.contains(&m.id));
    let removed = before - mem.len();
    mem.push(Memory {
        id: uuid::Uuid::new_v4().simple().to_string(),
        ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        kind: "summary".into(),
        content: summary.trim().to_string(),
        source: "autopilot".into(),
    });
    drop(mem);
    persist(ctx);
    removed
}
