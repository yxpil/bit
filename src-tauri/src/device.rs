// yxpil · BIT
//! 设备指纹与设备凭证（bitdev_*）：
//! 采集本机硬件指纹（CPU 品牌 / GPU / 内存 / 主板序列号）+ 真实公网 IP（公开 API 竞速）
//! + 注册时间戳，经 HMAC 派生设备 key 并持久化到配置。设备 key 是账号凭证级别的
//! 稳定锚点：并入信道签名材料（bitsign-v2），被中继转发的请求必须能算出 v2 签名。
//!
//! 语义约定：
//! - 注册一次、长期使用：IP 变动、内存微变不轮换 key（稳定锚点才有封禁价值）；
//!   只有手动清除或指纹整体重置（device_fp_hash 不匹配且用户确认）才重新注册。
//! - 开源客户端无法保守任何秘密：设备 key 提高伪造门槛、支撑服务端封禁与限额，
//!   最终防滥用以服务器侧（Worker 限流 / 设备黑名单 / 本地并发与审核）为准。
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::state::Ctx;

pub const DEVICE_KEY_PREFIX: &str = "bitdev_";
/// 签名材料派生盐（与 sign.cjs / 手机端实现约定一致）
pub const SIG_MATERIAL_SALT: &str = "bitdev-material:";

/// 采集到的本机硬件指纹
#[derive(Debug, Clone)]
pub struct DeviceFp {
    pub cpu: String,
    pub gpu: String,
    pub ram_gb: u64,
    pub board_serial: String,
}

fn trim_take(s: String) -> String {
    let t = s.trim().to_string();
    if t.is_empty() { "?".into() } else { t }
}

/// 执行系统命令取 stdout（失败返回空串；采集失败不致命，指纹允许弱化）
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn sysout(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn collect() -> DeviceFp {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let cpu = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_default();
    // 主板/整机序列号：IOPlatformExpertDevice 的 IOPlatformSerialNumber
    let ioreg = sysout("ioreg", &["-rd1", "-c", "IOPlatformExpertDevice"]);
    let board_serial = ioreg
        .lines()
        .find_map(|l| {
            let t = l.trim();
            t.starts_with("\"IOPlatformSerialNumber\"")
                .then(|| t.split('"').nth(3).unwrap_or_default().to_string())
        })
        .unwrap_or_default();
    // GPU：Chipset Model 第一行（一次性采集，慢一点可接受）
    let sp = sysout("system_profiler", &["SPDisplaysDataType"]);
    let gpu = sp
        .lines()
        .find_map(|l| {
            let t = l.trim();
            t.starts_with("Chipset Model")
                .then(|| t.split(':').nth(1).unwrap_or_default().trim().to_string())
        })
        .unwrap_or_default();
    DeviceFp {
        cpu: trim_take(cpu),
        gpu: trim_take(gpu),
        ram_gb: (sys.total_memory() / (1024 * 1024 * 1024)).max(1),
        board_serial: trim_take(board_serial),
    }
}

#[cfg(target_os = "linux")]
fn collect() -> DeviceFp {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let cpu = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_default();
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default().trim().to_string();
    let board_serial = read("/sys/class/dmi/id/board_serial");
    let board_serial = if board_serial.is_empty() { read("/etc/machine-id") } else { board_serial };
    let lspci = sysout("lspci", &[]);
    let gpu = lspci
        .lines()
        .find(|l| l.to_lowercase().contains("vga") || l.to_lowercase().contains("3d controller"))
        .and_then(|l| l.split(':').nth(2))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    DeviceFp {
        cpu: trim_take(cpu),
        gpu: trim_take(gpu),
        ram_gb: (sys.total_memory() / (1024 * 1024 * 1024)).max(1),
        board_serial: trim_take(board_serial),
    }
}

#[cfg(target_os = "windows")]
fn collect() -> DeviceFp {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let cpu = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_default();
    let board_serial = sysout("wmic", &["baseboard", "get", "serialnumber"])
        .lines()
        .nth(1)
        .unwrap_or_default()
        .trim()
        .to_string();
    let gpu = sysout("wmic", &["path", "win32_VideoController", "get", "name"])
        .lines()
        .nth(1)
        .unwrap_or_default()
        .trim()
        .to_string();
    DeviceFp {
        cpu: trim_take(cpu),
        gpu: trim_take(gpu),
        ram_gb: (sys.total_memory() / (1024 * 1024 * 1024)).max(1),
        board_serial: trim_take(board_serial),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn collect() -> DeviceFp {
    DeviceFp { cpu: "?".into(), gpu: "?".into(), ram_gb: 1, board_serial: "?".into() }
}

/// 公开 API 竞速取真实公网 IP：任一成功即用（5s 超时，全部失败则弱化处理）
async fn pub_ip_api() -> Option<String> {
    const APIS: &[&str] = &[
        "https://api.ipify.org",
        "https://4.ipw.cn",
        "https://api.ip.sb/ip",
        "https://ifconfig.me/ip",
    ];
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let jobs = APIS.iter().map(|u| {
        let client = client.clone();
        async move {
            client
                .get(*u)
                .header("user-agent", "BIT/1.0")
                .send()
                .await
                .ok()?
                .text()
                .await
                .ok()
        }
    });
    let results = futures_util::future::join_all(jobs).await;
    results.into_iter().flatten().find_map(|t| {
        let t = t.trim().to_string();
        // 粗验 IPv4 形态，避免拿到 HTML 报错页
        (t.parse::<std::net::Ipv4Addr>().is_ok()).then_some(t)
    })
}

/// 指纹原文（注册时刻定版；v1 前缀留算法演进空间）
fn fp_raw(fp: &DeviceFp, pub_ip: &str) -> String {
    format!("v1|{}|{}|{}|{}|{}", fp.cpu, fp.gpu, fp.ram_gb, fp.board_serial, pub_ip)
}

fn fp_hash16(raw: &str) -> String {
    Sha256::digest(raw.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 设备 key 派生：HMAC(FP_MASTER, raw|registered_at) 前 16 字节 hex。
/// registered_at 绑入派生：同指纹不同时刻注册得到不同 key（时间戳入钥）
fn derive_key(raw: &str, registered_at: i64) -> String {
    let material = format!("{raw}|{registered_at}");
    let mac = crate::security::hmac_sha256(
        crate::security::FP_MASTER.as_bytes(),
        material.as_bytes(),
    );
    let hex: String = mac.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{DEVICE_KEY_PREFIX}{hex}")
}

/// 信道签名材料：设备 key 的加盐 SHA256 前 16 hex（canonical 末段）。
/// 泄露面控制：签名里出现的是单向 hash 片段，不回传完整设备 key
pub fn sig_material(device_key: &str) -> String {
    Sha256::digest(format!("{SIG_MATERIAL_SALT}{device_key}").as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 幂等注册：已有设备 key 直接复用（稳定锚点）；首次启动采集指纹 + 公网 IP +
/// 时间戳派生 key，落盘并审计。返回 (device_key, registered_at)
pub async fn ensure_device_key(ctx: &Arc<Ctx>) -> Result<(String, i64), String> {
    {
        let cfg = ctx.config.lock().unwrap();
        if let Some(k) = cfg.device_key.as_ref().filter(|s| !s.is_empty()) {
            return Ok((k.clone(), cfg.device_registered_at.unwrap_or(0)));
        }
    }
    // 阻塞采集（system_profiler 可能 1-2s）放 blocking 线程
    let fp = tokio::task::spawn_blocking(collect)
        .await
        .map_err(|e| e.to_string())?;
    let pub_ip = pub_ip_api().await.unwrap_or_else(|| "unresolved".into());
    let registered_at = chrono::Utc::now().timestamp();
    let raw = fp_raw(&fp, &pub_ip);
    let key = derive_key(&raw, registered_at);
    let fp_hash = fp_hash16(&raw);
    {
        let mut c = ctx.config.lock().unwrap();
        c.device_key = Some(key.clone());
        c.device_registered_at = Some(registered_at);
        c.device_fp_hash = Some(fp_hash.clone());
        c.revision += 1;
    }
    ctx.save_config();
    crate::audit::record(
        ctx,
        "local-user",
        "device.registered",
        "config",
        json!({
            "registered_at": registered_at,
            "fp_hash": fp_hash,
            "pub_ip": pub_ip,
            "cpu": fp.cpu,
            "ram_gb": fp.ram_gb,
            "note": "硬件指纹+公网IP+时间戳派生设备凭证（原始指纹不落盘）"
        }),
        true,
    );
    Ok((key, registered_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_stable_and_ts_bound() {
        let fp = DeviceFp { cpu: "Apple M1".into(), gpu: "M1".into(), ram_gb: 16, board_serial: "XYZ".into() };
        let raw = fp_raw(&fp, "1.2.3.4");
        let k1 = derive_key(&raw, 1700000000);
        let k2 = derive_key(&raw, 1700000000);
        let k3 = derive_key(&raw, 1700000001);
        assert_eq!(k1, k2);
        assert_ne!(k1, k3, "时间戳必须参与派生");
        assert!(k1.starts_with("bitdev_") && k1.len() == 32 + DEVICE_KEY_PREFIX.len());
    }

    #[test]
    fn sig_material_stable() {
        let a = sig_material("bitdev_abcdef");
        let b = sig_material("bitdev_abcdef");
        let c = sig_material("bitdev_other");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn fp_hash_differs_by_ip() {
        let fp = DeviceFp { cpu: "x".into(), gpu: "y".into(), ram_gb: 8, board_serial: "z".into() };
        assert_ne!(fp_hash16(&fp_raw(&fp, "1.1.1.1")), fp_hash16(&fp_raw(&fp, "2.2.2.2")));
    }
}
