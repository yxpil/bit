// yxpil · BIT
use futures_util::StreamExt;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Instant;

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

/// 进程内短缓存：检测按钮反复点击、开机自检等多触发点复用同一份 latest，
/// 避免每点一次就把镜像/GitHub/假源三处全打一遍（国内 6s×3 串行 ≈18s 体感迟钝）。
const LATEST_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
static LATEST_CACHE: std::sync::Mutex<Option<(LatestInfo, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// 检测最新版本。BIT_FAKE_UPDATE_URL 指向的源优先（e2e 测试注入）。
/// 三个源（fake + 镜像 + 备用域名）并发探测，取首个 2xx 且含 version 的响应；
/// 全部失败再回退 GitHub API。结果在 60s 内复用。
pub async fn fetch_latest() -> Result<LatestInfo, String> {
    // 1) 缓存命中：直接复用，省一次网络
    {
        let cache = LATEST_CACHE.lock().unwrap();
        if let Some((info, at)) = cache.as_ref() {
            if at.elapsed() < LATEST_CACHE_TTL {
                return Ok(info.clone());
            }
        }
    }

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

    // 2) 并发探测（之前是串行，首源挂时最坏要等 6s×3）
    let client = std::sync::Arc::new(client);
    let futs = sources.into_iter().map(|src| {
        let client = client.clone();
        async move {
            let resp = match client.get(&src).send().await {
                Ok(r) => r,
                Err(_) => return None,
            };
            if !resp.status().is_success() {
                return None;
            }
            let j: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(_) => return None,
            };
            let ver = j.get("version").and_then(|x| x.as_str())?;
            Some(LatestInfo {
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
            })
        }
    });
    let results = futures_util::future::join_all(futs).await;
    for info in results.into_iter().flatten() {
        *LATEST_CACHE.lock().unwrap() = Some((info.clone(), std::time::Instant::now()));
        return Ok(info);
    }

    // 3) 回退：GitHub API（拿 tag_name / body / html_url）
    if let Ok(v) = client
        .get("https://api.github.com/repos/yxpil/bit/releases/latest")
        .header("User-Agent", "BIT-Agent")
        .send()
        .await
    {
        if v.status().is_success() {
            if let Ok(j) = v.json::<serde_json::Value>().await {
                let ver = j
                    .get("tag_name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .trim_start_matches('v')
                    .to_string();
                if !ver.is_empty() {
                    let info = LatestInfo {
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
                    };
                    *LATEST_CACHE.lock().unwrap() = Some((info.clone(), std::time::Instant::now()));
                    return Ok(info);
                }
            }
        }
    }
    Err("暂时无法获取最新版本信息".into())
}

/// 可信下载主机白名单：latest.json 的 assets URL 只接受这些主机，
/// 防止 latest.json 被篡改时把任意主机的安装包喂给换装逻辑（HTTPS 之外一律拒绝）。
/// 注意 GitHub release 下载会 302 到 objects/release-assets.githubusercontent.com。
fn trusted_asset_url(url: &str) -> bool {
    trusted_asset_url_impl(url, std::env::var_os("BIT_FAKE_UPDATE_URL").is_some())
}

/// 纯函数实现。allow_loopback 仅由 e2e 注入变量 BIT_FAKE_UPDATE_URL 开启：
/// 此时允许 http(s)://127.0.0.1 / localhost 回环资产（本地 mock 源），
/// 生产未设该变量 → 严格 HTTPS 白名单，行为不变。
fn trusted_asset_url_impl(url: &str, allow_loopback: bool) -> bool {
    let rest = if let Some(r) = url.strip_prefix("https://") {
        r
    } else if allow_loopback {
        match url.strip_prefix("http://") {
            Some(r) => r,
            None => return false,
        }
    } else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    if allow_loopback {
        let bare = host.split(':').next().unwrap_or("");
        if matches!(bare, "127.0.0.1" | "localhost") {
            return true;
        }
    }
    matches!(
        host,
        "github.com"
            | "objects.githubusercontent.com"
            | "release-assets.githubusercontent.com"
            | "yxpil.github.io"
            | "osbt.space"
    )
}

/// 从 latest.json 选出当前平台的安装包下载地址：
/// 优先 assets 映射（仅可信主机）；缺失或不可信时按发布资产命名规则构造 GitHub 直链。
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
        if trusted_asset_url(u) {
            let name = u.rsplit('/').next().unwrap_or("bit-update.bin").to_string();
            return Some((name, u.to_string()));
        }
    }
    // 回退：按 CI 发布资产的命名规则构造直链
    let v = latest.get("version").and_then(|x| x.as_str())?;
    // 版本号只允许 数字+点（清单数据不可信，防止 "1.2/../../x" 之类路径注入）
    if v.is_empty()
        || !v
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.')
        || v.contains("..")
        || v.starts_with('.')
    {
        return None;
    }
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

/// 下载互斥：自动静默下载与「关于」里的手动下载可能同时触发，
/// 用进程内原子开关挡住并发写同一份更新包（本函数是两条路径的唯一入口）。
static DL_BUSY: AtomicBool = AtomicBool::new(false);

/// 下载当前平台的更新包到升级目录（已存在且版本一致则直接复用，不重复下载）。
/// 流式落盘，期间每 ~200ms 广播一次 update-progress（含已下载字节/总量/瞬时速度）。
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
    if DL_BUSY.swap(true, AtomicOrdering::SeqCst) {
        return Err("更新已在下载中，请稍候".into());
    }
    let result = stream_to_file(ctx, &latest, &url, &dest, &name).await;
    DL_BUSY.store(false, AtomicOrdering::SeqCst);
    result
}

/// 流式下载核心：分块写盘 + 进度事件（speed 单位：字节/秒）。
/// 不做并发处理，由调用方（download_update）持有互斥标志。
async fn stream_to_file(
    ctx: &Arc<Ctx>,
    latest: &LatestInfo,
    url: &str,
    dest: &Path,
    name: &str,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .header("User-Agent", "BIT-Agent")
        .send()
        .await
        .map_err(|e| format!("下载失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("下载失败: HTTP {status}"));
    }
    let total = resp.content_length().unwrap_or(0);
    let _ = std::fs::create_dir_all(upgrade_dir(ctx));
    let mut file = std::fs::File::create(dest).map_err(|e| format!("写入失败: {e}"))?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_at = Instant::now();
    let mut last_bytes: u64 = 0;
    while let Some(item) = stream.next().await {
        let chunk = item.map_err(|e| format!("下载失败: {e}"))?;
        file.write_all(&chunk).map_err(|e| format!("写入失败: {e}"))?;
        downloaded += chunk.len() as u64;
        // 进度事件节流：~200ms 一条，避免高频刷新
        let el = last_at.elapsed();
        if el.as_millis() >= 200 {
            let speed = if el.as_secs_f64() > 0.0 {
                ((downloaded - last_bytes) as f64 / el.as_secs_f64()) as u64
            } else {
                0
            };
            last_bytes = downloaded;
            last_at = Instant::now();
            let _ = ctx.app.emit(
                "update-progress",
                serde_json::json!({
                    "state": "downloading",
                    "version": latest.version,
                    "file": name,
                    "downloaded": downloaded,
                    "total": total,
                    "speed": speed,
                }),
            );
        }
    }
    file.flush().map_err(|e| format!("写入失败: {e}"))?;
    drop(file);
    if downloaded == 0 {
        let _ = std::fs::remove_file(dest);
        return Err("下载失败: 内容为空".into());
    }
    let st = serde_json::json!({
        "version": latest.version,
        "file": name,
        "state": "downloaded",
        "time": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    });
    save_state(ctx, st);
    crate::audit::record(ctx, "local-app", "app.update.download", &latest.version, serde_json::json!({ "file": name, "bytes": downloaded }), true);
    Ok(json_status("downloaded", latest, Some(dest)))
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
/// 版本三元组解析（"0.6.2" → (0,6,2)；容忍 v 前缀与非数字尾巴）
fn ver_tuple(v: &str) -> (u64, u64, u64) {
    let mut it = v
        .trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|seg| {
            seg.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
        });
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

pub fn apply_update(ctx: &Arc<Ctx>, respawn: bool) -> Result<String, String> {
    let st = read_state(ctx).ok_or("没有已下载的更新")?;
    let state = st["state"].as_str().unwrap_or("");
    if state != "downloaded" {
        return Err("没有已下载的更新".into());
    }
    // 防降级：暂存包版本不高于当前版本时拒装并清除暂存。
    // （曾发生过：退出时把数小时前的旧暂存包装回去，新功能整体蒸发）
    let staged = st["version"].as_str().unwrap_or("").to_string();
    let cur = ctx.app.package_info().version.to_string();
    if ver_tuple(&staged) <= ver_tuple(&cur) {
        let _ = std::fs::remove_dir_all(upgrade_dir(ctx));
        return Err(format!(
            "暂存更新版本 {staged} 不高于当前 {cur}，已丢弃（防降级保护）"
        ));
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
        // R 后缀（0.6R1/0.6R2…）：R 后数字作为下一段，保证 0.6 < 0.6R1 < 0.6R2
        assert!(crate::commands::version_gt("0.6R1", "0.5.36"));
        assert!(crate::commands::version_gt("0.6R2", "0.6R1"));
        assert!(crate::commands::version_gt("0.6R1", "0.6"));
        assert!(!crate::commands::version_gt("0.6R1", "0.6R1"));
        assert!(!crate::commands::version_gt("0.6R1", "0.6R2"));
        assert!(!crate::commands::version_gt("0.5.36", "0.6R1"));
        assert!(crate::commands::version_gt("v0.6R1", "0.6"));
        // 不带 R 时 R<...> 段被忽略，仍按基础三段比较
        assert!(crate::commands::version_gt("0.6.1", "0.6"));
    }

    #[test]
    fn pick_asset_from_map() {
        // assets 映射优先：当前平台键命中且主机可信即用
        let latest = serde_json::json!({
            "version": "9.9.9",
            "assets": {
                "windows-x64": "https://github.com/yxpil/bit/releases/download/v9.9.9/BIT_9.9.9_x64-setup.exe",
                "windows-arm64": "https://github.com/yxpil/bit/releases/download/v9.9.9/BIT_9.9.9_aarch64-setup.exe",
                "macos-arm64": "https://github.com/yxpil/bit/releases/download/v9.9.9/BIT_9.9.9_aarch64-app.zip",
                "macos-x64": "https://github.com/yxpil/bit/releases/download/v9.9.9/BIT_9.9.9_x64-app.zip",
                "linux-x64": "https://github.com/yxpil/bit/releases/download/v9.9.9/BIT_9.9.9_amd64.AppImage",
                "linux-arm64": "https://github.com/yxpil/bit/releases/download/v9.9.9/BIT_9.9.9_aarch64.AppImage",
            }
        });
        let (name, url) = match pick_asset(&latest) {
            Some((n, u)) => (n, u),
            // exotic 平台（riscv64/loongarch64 等）不支持自动更新，返回 None 是正确行为
            None => return,
        };
        assert!(url.starts_with("https://github.com/"));
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
    fn pick_asset_rejects_untrusted_host() {
        // latest.json 被篡改指向任意主机（http、evil 域名、仿冒后缀）→ 必须回退到构造的 GitHub 直链
        for evil in [
            "http://x/BIT_9.9.9_x64-setup.exe",
            "https://evil.com/BIT_9.9.9_x64-setup.exe",
            "https://github.com.evil.com/BIT_9.9.9_x64-setup.exe",
            "ftp://github.com/BIT_9.9.9_x64-setup.exe",
        ] {
            let latest = serde_json::json!({ "version": "9.9.9", "assets": { "windows-x64": evil } });
            if let Some((_, url)) = pick_asset(&latest) {
                assert!(url.starts_with("https://github.com/yxpil/bit/releases/"), "evil={evil} url={url}");
            }
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
    fn trusted_url_loopback_only_with_fake_env() {
        // e2e 注入（allow_loopback=true）：仅放行回环 http/https
        assert!(trusted_asset_url_impl("http://127.0.0.1:9903/asset.bin", true));
        assert!(trusted_asset_url_impl("https://127.0.0.1/asset.bin", true));
        assert!(trusted_asset_url_impl("http://localhost:9903/asset.bin", true));
        // 即便注入变量存在，非回环主机仍被拒绝
        assert!(!trusted_asset_url_impl("http://evil.com/asset.bin", true));
        assert!(!trusted_asset_url_impl("https://evil.com/asset.bin", true));
        // 生产（allow_loopback=false）：回环 http 一律拒绝，HTTPS 白名单照常生效
        assert!(!trusted_asset_url_impl("http://127.0.0.1:9903/asset.bin", false));
        assert!(!trusted_asset_url_impl("http://github.com/a.bin", false));
        assert!(trusted_asset_url_impl("https://github.com/yxpil/bit/releases/download/v1/x", false));
        assert!(trusted_asset_url_impl("https://osbt.space/bit.dmg", false));
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
