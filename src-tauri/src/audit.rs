use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone)]
pub struct AuditEntry {
    pub id: String,
    pub ts: String,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: serde_json::Value,
    pub ok: bool,
}

/// 记录一条审计事件（内存 + 持久化）
pub fn record(
    ctx: &Arc<crate::state::Ctx>,
    actor: &str,
    action: &str,
    target: &str,
    detail: serde_json::Value,
    ok: bool,
) {
    let entry = AuditEntry {
        id: uuid::Uuid::new_v4().to_string(),
        ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        actor: actor.to_string(),
        action: action.to_string(),
        target: target.to_string(),
        detail,
        ok,
    };

    let mut log = ctx.audit.lock().unwrap();
    log.push(entry.clone());
    if log.len() > crate::state::AUDIT_MAX {
        let drop_n = log.len() - crate::state::AUDIT_MAX;
        log.drain(0..drop_n);
    }
    drop(log);
    persist(ctx);
}

/// 把当前内存审计日志全量落盘（调用方须已释放 audit 锁）
pub fn persist(ctx: &Arc<crate::state::Ctx>) {
    let log = ctx.audit.lock().unwrap();
    let _ = std::fs::write(
        ctx.data_dir.join("audit.json"),
        serde_json::to_string(&*log).unwrap_or_default(),
    );
}

/// 清空审计日志（内存 + 持久化）
pub fn clear(ctx: &Arc<crate::state::Ctx>) {
    ctx.audit.lock().unwrap().clear();
    persist(ctx);
}

/// 删除单条审计记录（内存 + 持久化）；返回是否存在
pub fn delete(ctx: &Arc<crate::state::Ctx>, id: &str) -> bool {
    let removed = {
        let mut log = ctx.audit.lock().unwrap();
        let before = log.len();
        log.retain(|e| e.id != id);
        before != log.len()
    };
    if removed {
        persist(ctx);
    }
    removed
}
