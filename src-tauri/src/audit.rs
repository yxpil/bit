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

    // 追加写持久化文件（全量覆写，保证与内存一致）
    let _ = std::fs::write(
        ctx.data_dir.join("audit.json"),
        serde_json::to_string(&*log).unwrap_or_default(),
    );
}
