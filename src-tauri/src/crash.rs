// yxpil · BIT
//! 崩溃诊断：全局 panic 钩子把 panic 信息（含回溯）以 JSONL 追加到数据目录 crash.log，
//! 供诊断报告展示。硬崩溃（段错误等无法自捕的）由守护进程的 guardian.restart 事件留痕。

use serde_json::json;
use std::path::{Path, PathBuf};

/// 安装全局 panic 钩子：panic 信息追加写入 crash.log（JSONL），同时保留 stderr 输出。
/// 需在 Ctx 创建后调用（数据目录已确定）
pub fn install(data_dir: &Path) {
    let sink: PathBuf = data_dir.join("crash.log");
    std::panic::set_hook(Box::new(move |info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let entry = json!({
            "time": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            "thread": std::thread::current().name().unwrap_or("<unnamed>"),
            "msg": msg.chars().take(500).collect::<String>(),
            "loc": info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_default(),
            "backtrace": std::backtrace::Backtrace::force_capture()
                .to_string()
                .chars()
                .take(8000)
                .collect::<String>(),
        });
        eprintln!("[BIT] PANIC {} @ {}", entry["msg"], entry["loc"]);
        // 尽力而为：崩溃路径上的 IO 失败只能放弃
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&sink) {
            use std::io::Write;
            let _ = writeln!(f, "{entry}");
        }
    }));
}

/// 读取最近 n 条崩溃记录（新的在前）；损坏的行跳过
pub fn tail(data_dir: &Path, n: usize) -> Vec<serde_json::Value> {
    let text = match std::fs::read_to_string(data_dir.join("crash.log")) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut list: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    list.reverse();
    list.truncate(n);
    list
}
