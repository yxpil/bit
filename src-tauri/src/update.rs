use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::state::Ctx;
use tauri::Emitter;

/// 自动更新：检测（镜像 → GitHub API，BIT_FAKE_UPDATE_URL 测试注入在首位）
/// → 启动后台静默下载 → 退出（托盘）或点击更新按钮时换装，
/// 下次启动即新版本（macOS .app 内嵌资源随二进制一同更新，等效热更新内容）。

#[derive(Serialize, Clone, Debug)]
pub struct LatestInfo {
    pub version: String,
    pub notes: String,
    pub url: String,
    /// latest.json 的 assets 平台映射（无则用 GitHub 直链回退）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assets: Option<serde_json::Value>,
}

/// 检测最新版本。BIT_FAKE_UPDATE_URL 指向的源优先（e2e 测试注入）。
pub async fn fetch_latest() -> Result<LatestInfo, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .build()
        .map_err(|e| e.to_string())?;
    let mut sources: Vec<String> = Vec::new();
    if let Ok(fake) = std::env::var("BIT_FAKE_UPDATE_URL") {
        sources.push(fake);
    }
    sources.push("https://yxpil.github.io/bit/latest.json".into());
    sources.push("https://osbt.space/latest.json".into());

    for src in &sources {
        if let Ok(resp) = client.get(src).send().await {
            if let Ok(j) = resp.json::<serde_json::Value>().await {
                if let Some(ver) = j.get("version").and_then(|x| x.as_str()) {
                    return Ok(LatestInfo {
                        version: ver.trim_start_matches('v').to_string(),
                        notes: j
                            .get("notes")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        url: j
                            .get("url")
                            .and_then(|x| x.as_str())
                            .unwrap_or("https://osbt.space")
                            .to_string(),
                        assets: j.get("assets").cloned(),
                    });
                }
            }
        }
    }
    // 回退：GitHub API（拿 tag_name / body / html_url）
    if let Ok(v) = client
        .get("https://api.github.com/repos/yxpil/bit/releases/latest")
        .header("User-Agent", "BIT-Agent")
        .send()
        .await
    {
        if let Ok(j) = v.json::<serde_json::Value>().await {
            let ver = j
                .get("tag_name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim_start_matches('v')
                .to_string();
            if !ver.is_empty() {
                return Ok(LatestInfo {
                    version: ver,
                    notes: j
                        .get("body")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(300)
                        .collect(),
                    url: j
                        .get("html_url")
                        .and_then(|x| x.as_str())
                        .unwrap_or("https://osbt.space")
                        .to_string(),
                    assets: None,
                });
            }
        }
    }
    Err("暂时无法获取最新版本信息".into())
}

/// 从 latest.json 选出当前平台的安装包下载地址：
/// 优先 assets 映射；缺失时按发布资产命名规则构造 GitHub 直链。
/// 返回 (文件名, URL)。 exotic 平台返回 None（不支持自动更新）。
pub fn pick_asset(latest: &serde_json::Value) -> Option<(String, String)> {
    let key = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows-x64",
        ("windows", "aarch64") => "windows-arm64",
        ("macos", "aarch64") => "macos-arm64",
        ("macos", "x86_64") => "macos-x64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        _ => return None,
    };
    if let Some(u) = latest
        .get("assets")
        .and_then(|a| a.get(key))
        .and_then(|v| v.as_str())
    {
        let name = u.rsplit('/').next().unwrap_or("bit-update.bin").to_string();
        return Some((name, u.to_string()));
    }
    // 回退：按 CI 发布资产的命名规则构造直链
    let v = latest.get("version").and_then(|x| x.as_str())?;
    let name = match key {
        "windows-x64" => format!("BIT_{v}_x64-setup.exe"),
        "windows-arm64" => format!("BIT_{v}_aarch64-setup.exe"),
        "macos-arm64" => format!("BIT_{v}_aarch64-app.zip"),
        "macos-x64" => format!("BIT_{v}_x64-app.zip"),
        "linux-x64" => format!("BIT_{v}_amd64.AppImage"),
        "linux-arm64" => format!("BIT_{v}_aarch64.AppImage"),
        _ => return None,
    };
    Some((
        name.clone(),
        format!("https://github.com/yxpil/bit/releases/download/v{v}/{name}"),
    ))
}

/// 升级暂存目录：data_dir/upgrade/
fn upgrade_dir(ctx: &Ctx) -> PathBuf {
    ctx.data_dir.join("upgrade")
}

/// 已下载状态文件：{version, file, state, time}
fn state_path(ctx: &Ctx) -> PathBuf {
    upgrade_dir(ctx).join("state.json")
}

pub fn read_state(ctx: &Ctx) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(state_path(ctx)).ok()?;
    serde_json::from_str(&s).ok()
}

fn save_state(ctx: &Ctx, v: serde_json::Value) {
    let _ = std::fs::create_dir_all(upgrade_dir(ctx));
    let _ = std::fs::write(state_path(ctx), serde_json::to_string_pretty(&v).unwrap_or_default());
}

/// 静默下载当前平台的更新包到升级目录（已存在且版本一致则直接复用，不重复下载）。
/// 完成后写 state.json 并返回状态。
pub async fn download_update(ctx: &Arc<Ctx>) -> Result<serde_json::Value, String> {
    let latest = fetch_latest().await?;
    let current = env!("CARGO_PKG_VERSION");
    if !crate::commands::version_gt(&latest.version, current) {
        return Ok(json_status("none", &latest, None));
    }
    let Some((name, url)) = pick_asset(&serde_json::json!({
        "version": latest.version,
        "assets": latest.assets.clone().unwrap_or(serde_json::Value::Null),
    })) else {
        return Err("当前平台暂不支持自动更新，请手动下载".into());
    };
    let dest = upgrade_dir(ctx).join(&name);
    // 缓存命中：同版本同名文件已存在且非空
    if dest.exists() {
        if let Some(st) = read_state(ctx) {
            if st["version"] == latest.version.as_str()
                && st["file"] == name.as_str()
                && std::fs::metadata(&dest).map(|m| m.len() > 0).unwrap_or(false)
            {
                return Ok(json_status("downloaded", &latest, Some(&dest)));
            }
        }
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("User-Agent", "BIT-Agent")
        .send()
        .await
        .map_err(|e| format!("下载失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("下载失败: HTTP {status}"));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("下载失败: {e}"))?;
    if bytes.is_empty() {
        return Err("下载失败: 内容为空".into());
    }
    let _ = std::fs::create_dir_all(upgrade_dir(ctx));
    std::fs::write(&dest, &bytes).map_err(|e| format!("写入失败: {e}"))?;
    let st = serde_json::json!({
        "version": latest.version,
        "file": name,
        "state": "downloaded",
        "time": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    });
    save_state(ctx, st);
    crate::audit::record(ctx, "local-app", "app.update.download", &latest.version, serde_json::json!({ "file": name, "bytes": bytes.len() }), true);
    Ok(json_status("downloaded", &latest, Some(&dest)))
}

fn json_status(state: &str, latest: &LatestInfo, file: Option<&Path>) -> serde_json::Value {
    serde_json::json!({
        "state": state,
        "version": latest.version,
        "notes": latest.notes,
        "url": latest.url,
        "file": file.map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
    })
}

/// 原子换装核心：目标文件改名留作备份，再把新文件放到位。
/// 返回备份路径。Linux(AppImage) 专用 + 单测验证；纯文件操作。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn replace_binary(target: &Path, new_file: &Path) -> std::io::Result<PathBuf> {
    let backup = target.with_extension(format!(
        "old-{}",
        chrono::Local::now().timestamp_millis()
    ));
    if target.exists() {
        std::fs::rename(target, &backup)?;
    }
    match std::fs::copy(new_file, target) {
        Ok(_) => Ok(backup),
        Err(e) => {
            // 回滚：换装失败必须恢复原文件，避免把可执行文件弄丢
            if backup.exists() {
                let _ = std::fs::rename(&backup, target);
            }
            Err(e)
        }
    }
}

/// 换装已下载的更新并（可选）重启。
/// - Windows：NSIS 安装器静默覆盖安装（/S），随后退出（安装器负责替换文件）
/// - macOS：app.zip 解包后整体替换 .app（ditto 保留签名），respawn 时重启
/// - Linux：AppImage 单文件换装，respawn 时重启
pub fn apply_update(ctx: &Arc<Ctx>, respawn: bool) -> Result<String, String> {
    let st = read_state(ctx).ok_or("没有已下载的更新")?;
    let state = st["state"].as_str().unwrap_or("");
    if state != "downloaded" {
        return Err("没有已下载的更新".into());
    }
    let file = upgrade_dir(ctx).join(st["file"].as_str().unwrap_or(""));
    if !file.exists() {
        return Err("更新包文件缺失".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    {
        // NSIS 静默安装：安装器自带文件替换逻辑，退出后由它完成
        let _ = exe;
        std::process::Command::new(&file)
            .arg("/S")
            .spawn()
            .map_err(|e| format!("无法启动安装器: {e}"))?;
        crate::audit::record(ctx, "local-app", "app.update.apply", st["version"].as_str().unwrap_or(""), serde_json::json!({ "via": "nsis" }), true);
        Ok("安装器已启动，BIT 即将退出并完成更新".into())
    }

    #[cfg(target_os = "macos")]
    {
        // 解包 app.zip → 找到 BIT.app → 整体替换当前 bundle（先挪走旧的作备份）
        let extract = upgrade_dir(ctx).join("extract");
        let _ = std::fs::remove_dir_all(&extract);
        std::fs::create_dir_all(&extract).map_err(|e| e.to_string())?;
        let out = std::process::Command::new("ditto")
            .args(["-x", "-k"])
            .arg(&file)
            .arg(&extract)
            .output()
            .map_err(|e| format!("解包失败: {e}"))?;
        if !out.status.success() {
            return Err(format!("解包失败: {}", String::from_utf8_lossy(&out.stderr)));
        }
        let new_app = find_app_dir(&extract).ok_or("解包内容中未找到 BIT.app")?;
        let cur_app = exe
            .ancestors()
            .find(|p| p.extension().map(|e| e == "app").unwrap_or(false))
            .map(|p| p.to_path_buf())
            .ok_or("当前运行的不是 .app 包结构")?;
        let backup = cur_app.with_extension("old-app");
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(&cur_app, &backup).map_err(|e| format!("备份失败: {e}"))?;
        if std::fs::rename(&new_app, &cur_app).is_err() {
            // 换装失败回滚
            let _ = std::fs::rename(&backup, &cur_app);
            return Err("换装失败（已回滚）".into());
        }
        crate::audit::record(ctx, "local-app", "app.update.apply", st["version"].as_str().unwrap_or(""), serde_json::json!({ "via": "app-bundle" }), true);
        if respawn {
            let _ = std::process::Command::new(&cur_app.join("Contents/MacOS/bit")).spawn();
        }
        Ok("更新完成".into())
    }

    #[cfg(target_os = "linux")]
    {
        // AppImage 单文件换装
        std::fs::copy(&file, upgrade_dir(ctx).join("bit.new"))
            .map_err(|e| format!("准备失败: {e}"))?;
        let staged = upgrade_dir(ctx).join("bit.new");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).ok();
        replace_binary(&exe, &staged).map_err(|e| format!("换装失败: {e}"))?;
        let _ = std::fs::remove_file(&staged);
        crate::audit::record(ctx, "local-app", "app.update.apply", st["version"].as_str().unwrap_or(""), serde_json::json!({ "via": "appimage" }), true);
        if respawn {
            let _ = std::process::Command::new(&exe).spawn();
        }
        Ok("更新完成".into())
    }

    // 其余平台：显式报错防止静默吞掉
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err("当前平台暂不支持自动更新".into())
    }
}

/// 在解包目录里递归找 BIT.app（zip 内可能有一层目录包裹）
fn find_app_dir(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p.extension().map(|x| x == "app").unwrap_or(false)
                        && p.join("Contents/MacOS").exists()
                    {
                        return Some(p);
                    }
                    stack.push(p);
                }
            }
        }
    }
    None
}

/// 启动后台自动更新：延时后检测 → 有新版本即静默下载（一次），发 update-state 事件。
/// 下载失败静默跳过（不打扰用户，pill 仍可手动点击打开下载页）。
pub async fn auto_update_task(app: tauri::AppHandle, ctx: Arc<Ctx>) {
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    let latest = match fetch_latest().await {
        Ok(l) => l,
        Err(_) => return,
    };
    if !crate::commands::version_gt(&latest.version, env!("CARGO_PKG_VERSION")) {
        return;
    }
    // 同版本已下载过就不再下载
    if let Some(st) = read_state(&ctx) {
        if st["version"] == latest.version.as_str() && st["state"] == "downloaded" {
            let _ = app.emit("update-state", st);
            return;
        }
    }
    if let Ok(status) = download_update(&ctx).await {
        let _ = app.emit("update-state", status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        // 借用 commands 的比较函数验证更新判定
        assert!(crate::commands::version_gt("0.5.0", "0.4.9"));
        assert!(crate::commands::version_gt("1.0.0", "0.9.9"));
        assert!(!crate::commands::version_gt("0.4.9", "0.4.9"));
        assert!(!crate::commands::version_gt("0.4.8", "0.4.9"));
        assert!(crate::commands::version_gt("v0.5.0", "0.4.9"));
    }

    #[test]
    fn pick_asset_from_map() {
        // assets 映射优先：当前平台键命中即用
        let latest = serde_json::json!({
            "version": "9.9.9",
            "assets": {
                "windows-x64": "http://x/BIT_9.9.9_x64-setup.exe",
                "windows-arm64": "http://x/BIT_9.9.9_aarch64-setup.exe",
                "macos-arm64": "http://x/BIT_9.9.9_aarch64-app.zip",
                "macos-x64": "http://x/BIT_9.9.9_x64-app.zip",
                "linux-x64": "http://x/BIT_9.9.9_amd64.AppImage",
                "linux-arm64": "http://x/BIT_9.9.9_aarch64.AppImage",
            }
        });
        let (name, url) = match pick_asset(&latest) {
            Some((n, u)) => (n, u),
            // exotic 平台（riscv64/loongarch64 等）不支持自动更新，返回 None 是正确行为
            None => return,
        };
        assert!(url.starts_with("http://x/"));
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => assert!(name.ends_with("_x64-setup.exe")),
            ("windows", "aarch64") => assert!(name.ends_with("_aarch64-setup.exe")),
            ("macos", "aarch64") => assert!(name.ends_with("_aarch64-app.zip")),
            ("macos", "x86_64") => assert!(name.ends_with("_x64-app.zip")),
            ("linux", "x86_64") => assert!(name.ends_with("_amd64.AppImage")),
            ("linux", "aarch64") => assert!(name.ends_with("_aarch64.AppImage")),
            _ => panic!("exotic 平台不支持自动更新"),
        }
    }

    #[test]
    fn pick_asset_fallback_constructs_github_url() {
        // 无 assets 映射：按发布命名规则构造 GitHub 直链
        let latest = serde_json::json!({ "version": "0.5.0" });
        let (name, url) = match pick_asset(&latest) {
            Some(x) => x,
            None => return, // exotic 平台
        };
        assert!(url == format!("https://github.com/yxpil/bit/releases/download/v0.5.0/{name}"));
        assert!(name.contains("0.5.0"));
    }

    #[test]
    fn replace_binary_swaps_and_keeps_backup() {
        let dir = std::env::temp_dir().join(format!("bit-swap-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let target = dir.join("app.bin");
        let new_file = dir.join("new.bin");
        std::fs::write(&target, b"OLD").unwrap();
        std::fs::write(&new_file, b"NEW-CONTENT").unwrap();
        let backup = replace_binary(&target, &new_file).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW-CONTENT");
        assert_eq!(std::fs::read(&backup).unwrap(), b"OLD");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_binary_rolls_back_on_failure() {
        let dir = std::env::temp_dir().join(format!("bit-swap-rb-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let target = dir.join("app.bin");
        std::fs::write(&target, b"OLD").unwrap();
        // 源文件不存在 → copy 失败 → 必须回滚还原
        let result = replace_binary(&target, &dir.join("missing.bin"));
        assert!(result.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
