// yxpil · BIT
//! 守护进程：与主进程互为看门狗，主进程被外界/病毒杀掉后自动"接力"拉起，
//! 并把接力事件写入日志，主进程下次启动时转存审计留痕。
//!
//! 安全设计：
//! - 拉起前校验主程序二进制 sha256 与布防时一致——二进制被换装（病毒篡改）时拒绝"借壳复活"
//! - 主进程正常退出（托盘退出 / 更新重启）前置 expect_exit 标记，守护进程不误拉
//! - 握手文件信封签名：{"payload":..,"hmac":..}，hmac = HMAC-SHA256(client_key, payload)——
//!   盲写 expect_exit / 清空 / 伪造状态无法解除布防；旧明文格式一律按篡改拒绝
//! - 握手文件被删/损坏/验签失败：守护进程沿用内存中最后可信状态继续守望（状态切换沿留痕一次，不刷屏）；
//!   主进程侧巡检（5s）发现缺失/损坏即重新布防自愈。已知行为：macOS App Nap 会节流
//!   隐藏窗口 GUI 进程的定时器，后台空闲时主进程侧自愈实测 5~15s（无窗口的守护进程 tick 始终准点，
//!   关键复活路径不受影响）
//! - 局限（如实注明）：同用户级定向攻击者可读 config.json 获得密钥，签名只防盲写/kill-switch 级恶意软件，
//!   与安全模块既有信任假设一致
//! - 新实例布防时接管存活旧守护进程（按 guardian_pid 判定），旧守护进程发现被接管即退出
//! - 连续拉起超过上限自动放弃（防崩溃死循环），事件均留痕
//! - BIT_NO_GUARDIAN=1 环境变量可整体关闭（E2E / 调试用）

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::state::Ctx;

/// 守护进程命令行标记：`bit --bit-guardian <握手文件> <接力日志>`
pub const GUARDIAN_FLAG: &str = "--bit-guardian";
/// 连续拉起上限：超过即放弃守护（防崩溃死循环烧资源）
const MAX_CONSECUTIVE_RESTARTS: u32 = 10;
/// 看门狗巡检间隔
const TICK: std::time::Duration = std::time::Duration::from_secs(2);

/// 主进程 ↔ 守护进程握手文件（data_dir/guardian.json）
#[derive(Serialize, Deserialize, Clone)]
struct GuardianState {
    /// 被守护的主进程 pid
    pid: u32,
    /// 守护进程自身 pid（新实例据此接管；接管后旧守护进程自行退出）
    guardian_pid: u32,
    /// 主进程正常退出标记：true = 守护进程不要拉起、自行退出
    expect_exit: bool,
    /// 布防时的主程序二进制 sha256（拉起前校验，防病毒换装借壳）
    bin_hash: String,
}

/// 接力日志（data_dir/guardian.log，JSON 行）：事件在主进程下次启动时转存审计
#[derive(Serialize, Deserialize)]
struct GuardianEvent {
    time: String,
    event: String,
    detail: String,
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 握手文件信封：payload 为 GuardianState 紧凑 JSON，hmac 为其 HMAC-SHA256(client_key)
#[derive(Serialize, Deserialize)]
struct StateEnvelope {
    payload: String,
    hmac: String,
}

/// 握手状态读取失败分类：Missing=文件不存在；Unreadable=读坏/解析失败；Tampered=未签名或验签不过
#[derive(Debug)]
enum StateError {
    Missing,
    Unreadable,
    Tampered,
}

/// 验签读取握手文件（纯函数，不回退内存态——回退策略由调用方持有，便于单测）。
/// 旧版明文状态（无 payload 字段）与 HMAC 不匹配一律 Tampered：防明文 expect_exit 伪造解除布防
fn load_state_verified(path: &Path, key: &str) -> Result<GuardianState, StateError> {
    let raw = std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => StateError::Missing,
        _ => StateError::Unreadable,
    })?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|_| StateError::Unreadable)?;
    let payload = v.get("payload").and_then(|p| p.as_str()).ok_or(StateError::Tampered)?;
    let hmac = v.get("hmac").and_then(|h| h.as_str()).ok_or(StateError::Tampered)?;
    let expect = hex(&crate::security::hmac_sha256(key.as_bytes(), payload.as_bytes()));
    if !crate::security::ct_eq(expect.as_bytes(), hmac.as_bytes()) {
        return Err(StateError::Tampered);
    }
    serde_json::from_str(payload).map_err(|_| StateError::Unreadable)
}

/// 唯一握手写入通道：签名装信封，临时文件 + rename 原子替换（防读侧撕裂被误判为篡改）
fn write_state_signed(state: &GuardianState, path: &Path, key: &str) {
    let Ok(payload) = serde_json::to_string(state) else { return };
    let hmac = hex(&crate::security::hmac_sha256(key.as_bytes(), payload.as_bytes()));
    let env = serde_json::to_string(&StateEnvelope { payload, hmac }).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, env).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// 只读解析 config.json 的 client_key（不走 Config::load：其解析失败会把默认配置写回磁盘，
/// 可能覆盖用户配置——守护进程绝不能有写配置的副作用）
fn load_client_key(dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(dir.join("config.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let k = v.get("client_key")?.as_str()?.to_string();
    if k.is_empty() { None } else { Some(k) }
}

/// 巡检一步的状态刷新：验签成功 → 同步内存态并清告警沿；失败 → 沿用内存态
/// （删除/篡改握手文件不能解除布防）。Tampered 时先重读一次 client_key 再试
/// （兼容运行中 regenerate_client_key 轮换）；告警沿只触发一次留痕，避免 2s 一条刷屏
fn tick_refresh(
    data_dir: &Path,
    state_path: &Path,
    key: &mut String,
    state: &mut GuardianState,
    alarmed: &mut bool,
    log_path: &Path,
) {
    match load_state_verified(state_path, key) {
        Ok(newer) => {
            *state = newer;
            *alarmed = false;
        }
        Err(StateError::Tampered) => {
            // key 轮换重试：磁盘 key 变了则采纳并重验
            if let Some(k) = load_client_key(data_dir) {
                if k != *key && load_state_verified(state_path, &k).is_ok() {
                    *key = k;
                    *state = load_state_verified(state_path, key).unwrap();
                    *alarmed = false;
                    return;
                }
            }
            if !*alarmed {
                append_event(log_path, "tamper", "handshake hmac invalid; keep last known state");
                *alarmed = true;
            }
        }
        Err(e) => {
            if !*alarmed {
                append_event(log_path, "state_invalid", &format!("handshake unreadable ({e:?}); keep last known state"));
                *alarmed = true;
            }
        }
    }
}

/// 诊断视图：验签通过 → 状态字段；失败 → { "error": 分类 }（篡改在诊断面板可见）
pub fn diagnose(data_dir: &Path, key: &str) -> serde_json::Value {
    match load_state_verified(&data_dir.join("guardian.json"), key) {
        Ok(s) => serde_json::to_value(&s).unwrap_or(serde_json::Value::Null),
        Err(e) => json!({ "error": format!("{e:?}") }),
    }
}

fn append_event(path: &Path, event: &str, detail: &str) {
    let line = serde_json::to_string(&GuardianEvent {
        time: now(),
        event: event.to_string(),
        detail: detail.to_string(),
    })
    .unwrap_or_default();
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

fn sha256_file(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let data = std::fs::read(path).ok()?;
    Some(hex(&Sha256::digest(&data)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 进程存活检查（僵尸进程视为已死：父进程尚未回收时避免误判存活）
fn pid_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    sys.process(Pid::from_u32(pid))
        .map(|p| p.status() != sysinfo::ProcessStatus::Zombie)
        .unwrap_or(false)
}

/// 守护开关：默认开启，BIT_NO_GUARDIAN=1 关闭（E2E / 调试 / TUI）。
/// macOS 上默认关闭（用户 kill 后会被重拉，"关不掉"；如需守护可设 BIT_GUARDIAN=1 显式开启）。
pub fn enabled() -> bool {
    if cfg!(target_os = "macos") {
        return std::env::var("BIT_GUARDIAN").is_ok();
    }
    std::env::var("BIT_NO_GUARDIAN").is_err()
}

/// 主进程侧布防（桌面端启动时调用）：写签名握手文件、接管或拉起守护进程。
/// bin_hash 仅在布防时记录（信任基线），运行中不重算——防止病毒趁主进程存活时
/// 换装二进制后被下一次布防"洗白"。
pub fn arm(ctx: &Arc<Ctx>) {
    if !enabled() {
        // 关闭守护时通知残留的旧守护进程退出（它每 2s 读一次握手文件，读到 expect_exit 即退出），
        // 否则升级前布防的旧守护进程会继续重拉主进程，造成"关不掉"
        expect_exit(ctx);
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let Some(hash) = sha256_file(&exe) else { return };
    let state_path = ctx.data_dir.join("guardian.json");
    let log_path = ctx.data_dir.join("guardian.log");
    let my_pid = std::process::id();
    let key = ctx.config.lock().unwrap().client_key.clone();

    // 握手文件缺失/损坏/验签不过（含旧明文格式）：走全新布防分支重写签名版——
    // 存活的旧守护进程会在 2s 内读到新签名文件，按接管逻辑自行让位
    let mut state = load_state_verified(&state_path, &key).unwrap_or(GuardianState {
        pid: my_pid,
        guardian_pid: 0,
        expect_exit: false,
        bin_hash: hash.clone(),
    });
    // 复用/接管：旧守护进程仍存活则只更新守护目标与基线，不重复拉起
    if state.guardian_pid != my_pid && state.guardian_pid != 0 && pid_alive(state.guardian_pid) {
        state.pid = my_pid;
        state.expect_exit = false;
        state.bin_hash = hash;
        write_state_signed(&state, &state_path, &key);
        return;
    }
    state.pid = my_pid;
    state.guardian_pid = 0;
    state.expect_exit = false;
    state.bin_hash = hash;
    write_state_signed(&state, &state_path, &key);
    spawn_guardian(&exe, &state_path, &log_path);
    // 磁盘 key 漂移自愈：config.json 的 client_key 与内存不一致（带外改动）时以内存为准回写，
    // 避免守护进程用磁盘 key 验签永久失败导致反复重布防
    if load_client_key(&ctx.data_dir).as_deref() != Some(key.as_str()) {
        let cfg = ctx.config.lock().unwrap();
        cfg.save(&ctx.data_dir);
        drop(cfg);
        crate::audit::record(ctx, "local-app", "guardian.key_drift", "BIT", json!({ "resynced": true }), true);
    }
}

fn spawn_guardian(exe: &Path, state_path: &Path, log_path: &Path) -> Option<()> {
    std::process::Command::new(exe)
        .arg(GUARDIAN_FLAG)
        .arg(state_path)
        .arg(log_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
        .map(|_| ())
}

/// 主进程侧巡检任务：守护进程意外死亡时重新拉起（互为看门狗的另一半）。
/// 5s 巡检：缩短守护被杀后的无守护窗口；握手文件缺失/损坏/验签失败也重新布防自愈
pub async fn watchdog_task(ctx: Arc<Ctx>) {
    let state_path = ctx.data_dir.join("guardian.json");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if !enabled() {
            continue;
        }
        let key = ctx.config.lock().unwrap().client_key.clone();
        let needs_respawn = match load_state_verified(&state_path, &key) {
            Ok(s) => s.guardian_pid == 0 || (s.guardian_pid != std::process::id() && !pid_alive(s.guardian_pid)),
            Err(_) => true, // 缺失/损坏/验签失败：重新布防（arm 重写签名状态自愈）
        };
        if needs_respawn {
            arm(&ctx);
        }
    }
}

/// 主进程正常退出前调用：通知守护进程不要拉起、自行退出。
/// 读取失败（文件被删/篡改）不放弃通知：用当前基线重建并签名——只要守护进程能验签
/// 就会读到退出标记（防"篡改文件后正常退出被复活"）
pub fn expect_exit(ctx: &Arc<Ctx>) {
    let state_path = ctx.data_dir.join("guardian.json");
    let key = ctx.config.lock().unwrap().client_key.clone();
    let mut state = load_state_verified(&state_path, &key).unwrap_or(GuardianState {
        pid: std::process::id(),
        guardian_pid: 0,
        expect_exit: true,
        bin_hash: std::env::current_exe().ok().and_then(|e| sha256_file(&e)).unwrap_or_default(),
    });
    state.expect_exit = true;
    write_state_signed(&state, &state_path, &key);
}

/// 主进程启动时把接力日志转存审计（守卫被杀→拉起、篡改拒绝、放弃守护等），随后清空日志
pub fn drain_log(ctx: &Arc<Ctx>) {
    let log_path = ctx.data_dir.join("guardian.log");
    let Ok(content) = std::fs::read_to_string(&log_path) else { return };
    let mut recorded = 0;
    for line in content.lines() {
        let Ok(ev) = serde_json::from_str::<GuardianEvent>(line) else { continue };
        let action = format!("guardian.{}", ev.event);
        crate::audit::record(
            ctx,
            "guardian",
            &action,
            "BIT",
            json!({ "at": ev.time, "detail": ev.detail }),
            true,
        );
        recorded += 1;
    }
    if recorded > 0 {
        let _ = std::fs::remove_file(&log_path);
        crate::audit::record(ctx, "guardian", "guardian.summary", "BIT", json!({ "events": recorded }), true);
    }
}

/// 守护进程主循环（本进程以 `bit --bit-guardian <握手文件> <日志>` 启动，不进入 GUI/TUI）。
/// 巡检：被新实例接管 → 退出；主进程正常退出 → 退出；主进程意外死亡 → 校验完整性后接力拉起。
/// 握手文件被删/损坏/验签失败：沿用内存中最后可信状态继续守望（删除/篡改不能解除布防）；
/// 启动时无法验签则退出（主进程 5s 内重新布防重写签名状态）
pub fn run_guardian(state_path: PathBuf, log_path: PathBuf) {
    let Some(exe) = std::env::current_exe().ok() else { return };
    let my_pid = std::process::id();
    let data_dir = state_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    // 守护进程只读 config.json 取验签密钥；读不到则无法验签——立即退出让主进程重布防
    let Some(mut key) = load_client_key(&data_dir) else {
        append_event(&log_path, "tamper", "config unreadable; cannot verify handshake; exiting");
        return;
    };
    let Ok(mut state) = load_state_verified(&state_path, &key) else {
        append_event(&log_path, "tamper", "handshake invalid at startup; exiting for re-arm");
        return;
    };
    state.guardian_pid = my_pid;
    write_state_signed(&state, &state_path, &key);

    let mut consecutive: u32 = 0;
    let mut alarmed = false;
    loop {
        std::thread::sleep(TICK);
        // 状态刷新：验签成功 → 同步内存态；失败 → 沿用内存态（删除/篡改不能解除布防），
        // 告警沿只留痕一次
        tick_refresh(&data_dir, &state_path, &mut key, &mut state, &mut alarmed, &log_path);
        if state.guardian_pid != my_pid || state.expect_exit {
            return;
        }
        if pid_alive(state.pid) {
            consecutive = 0;
            continue;
        }
        // 主进程死亡：接力
        consecutive += 1;
        if consecutive > MAX_CONSECUTIVE_RESTARTS {
            append_event(&log_path, "giveup", &format!("consecutive restarts exceeded {MAX_CONSECUTIVE_RESTARTS}"));
            return;
        }
        // 完整性校验：二进制被换装（病毒）→ 拒绝借壳复活
        match sha256_file(&exe) {
            Some(h) if h == state.bin_hash => {}
            Some(_) => {
                append_event(&log_path, "tamper", "binary hash changed since arm; refusing to relaunch");
                return;
            }
            None => {
                append_event(&log_path, "tamper", "binary unreadable; refusing to relaunch");
                return;
            }
        }
        match std::process::Command::new(&exe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                append_event(&log_path, "restart", &format!("main pid {} died; relaunched as {}", state.pid, child.id()));
                state.pid = child.id();
                write_state_signed(&state, &state_path, &key);
            }
            Err(e) => {
                append_event(&log_path, "spawn_err", &format!("{e}"));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guardian.json");
        let st = GuardianState {
            pid: 123,
            guardian_pid: 456,
            expect_exit: true,
            bin_hash: "abc".into(),
        };
        write_state_signed(&st, &path, "k1");
        let back = load_state_verified(&path, "k1").unwrap();
        assert_eq!(back.pid, 123);
        assert_eq!(back.guardian_pid, 456);
        assert!(back.expect_exit);
        assert_eq!(back.bin_hash, "abc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_signed_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-sig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guardian.json");
        let st = GuardianState { pid: 7, guardian_pid: 9, expect_exit: false, bin_hash: "h".into() };
        write_state_signed(&st, &path, "key-x");
        // 信封结构确证：外层有 payload/hmac 两字段，payload 内嵌状态 JSON
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v.get("payload").and_then(|p| p.as_str()).is_some());
        assert_eq!(v.get("hmac").and_then(|h| h.as_str()).map(|s| s.len()), Some(64));
        let back = load_state_verified(&path, "key-x").unwrap();
        assert_eq!(back.pid, 7);
        assert_eq!(back.guardian_pid, 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tamper_detected() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-tamper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guardian.json");
        let st = GuardianState { pid: 1, guardian_pid: 2, expect_exit: false, bin_hash: "h".into() };
        write_state_signed(&st, &path, "k");
        let raw = std::fs::read_to_string(&path).unwrap();
        // 篡改 payload 一个字符 → Tampered（伪造 expect_exit 的场景被拦截）
        let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let payload = v["payload"].as_str().unwrap().to_string();
        let flipped = if payload.contains("\"pid\":1") { payload.replacen("\"pid\":1", "\"pid\":2", 1) } else { payload.replacen("1", "2", 1) };
        v["payload"] = serde_json::Value::String(flipped);
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        assert!(matches!(load_state_verified(&path, "k"), Err(StateError::Tampered)));
        // 篡改 hmac 一个字符 → Tampered
        write_state_signed(&st, &path, "k");
        let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let hmac = v["hmac"].as_str().unwrap().to_string();
        let ch = if hmac.starts_with('0') { '1' } else { '0' };
        v["hmac"] = serde_json::Value::String(format!("{ch}{}", &hmac[1..]));
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        assert!(matches!(load_state_verified(&path, "k"), Err(StateError::Tampered)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_legacy_plaintext_rejected() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guardian.json");
        // 旧版明文格式（含伪造 expect_exit=true）→ 拒绝
        let legacy = GuardianState { pid: 5, guardian_pid: 6, expect_exit: true, bin_hash: "h".into() };
        std::fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();
        assert!(matches!(load_state_verified(&path, "k"), Err(StateError::Tampered)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_missing_and_wrong_key() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-miss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guardian.json");
        assert!(matches!(load_state_verified(&path, "k"), Err(StateError::Missing)));
        let st = GuardianState { pid: 1, guardian_pid: 0, expect_exit: false, bin_hash: "h".into() };
        write_state_signed(&st, &path, "k1");
        assert!(matches!(load_state_verified(&path, "k2"), Err(StateError::Tampered)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_client_key() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-ck-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 正常读取
        std::fs::write(dir.join("config.json"), r#"{"client_key":"ck-123"}"#).unwrap();
        assert_eq!(load_client_key(&dir).as_deref(), Some("ck-123"));
        // 空串 → None
        std::fs::write(dir.join("config.json"), r#"{"client_key":""}"#).unwrap();
        assert_eq!(load_client_key(&dir), None);
        // 缺字段 / 坏 JSON / 文件缺失 → None
        std::fs::write(dir.join("config.json"), r#"{"other":1}"#).unwrap();
        assert_eq!(load_client_key(&dir), None);
        std::fs::write(dir.join("config.json"), "not-json").unwrap();
        assert_eq!(load_client_key(&dir), None);
        std::fs::remove_file(dir.join("config.json")).unwrap();
        assert_eq!(load_client_key(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tick_refresh_fallback_and_recover() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-tick-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guardian.json");
        let log = dir.join("guardian.log");
        let st = GuardianState { pid: 11, guardian_pid: 22, expect_exit: false, bin_hash: "h".into() };
        write_state_signed(&st, &path, "k1");
        let mut key = "k1".to_string();
        let mut state = st.clone();
        let mut alarmed = false;
        // 1) 良好文件：同步内存态、清告警
        tick_refresh(&dir, &path, &mut key, &mut state, &mut alarmed, &log);
        assert_eq!(state.pid, 11);
        assert!(!alarmed);
        // 2) 篡改文件：内存态保持、告警沿触发一次
        std::fs::write(&path, "{\"payload\":\"x\",\"hmac\":\"y\"}").unwrap();
        state.pid = 99; // 标记内存态，验证不被破坏性覆盖
        tick_refresh(&dir, &path, &mut key, &mut state, &mut alarmed, &log);
        assert_eq!(state.pid, 99, "篡改时内存态必须原样保留");
        assert!(alarmed);
        assert!(std::fs::read_to_string(&log).unwrap().contains("tamper"));
        // 3) 再次巡检：告警沿不重复留痕
        let before = std::fs::read_to_string(&log).unwrap().lines().count();
        tick_refresh(&dir, &path, &mut key, &mut state, &mut alarmed, &log);
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), before, "告警沿只留痕一次");
        // 4) 恢复良好文件：内存态更新、告警复位
        write_state_signed(&st, &path, "k1");
        tick_refresh(&dir, &path, &mut key, &mut state, &mut alarmed, &log);
        assert_eq!(state.pid, 11);
        assert!(!alarmed);
        // 5) 文件删除：内存态保持（删除不能解除布防）
        std::fs::remove_file(&path).unwrap();
        state.pid = 77;
        tick_refresh(&dir, &path, &mut key, &mut state, &mut alarmed, &log);
        assert_eq!(state.pid, 77);
        assert!(alarmed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tick_refresh_key_rotation() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-rot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guardian.json");
        let log = dir.join("guardian.log");
        // 磁盘 config.json 已轮换为 k2，握手文件用 k2 签名；守护进程内存仍持 k1
        std::fs::write(dir.join("config.json"), r#"{"client_key":"k2"}"#).unwrap();
        let st = GuardianState { pid: 31, guardian_pid: 32, expect_exit: true, bin_hash: "h".into() };
        write_state_signed(&st, &path, "k2");
        let mut key = "k1".to_string();
        let mut state = GuardianState { pid: 0, guardian_pid: 0, expect_exit: false, bin_hash: String::new() };
        let mut alarmed = false;
        tick_refresh(&dir, &path, &mut key, &mut state, &mut alarmed, &log);
        // 轮换被采纳：退出标记仍能被守护进程验签读到（托盘退出不误拉）
        assert_eq!(key, "k2");
        assert_eq!(state.pid, 31);
        assert!(state.expect_exit);
        assert!(!alarmed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_event_log_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guardian.log");
        append_event(&path, "restart", "pid 1 died; relaunched as 2");
        append_event(&path, "tamper", "hash mismatch");
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<GuardianEvent> = content.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].event, "restart");
        assert_eq!(lines[1].event, "tamper");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_sha256_stable() {
        let dir = std::env::temp_dir().join(format!("bit-guardian-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.bin");
        std::fs::write(&path, b"hello").unwrap();
        let h1 = sha256_file(&path).unwrap();
        let h2 = sha256_file(&path).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        // sha256("hello")
        assert_eq!(h1, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        std::fs::write(&path, b"world").unwrap();
        assert_ne!(sha256_file(&path).unwrap(), h1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_pid_alive_self() {
        assert!(pid_alive(std::process::id()));
        // pid 0 恒不存在（swapper/无效），不应误报存活
        assert!(!pid_alive(0));
    }
}
