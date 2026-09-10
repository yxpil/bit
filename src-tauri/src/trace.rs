// yxpil · BIT — 极简同步调试追踪（死锁诊断专用）
// 设计原则：
//   1. 零依赖：不引入 channel/thread 等可能干扰排查的组件
//   2. 同步直写：OS 文件锁自动排队，日志本身不会成为新的死锁源
//   3. 崩溃留存：每行独立 flush，进程 crash 前最后痕迹不会丢
//   4. monotonic 时钟 + 线程 ID，便于排序与跨线程锁序分析

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static PATH: OnceLock<std::path::PathBuf> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

/// 开启追踪：日志写到 data_dir/trace.log
pub fn init(data_dir: &std::path::Path) {
    if ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let path = data_dir.join("trace.log");
    let bak = data_dir.join("trace.log.prev");
    let _ = std::fs::rename(&path, &bak);
    let _ = PATH.set(path);
    let _ = START.set(Instant::now());
    ENABLED.store(true, Ordering::Relaxed);
    log("trace", "init", "tracing enabled");
}

#[inline(always)]
fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 底层写行：带时间戳、线程 ID、目标、消息
#[inline]
fn log(target: &str, phase: &str, detail: &str) {
    if !enabled() {
        return;
    }
    let Some(path) = PATH.get() else { return };
    let start = START.get().copied().unwrap_or_else(Instant::now);
    let ts = start.elapsed().as_millis() as f64 / 1000.0;
    let tid = format!("{:?}", std::thread::current().id());
    let line = format!("[{ts:.3}] {tid:>5} | {phase:>9} | {target} | {detail}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 记录函数入口
#[inline(always)]
pub fn enter(target: &str, detail: &str) {
    log(target, "ENTER", detail);
}

/// 记录函数出口
#[inline(always)]
pub fn exit(target: &str, detail: &str) {
    log(target, "EXIT", detail);
}

/// 记录事件
#[inline(always)]
pub fn event(target: &str, detail: &str) {
    log(target, "EVENT", detail);
}

/// 记录锁获取
#[inline(always)]
pub fn lock_acquired(name: &str, location: &str) {
    log(name, "LOCK_ACQ", location);
}

/// 记录锁释放
#[inline(always)]
pub fn lock_released(name: &str, location: &str) {
    log(name, "LOCK_REL", location);
}

/// 耗时块（span）：构造时 ENTER，Drop 时 EXIT
pub struct Span {
    target: String,
    detail: String,
}

impl Span {
    pub fn new(target: &str, detail: &str) -> Self {
        enter(target, detail);
        Self {
            target: target.to_string(),
            detail: detail.to_string(),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        exit(&self.target, &self.detail);
    }
}
