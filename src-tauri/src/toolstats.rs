// yxpil · BIT
//! 工具质量评估：按工具聚合调用成功率（近期窗口 + 累计），供工具页质量徽章与诊断报告使用。
//! 所有调用路径（AI / 本地 / 远程 API）统一经过 registry::invoke，在这里收口统计。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::state::Ctx;

/// 近期成功率窗口：只看最近 N 次结果，让近期表现优先于历史累计
const RECENT_WINDOW: usize = 50;

/// 单个工具的质量统计（持久化到 tool_stats.json）
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ToolStat {
    /// 近期结果窗口（true=成功），超出窗口淘汰最旧
    #[serde(default)]
    pub recent: Vec<bool>,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub ok: u64,
    #[serde(default)]
    pub fail: u64,
    /// 累计耗时（毫秒），用于诊断慢工具
    #[serde(default)]
    pub sum_ms: u64,
    /// 最近一次失败原因（截断）
    #[serde(default)]
    pub last_err: String,
    #[serde(default)]
    pub last_used: String,
}

impl ToolStat {
    /// 近期成功率（0.0-1.0）；从未调用返回 None
    pub fn recent_rate(&self) -> Option<f64> {
        if self.recent.is_empty() {
            None
        } else {
            Some(self.recent.iter().filter(|x| **x).count() as f64 / self.recent.len() as f64)
        }
    }
}

/// tool_id → 统计。Ctx 持有内存态，启动时从 tool_stats.json 载入
pub type Store = HashMap<String, ToolStat>;

/// 记录一次工具调用结果（含耗时与失败原因），即时落盘。任何 IO 失败静默放弃（不影响调用）
pub fn record(ctx: &Arc<Ctx>, tool_id: &str, ok: bool, dur_ms: u64, err: Option<&str>) {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let json = {
        let mut map = ctx.tool_stats.lock().unwrap();
        let s = map.entry(tool_id.to_string()).or_default();
        s.recent.push(ok);
        if s.recent.len() > RECENT_WINDOW {
            s.recent.remove(0);
        }
        s.total += 1;
        if ok {
            s.ok += 1;
        } else {
            s.fail += 1;
            if let Some(e) = err {
                s.last_err = e.chars().take(200).collect();
            }
        }
        s.sum_ms += dur_ms;
        s.last_used = now;
        serde_json::to_string(&*map).unwrap_or_default()
    };
    let _ = std::fs::write(ctx.data_dir.join("tool_stats.json"), json);
}

/// 前端快照：附工具名，按失败次数降序
pub fn snapshot(ctx: &Arc<Ctx>) -> serde_json::Value {
    let map = ctx.tool_stats.lock().unwrap();
    let mut list: Vec<serde_json::Value> = map
        .iter()
        .map(|(id, s)| {
            let rate = s.recent_rate();
            serde_json::json!({
                "id": id,
                "name": ctx.tools.lock().unwrap().iter().find(|t| &t.id == id).map(|t| t.name.clone()).unwrap_or_else(|| id.clone()),
                "ok": s.ok,
                "fail": s.fail,
                "total": s.total,
                "recent_rate": rate,
                "recent_n": s.recent.len(),
                "avg_ms": if s.total > 0 { s.sum_ms / s.total } else { 0 },
                "last_err": s.last_err,
                "last_used": s.last_used,
            })
        })
        .collect();
    list.sort_by(|a, b| b["fail"].as_u64().cmp(&a["fail"].as_u64()).then(b["total"].as_u64().cmp(&a["total"].as_u64())));
    serde_json::Value::Array(list)
}
