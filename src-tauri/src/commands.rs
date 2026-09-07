// yxpil · BIT
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

use crate::state::Ctx;

fn ctx<'a>(state: State<'a, Arc<Ctx>>) -> Arc<Ctx> {
    state.inner().clone()
}

pub fn estimate_context_tokens(ctx: &Arc<Ctx>, session_id: &str, convo: &[crate::ai::ChatMessage]) -> usize {
    let _ = session_id;
    // 探测缓存按提供方记（能否原生调工具是「哪家端点」的属性）
    let probe_key = ctx
        .ai_config
        .lock()
        .unwrap()
        .active()
        .map(|p| p.id.clone())
        .unwrap_or_default();
    let native_mode = ctx.native_probe.lock().unwrap().get(&probe_key).copied() != Some(false);
    let convo_chars: usize = convo
        .iter()
        .map(|m| m.role.chars().count() + m.content.chars().count() + 8)
        .sum();
    let tool_chars = if native_mode {
        serde_json::to_string(&crate::ai::native_tool_defs(ctx))
            .unwrap_or_default()
            .chars()
            .count()
    } else {
        0
    };
    (convo_chars + tool_chars).div_ceil(2)
}

// ---------- 概览 ----------

/// 无界面模式（BIT_HEADLESS=1）：E2E/专项测试用，窗口保持隐藏不弹到前台
#[tauri::command]
pub fn is_headless() -> bool {
    std::env::var("BIT_HEADLESS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 前端挂载信号：App 首帧成功渲染后由前端调用，落审计供 CI 冒烟断言
/// 「渲染树完整挂载」（页面渲染崩溃时本事件缺席 → Windows 冒烟判失败，拦住黑屏包）
#[tauri::command]
pub fn ui_mounted(state: State<'_, Arc<Ctx>>) {
    crate::audit::record(
        &ctx(state),
        "local-app",
        "ui.mounted",
        "BIT",
        json!({}),
        true,
    );
}

/// 本进程内存占用（字节）：页眉仪表盘展示，前端每 3 秒轮询
#[tauri::command]
pub fn mem_usage() -> u64 {
    use sysinfo::{ProcessesToUpdate, System};
    let mut sys = System::new();
    let pid = sysinfo::Pid::from_u32(std::process::id());
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map(|p| p.memory()).unwrap_or(0)
}

#[tauri::command]
pub fn get_overview(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let cfg = ctx.config.lock().unwrap();
    json!({
        "tool_count": ctx.tools.lock().unwrap().len(),
        "memory_count": ctx.memories.lock().unwrap().len(),
        "skill_count": ctx.skills.lock().unwrap().len(),
        "goal_count": ctx.goals.lock().unwrap().iter().filter(|g| g.status == "active").count(),
        "todo_count": ctx.todos.lock().unwrap().iter().filter(|t| t.status != "completed").count(),
        "audit_count": ctx.audit.lock().unwrap().len(),
        "remote": {
            "enabled": cfg.remote_enabled,
            "addr": cfg.listen_addr(),
        },
        "ai_configured": ctx.ai_config.lock().unwrap().is_configured(),
        "autopilot_running": ctx.autopilot_running.load(Ordering::SeqCst),
    })
}

// ---------- 工具 ----------

#[tauri::command]
pub fn list_tools(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    json!({ "tools": ctx(state).tools.lock().unwrap().clone() })
}

#[tauri::command]
pub async fn register_tool(
    state: State<'_, Arc<Ctx>>,
    name: String,
    description: String,
    url: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    if url.trim().is_empty() {
        return Err("回调 URL 不能为空".into());
    }
    let tool = crate::registry::register(
        &ctx,
        &name,
        &description,
        json!({"type": "object", "properties": {}, "additionalProperties": true}),
        crate::registry::ToolKind::Remote { url: url.trim().to_string() },
        "local-user",
    )?;
    crate::audit::record(&ctx, "local-user", "tool.register", &tool.name, json!({ "url": url }), true);
    Ok(json!({ "tool": tool }))
}

#[tauri::command]
pub fn remove_tool(state: State<'_, Arc<Ctx>>, id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let name = {
        let tools = ctx.tools.lock().unwrap();
        tools.iter().find(|t| t.id == id).map(|t| t.name.clone())
    };
    let removed = crate::registry::remove(&ctx, &id)?;
    crate::audit::record(&ctx, "local-user", "tool.remove", &name.unwrap_or_default(), json!({}), true);
    Ok(json!({ "removed": removed }))
}

#[tauri::command]
pub fn set_tool_enabled(
    state: State<'_, Arc<Ctx>>,
    id: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let now = crate::registry::set_enabled(&ctx, &id, enabled)?;
    Ok(json!({ "id": id, "enabled": now }))
}

#[tauri::command]
pub async fn invoke_tool(
    state: State<'_, Arc<Ctx>>,
    id: String,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    crate::registry::invoke(&ctx, &id, params, "local-user", None).await
}

// 注册脚本工具：把一段 JS/PY 代码沉淀为常驻工具，由本机解释器执行
#[tauri::command]
pub fn register_script_tool(
    state: State<'_, Arc<Ctx>>,
    name: String,
    description: String,
    runtime: String,
    code: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    if code.trim().is_empty() {
        return Err("脚本代码不能为空".into());
    }
    if crate::runtime::get(&ctx, &runtime).is_none() {
        return Err(format!("解释器 `{runtime}` 未注册"));
    }
    let tool = crate::registry::register(
        &ctx,
        &name,
        &description,
        json!({"type": "object", "properties": {}, "additionalProperties": true}),
        crate::registry::ToolKind::Interpreter { runtime: runtime.clone(), code },
        "local-user",
    )?;
    crate::audit::record(&ctx, "local-user", "tool.register", &tool.name, json!({ "runtime": runtime }), true);
    Ok(json!({ "tool": tool }))
}

// ---------- 解释器 / 运行时 ----------

#[tauri::command]
pub fn list_runtimes(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    json!({ "runtimes": ctx(state).runtimes.lock().unwrap().clone() })
}

#[tauri::command]
pub fn refresh_runtimes(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let list = crate::runtime::refresh(&ctx);
    crate::audit::record(&ctx, "local-user", "runtime.refresh", "detect", json!({ "count": list.len() }), true);
    json!({ "runtimes": list })
}

#[tauri::command]
pub fn add_runtime(
    state: State<'_, Arc<Ctx>>,
    id: String,
    name: String,
    path: String,
    lang: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let rt = crate::runtime::add_manual(&ctx, &id, &name, &path, &lang)?;
    crate::audit::record(&ctx, "local-user", "runtime.add", &rt.id, json!({ "path": rt.path }), true);
    Ok(json!({ "runtime": rt }))
}

#[tauri::command]
pub fn remove_runtime(state: State<'_, Arc<Ctx>>, id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    crate::runtime::remove(&ctx, &id)?;
    crate::audit::record(&ctx, "local-user", "runtime.remove", &id, json!({}), true);
    Ok(json!({ "removed": id }))
}

// 暂停 / 启用解释器：暂停后 AI 不能用它执行代码或注册工具
#[tauri::command]
pub fn set_runtime_enabled(
    state: State<'_, Arc<Ctx>>,
    id: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let now = crate::runtime::set_enabled(&ctx, &id, enabled)?;
    crate::audit::record(
        &ctx,
        "local-user",
        if now { "runtime.enable" } else { "runtime.disable" },
        &id,
        json!({ "enabled": now }),
        true,
    );
    Ok(json!({ "id": id, "enabled": now }))
}

// 直接用某个解释器跑一段代码（不落地为工具），用于测试
#[tauri::command]
pub async fn run_script(
    state: State<'_, Arc<Ctx>>,
    runtime: String,
    code: String,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let runtime2 = runtime.clone();
    let code2 = code.clone();
    let ctx2 = ctx.clone();
    let handle = tauri::async_runtime::spawn_blocking(move || {
        crate::script_runtime::run(&ctx2, &runtime2, &code2, &params)
    });
    let result = match tokio::time::timeout(std::time::Duration::from_secs(30), handle).await {
        Ok(res) => res.map_err(|e| format!("脚本任务失败: {e}"))?,
        Err(_) => Err("脚本执行超时（30 秒）".into()),
    };
    crate::audit::record(&ctx, "local-user", "script.run", &runtime, json!({ "ok": result.is_ok() }), result.is_ok());
    result
}

// ---------- 审计 ----------

#[tauri::command]
pub fn list_audit(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let log = ctx.audit.lock().unwrap();
    let mut entries = log.clone();
    entries.reverse();
    json!({ "entries": entries })
}

/// 清空审计日志；清空动作本身记一条，保证操作可追溯
#[tauri::command]
pub fn clear_audit(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    crate::audit::clear(&ctx);
    crate::audit::record(&ctx, "local-user", "audit.clear", "audit", json!({}), true);
    Ok(json!({ "cleared": true }))
}

/// 删除单条审计记录；删除动作本身记一条
#[tauri::command]
pub fn delete_audit_entry(state: State<'_, Arc<Ctx>>, id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    if !crate::audit::delete(&ctx, &id) {
        return Err("记录不存在".into());
    }
    crate::audit::record(&ctx, "local-user", "audit.delete", &id, json!({}), true);
    Ok(json!({ "deleted": true }))
}

// ---------- 远程访问 ----------

#[tauri::command]
pub fn get_remote_config(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let cfg = ctx.config.lock().unwrap();
    json!({
        "remote_enabled": cfg.remote_enabled,
        "host": cfg.host,
        "port": cfg.port,
        "client_key": cfg.client_key,
        "access_password": cfg.access_password.clone().unwrap_or_default(),
        "password_enabled": cfg.password_enabled,
        "cloud_relay_url": cfg.cloud_relay_url.clone().unwrap_or_default(),
        "revision": cfg.revision,
    })
}

#[tauri::command]
pub async fn save_remote_config(
    state: State<'_, Arc<Ctx>>,
    remote_enabled: bool,
    host: String,
    port: u16,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    {
        let mut cfg = ctx.config.lock().unwrap();
        cfg.remote_enabled = remote_enabled;
        // 归一化主机输入：剥 IPv6 方括号 + 拒绝非 IP/域名形态（绑定时由 join_host_port 加回括号）
        let host = crate::config::normalize_host(&host)?;
        if port < 1024 {
            return Err("端口需不小于 1024".into());
        }
        cfg.host = host;
        cfg.port = port;
        cfg.revision += 1; // 每次保存自动递增版本号
    }
    // save_config 内部会再取 config 锁：std Mutex 不可重入，持锁调用同线程二次
    // 加锁在 macOS 上直接死锁（主线程卡死整个程序）。必须出锁后再落盘。
    ctx.save_config();
    crate::audit::record(&ctx, "local-user", "remote.save", "config", json!({ "revision": ctx.config.lock().unwrap().revision }), true);
    let addr = crate::http_api::restart_server(&ctx).await?;
    // 远程地址变化，同步托盘菜单显示
    crate::tray::refresh(&ctx.app);
    Ok(json!({ "addr": addr }))
}

/// 远程服务运行状态：供前端启动时查询端口是否被占用自动切换（事件可能早于 JS 监听而丢失）
#[tauri::command]
pub fn get_remote_status(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let cfg = ctx.config.lock().unwrap();
    let switched_from = *ctx.port_switch.lock().unwrap();
    Ok(json!({
        "enabled": cfg.remote_enabled,
        "addr": cfg.listen_addr(),
        "switched_from": switched_from,
    }))
}

/// 幻觉防护阈值（设置页读写）：word_repeat_max=单回复词重复上限 / tool_loop_max=单回合工具轮上限，0=关闭
#[tauri::command]
pub fn get_guard_limits(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let cfg = ctx.config.lock().unwrap();
    json!({ "word_repeat_max": cfg.word_repeat_max, "tool_loop_max": cfg.tool_loop_max })
}

#[tauri::command]
pub fn set_guard_limits(
    state: State<'_, Arc<Ctx>>,
    word_repeat_max: u32,
    tool_loop_max: u32,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    {
        let mut cfg = ctx.config.lock().unwrap();
        cfg.word_repeat_max = word_repeat_max.min(10000);
        cfg.tool_loop_max = tool_loop_max.min(10000);
    }
    ctx.save_config();
    crate::audit::record(
        &ctx,
        "local-user",
        "guard.limits",
        "set",
        json!({ "word_repeat_max": word_repeat_max, "tool_loop_max": tool_loop_max }),
        true,
    );
    Ok(json!({ "ok": true }))
}

/// 云中继地址（手机远程 App 用）：对称 NAT 无法直连时改连该地址
#[tauri::command]
pub fn save_cloud_relay(state: State<'_, Arc<Ctx>>, url: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let url = url.trim().trim_end_matches('/').to_string();
    if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("云中继地址需以 http:// 或 https:// 开头".into());
    }
    {
        let mut cfg = ctx.config.lock().unwrap();
        cfg.cloud_relay_url = if url.is_empty() { None } else { Some(url) };
        cfg.revision += 1;
    }
    ctx.save_config();
    crate::audit::record(&ctx, "local-user", "remote.cloud_relay", "set", json!({}), true);
    Ok(json!({ "ok": true }))
}

/// 网络探测：LAN/公网候选地址 + NAT 粗判（远程二维码数据源）
#[tauri::command]
pub async fn get_lan_info(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let stun = ctx.config.lock().unwrap().stun_servers.clone();
    Ok(crate::netinfo::lan_probe(stun.as_deref()).await)
}

/// 自定 STUN 服务器列表（host:port，逗号/分号分隔或数组）：整体替换内置免费列表，
/// 空列表 = 恢复默认。NAT 探测即时生效（下次打开二维码/探测即用新列表）
#[tauri::command]
pub fn save_stun_servers(state: State<'_, Arc<Ctx>>, servers: Vec<String>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    // 展平：数组元素本身也允许逗号/分号分隔（前端单输入框直接整串传进来）
    let mut list: Vec<String> = Vec::new();
    for item in &servers {
        for part in item.split([',', ';', '，', '；']) {
            let p = part.trim().trim_end_matches('/').to_string();
            if !p.is_empty() && !list.contains(&p) {
                list.push(p);
            }
        }
    }
    {
        let mut cfg = ctx.config.lock().unwrap();
        cfg.stun_servers = if list.is_empty() { None } else { Some(list) };
        cfg.revision += 1;
    }
    ctx.save_config();
    crate::audit::record(&ctx, "local-user", "remote.stun_servers", "set", json!({}), true);
    Ok(json!({ "ok": true }))
}

/// 二维码 payload（Tauri get_remote_qr 与 HTTP /api/qr 共用）。v2：新增 128 位识别码 rid
/// 与三种连接方式 methods（局域网直连 / IPv6 直连 / 云中继），手机端按 NAT 类型智能择路：
/// 局域网 → 直连；NAT1（锥形）且有全球 IPv6 → IPv6 直连；对称 NAT（NAT3/4）→ 云中继。
/// rid 懒生成：首次查看二维码时生成 128 位随机数并持久化，作为中继路由 + 访问凭据。
pub async fn qr_payload(ctx: &Arc<Ctx>) -> Result<serde_json::Value, String> {
    // 128 位识别码：只生成一次（避免每次探测漂移导致中继路由失效）
    let rid = {
        let mut c = ctx.config.lock().unwrap();
        if c.relay_id.is_empty() {
            let id = crate::relay::gen_relay_id();
            c.relay_id = id.clone();
            c.revision += 1;
            c.save(&ctx.data_dir);
            crate::audit::record(ctx, "local-app", "remote.relay_id", "generate", json!({}), true);
            id
        } else {
            c.relay_id.clone()
        }
    };
    let cfg = ctx.config.lock().unwrap().clone();
    let stun = cfg.stun_servers.clone();
    let probe = crate::netinfo::lan_probe(stun.as_deref()).await;
    let nat = probe.get("nat").cloned().unwrap_or(json!("unknown"));
    let port = cfg.port;
    let lan: Vec<String> = probe
        .get("lan")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let lan6: Vec<String> = probe
        .get("lan6")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let pub6 = probe.get("pub6").and_then(|v| v.as_str()).map(String::from);
    // IPv6 直连候选：本机全球 v6（网卡或 STUN 映射）。对称 NAT 下对方依旧无法主动连入，
    // 但 v6 出站映射通常独立于目标（EIM），保留候选由手机端实测决定
    let mut direct6: Vec<String> = lan6.iter().map(|a| format!("http://[{a}]:{port}")).collect();
    if let Some(p6) = &pub6 {
        let ip = p6.rsplit_once(':').map(|(a, _)| a.trim_matches(|c| c == '[' || c == ']')).unwrap_or(p6);
        let url = format!("http://[{ip}]:{port}");
        if !direct6.contains(&url) {
            direct6.push(url);
        }
    }
    let lan_urls: Vec<String> = lan.iter().map(|a| format!("http://{a}:{port}")).collect();
    // 云中继：用户显式配置优先，缺省用 osbt.space 官方中继（只转发不留存）
    let relay_base = cfg
        .cloud_relay_url
        .clone()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| crate::relay::DEFAULT_RELAY_BASE.to_string());
    let mut payload = json!({
        "v": 2,
        "app": "bit",
        "port": port,
        "key": cfg.client_key,
        "nat": nat.clone(),
        // 128 位识别码：客户端生成，云中继按它路由（手机端把请求发到 {relay}/relay/{rid}/…）
        "rid": rid,
        // 会话策略：device —— 每台设备扫码后自建独立会话（remote-<随机>）调 /api/chat，
        // 多台设备/多人互不串线；/api/chat 对空 session_id 直接拒绝，绝不落入桌面激活会话
        "sidPolicy": "device",
        // 内容留存声明：对话只存设备本地，中继 / 云服务器不留存（手机端据此展示隐私说明）
        "retention": "device-only",
        // 三种连接方式（按优先级排列尝试）
        "methods": {
            "lan": lan_urls,
            "direct6": direct6,
            "relay": format!("{relay_base}/relay/{rid}"),
        },
        // 原始候选（v1 兼容：旧手机端按 lan → lan6 → pub6 → pub4 依次尝试）
        "addrs": {
            "lan": probe.get("lan").cloned().unwrap_or(json!([])),
            "lan6": probe.get("lan6").cloned().unwrap_or(json!([])),
            "pub4": probe.get("pub4").cloned().unwrap_or(json!(null)),
            "pub4_alt": probe.get("pub4_alt").cloned().unwrap_or(json!(null)),
            "pub6": probe.get("pub6").cloned().unwrap_or(json!(null)),
        },
    });
    if cfg.password_enabled {
        if let Some(p) = &cfg.access_password {
            payload["pwd"] = json!(p);
        }
    }
    if let Some(u) = cfg.cloud_relay_url.clone().filter(|s| !s.is_empty()) {
        payload["cloud"] = json!(u);
    }
    // 信道签名算法标识：手机端据此实现配套的请求签名（bitsign-v2，材料含设备凭证）
    payload["alg"] = json!(crate::security::BITSIGN_ALG);
    // 加密块（BIT-Crypt v1）：把整个 payload JSON 加密成 BIT1: 密文——二维码图只编密文，
    // 普通扫码器/截图外泄读不出 client_key / rid 等连接凭据，只有本 App（内置主密钥）能解
    let plain = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    payload["enc"] = json!(crate::security::bitcrypt_encrypt(&plain));
    Ok(payload)
}

/// 远程连接二维码：payload 携带全部连接信息（地址候选 / 端口 / 密钥 / 密码 / 会话绑定 /
/// 128 位识别码 / 三种连接方式），手机 App 扫码后按 局域网 → IPv6 直连（NAT1）→ 云中继依次尝试。
/// 返回 payload JSON 与离线渲染的 SVG（黑码白底，深浅主题下均可识别）。
#[tauri::command]
pub async fn get_remote_qr(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let payload = qr_payload(&ctx).await?;
    // 二维码图只编加密块（BIT1: 密文）：截图/扫码器读不出明文凭据，
    // 本 App 扫码后用内置主密钥解出 payload JSON（手机端解密实现在 security.rs 注释）
    let enc = payload["enc"].as_str().ok_or("enc missing")?.to_string();
    let svg = qrcode::QrCode::new(enc.as_bytes())
        .map_err(|e| e.to_string())?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(220, 220)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Ok(json!({ "payload": payload, "svg": svg }))
}

/// 任意 URL 的二维码 SVG（离线渲染，黑码白底，深浅主题下均可识别）。
/// 用于「关于」弹窗的安卓版扫码下载等公开链接场景（连接凭据二维码走 get_remote_qr 加密通道）
#[tauri::command]
pub fn qr_svg_url(url: String) -> Result<String, String> {
    let url = url.trim().to_string();
    // 仅放行 https 链接：本命令面向公开下载地址，防止被滥用构造任意协议码
    if !url.starts_with("https://") {
        return Err("only https URLs are allowed".into());
    }
    Ok(qrcode::QrCode::new(url.as_bytes())
        .map_err(|e| e.to_string())?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(220, 220)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

#[tauri::command]
pub fn regenerate_client_key(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let key = {
        let mut cfg = ctx.config.lock().unwrap();
        let key = cfg.new_client_key();
        cfg.revision += 1;
        key
    };
    ctx.save_config();
    crate::audit::record(&ctx, "local-user", "remote.rotate_key", "client_key", json!({}), true);
    Ok(json!({ "client_key": key }))
}

#[tauri::command]
pub async fn test_connectivity(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let (addr, key, pwd, pwd_enabled) = {
        let cfg = ctx.config.lock().unwrap();
        (
            cfg.listen_addr(),
            cfg.client_key.clone(),
            cfg.access_password.clone().unwrap_or_default(),
            cfg.password_enabled,
        )
    };
    let url = format!("http://{addr}/api/health");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    // 健康检查（无需认证）
    let health_ok = matches!(
        client.get(&url).send().await,
        Ok(resp) if resp.status().is_success()
    );
    if !health_ok {
        return Err(format!("无法连接 {addr}"));
    }

    // 双重认证检查：带 Client Key + 密码访问受保护端点
    let mut req = client
        .get(format!("http://{addr}/api/tools"))
        .header("Authorization", format!("Bearer {key}"));
    if pwd_enabled {
        req = req.header("X-Access-Password", &pwd);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => Ok(json!({
            "ok": true,
            "addr": addr,
            "message": format!("服务运行中，双重认证通过: http://{addr}")
        })),
        Ok(resp) => Err(format!("认证异常: HTTP {}", resp.status())),
        Err(e) => Err(format!("无法连接 {addr}: {e}")),
    }
}

/// 设置远程访问密码（自定义），并可选启用/停用密码校验
#[tauri::command]
pub fn save_access_password(
    state: State<'_, Arc<Ctx>>,
    password: String,
    password_enabled: bool,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let password = password.trim().to_string();
    if password_enabled && (password.len() < 4 || password.len() > 64) {
        return Err("密码长度需在 4-64 位之间".into());
    }
    {
        let mut cfg = ctx.config.lock().unwrap();
        if password.is_empty() {
            // 未填密码时自动生成
            cfg.new_access_password();
        } else {
            cfg.access_password = Some(password);
        }
        cfg.password_enabled = password_enabled;
        cfg.revision += 1;
    }
    ctx.save_config();
    crate::audit::record(&ctx, "local-user", "remote.save_password", "access_password", json!({ "enabled": password_enabled }), true);
    Ok(json!({ "saved": true }))
}

/// 重新生成随机访问密码（8 位数字）
#[tauri::command]
pub fn regenerate_access_password(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let pwd = {
        let mut cfg = ctx.config.lock().unwrap();
        let pwd = cfg.new_access_password();
        cfg.revision += 1;
        pwd
    };
    ctx.save_config();
    crate::audit::record(&ctx, "local-user", "remote.rotate_password", "access_password", json!({}), true);
    Ok(json!({ "access_password": pwd }))
}

// ---------- AI（多协议提供方） ----------

/// 列出所有提供方（含当前激活项）
#[tauri::command]
pub fn list_providers(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let cfg = ctx.ai_config.lock().unwrap();
    json!({ "providers": cfg.providers.clone() })
}

/// 新增一个提供方（默认不激活）
#[tauri::command]
pub fn add_provider(
    state: State<'_, Arc<Ctx>>,
    name: String,
    protocol: String,
    base_url: String,
    api_key: String,
    model: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let protocol = match protocol.as_str() {
        "gemini" | "claude" | "openai" => protocol,
        _ => "openai".to_string(),
    };
    let base_url = {
        let b = base_url.trim();
        if b.is_empty() {
            crate::ai::Provider::default_base_url(&protocol).to_string()
        } else {
            b.to_string()
        }
    };
    let model = {
        let m = model.trim();
        if m.is_empty() {
            crate::ai::Provider::default_model(&protocol).to_string()
        } else {
            m.to_string()
        }
    };
    let name = {
        let n = name.trim();
        if n.is_empty() { protocol.clone() } else { n.to_string() }
    };
    let p = crate::ai::Provider {
        id: uuid::Uuid::new_v4().simple().to_string(),
        name,
        protocol,
        base_url,
        api_key: api_key.trim().to_string(),
        model,
        active: false,
        text_fallback: false,
    };
    let id = p.id.clone();
    {
        let mut cfg = ctx.ai_config.lock().unwrap();
        // 首个提供方自动设为激活
        let first = cfg.providers.is_empty();
        let mut p = p;
        p.active = first;
        cfg.providers.push(p);
    }
    ctx.save_ai_config();
    crate::audit::record(&ctx, "local-user", "ai.provider.add", &id, json!({}), true);
    Ok(json!({ "id": id }))
}

/// 更新某提供方的字段
#[tauri::command]
pub fn update_provider(
    state: State<'_, Arc<Ctx>>,
    id: String,
    name: String,
    protocol: String,
    base_url: String,
    api_key: String,
    model: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    {
        let mut cfg = ctx.ai_config.lock().unwrap();
        let p = cfg
            .providers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or("提供方不存在")?;
        let protocol = match protocol.as_str() {
            "gemini" | "claude" | "openai" => protocol,
            _ => "openai".to_string(),
        };
        p.name = { let n = name.trim(); if n.is_empty() { protocol.clone() } else { n.to_string() } };
        p.base_url = {
            let b = base_url.trim();
            if b.is_empty() { crate::ai::Provider::default_base_url(&protocol).to_string() } else { b.to_string() }
        };
        p.model = {
            let m = model.trim();
            if m.is_empty() { crate::ai::Provider::default_model(&protocol).to_string() } else { m.to_string() }
        };
        p.api_key = api_key.trim().to_string();
        p.protocol = protocol;
    }
    // 端点/协议可能已换：清除该提供方的原生探测缓存，下次对话重新探测
    ctx.native_probe.lock().unwrap().remove(&id);
    ctx.save_ai_config();
    crate::audit::record(&ctx, "local-user", "ai.provider.update", &id, json!({}), true);
    Ok(json!({ "saved": true }))
}

/// 删除某提供方（若删的是激活项，自动把剩余第一条设为激活）
#[tauri::command]
pub fn remove_provider(state: State<'_, Arc<Ctx>>, id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    {
        let mut cfg = ctx.ai_config.lock().unwrap();
        let was_active = cfg.providers.iter().find(|p| p.id == id).map(|p| p.active).unwrap_or(false);
        cfg.providers.retain(|p| p.id != id);
        if was_active {
            if let Some(first) = cfg.providers.first_mut() {
                first.active = true;
            }
        }
    }
    ctx.native_probe.lock().unwrap().remove(&id);
    ctx.save_ai_config();
    crate::audit::record(&ctx, "local-user", "ai.provider.remove", &id, json!({}), true);
    Ok(json!({ "removed": true }))
}

/// 文本协议降级开关（逐家提供方独立，默认关）：开启后该端点明确拒绝 tools 参数时
/// 自动降级文本协议（审计记录）；关闭时探测失败直接报错提示开启路径。
/// 切换即清除该提供方的探测缓存，下次对话重新探测
#[tauri::command]
pub fn set_provider_text_fallback(
    state: State<'_, Arc<Ctx>>,
    id: String,
    allowed: bool,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    {
        let mut cfg = ctx.ai_config.lock().unwrap();
        let p = cfg
            .providers
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or("提供方不存在")?;
        p.text_fallback = allowed;
    }
    ctx.native_probe.lock().unwrap().remove(&id);
    ctx.save_ai_config();
    crate::audit::record(&ctx, "local-user", "ai.provider.text_fallback", &id, json!({ "allowed": allowed }), true);
    Ok(json!({ "saved": true }))
}

/// 播放/暂停：设定当前激活提供方。active=true 时激活该项并暂停其余（互斥）；
/// active=false 时暂停该项（全部暂停 = 无激活项）。
#[tauri::command]
pub fn set_provider_active(
    state: State<'_, Arc<Ctx>>,
    id: String,
    active: bool,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    {
        let mut cfg = ctx.ai_config.lock().unwrap();
        if !cfg.providers.iter().any(|p| p.id == id) {
            return Err("提供方不存在".into());
        }
        for p in cfg.providers.iter_mut() {
            if p.id == id {
                p.active = active;
            } else if active {
                // 互斥：激活一个即暂停其余
                p.active = false;
            }
        }
    }
    ctx.save_ai_config();
    // 激活项变更：后台刷新模型上下文缓存（尽量获取最大上下文，失败静默）
    if active {
        let rf = ctx.clone();
        let rid = id.clone();
        tauri::async_runtime::spawn(async move {
            let p = rf.ai_config.lock().unwrap().providers.iter().find(|p| p.id == rid).cloned();
            if let Some(p) = p {
                refresh_model_context(&rf, &p.protocol, &p.base_url, &p.api_key).await;
            }
        });
    }
    crate::audit::record(&ctx, "local-user", "ai.provider.active", &id, json!({ "active": active }), true);
    Ok(json!({ "active": active }))
}

/// 任务完成后，若主窗口不可见（最小化/关闭到托盘），发系统通知提醒用户
fn notify_done(app: &tauri::AppHandle, ctx: &Arc<Ctx>, session_id: &str, messages: &[crate::ai::ChatMessage]) {
    let visible = app
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(true);
    if visible {
        return;
    }
    let title = ctx
        .sessions
        .lock()
        .unwrap()
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .map(|s| s.title.clone())
        .unwrap_or_else(|| "BIT".to_string());
    let reply = messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| crate::registry::safe_trunc(m.content.trim(), 80))
        .unwrap_or_default();
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title(format!("BIT · {title}"))
        .body(if reply.is_empty() { "任务执行完成".to_string() } else { reply })
        .show();
}

#[tauri::command]
pub async fn chat(
    state: State<'_, Arc<Ctx>>,
    app: tauri::AppHandle,
    session_id: String,
    message: String,
    images: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let messages = crate::agent::chat_turn(&ctx, &session_id, &message, images.unwrap_or_default()).await?;
    notify_done(&app, &ctx, &session_id, &messages);
    Ok(json!({ "messages": messages }))
}

/// 流式对话：过程通过 Tauri 事件 `event_name` 推送增量，返回最终完整消息列表。
/// `images` 为可选的图片（base64 data URL），仅随当前用户轮发给多模态模型。
#[tauri::command]
pub async fn chat_stream(
    state: State<'_, Arc<Ctx>>,
    app: tauri::AppHandle,
    session_id: String,
    message: String,
    event_name: String,
    images: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let ev = if event_name.trim().is_empty() { "chat-stream".to_string() } else { event_name };
    let messages =
        crate::agent::chat_turn_stream_auto(&ctx, &session_id, &message, &ev, images.unwrap_or_default()).await?;
    notify_done(&app, &ctx, &session_id, &messages);
    Ok(json!({ "messages": messages }))
}

/// 立即中断某会话正在执行的任务（执行循环在下个检查点停止）
#[tauri::command]
pub async fn chat_interrupt(state: State<'_, Arc<Ctx>>, session_id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let sid = if session_id.is_empty() { ctx.sessions.lock().unwrap().active.clone() } else { session_id };
    let hit = {
        let map = ctx.interrupts.lock().unwrap();
        match map.get(&sid) {
            Some(flag) => {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
                true
            }
            None => false,
        }
    };
    crate::audit::record(&ctx, "local-app", "chat.interrupt", &sid, json!({ "was_running": hit }), true);
    Ok(json!({ "id": sid, "interrupted": hit }))
}

/// 工具审批应答（允许 / 拒绝）
#[tauri::command]
pub async fn tool_approve(state: State<'_, Arc<Ctx>>, id: String, allow: bool) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let sender = ctx.approvals.lock().unwrap().remove(&id).map(|p| p.tx);
    match sender {
        Some(tx) => {
            let _ = tx.send(allow);
            Ok(json!({ "id": id, "allow": allow }))
        }
        None => Err("审批请求不存在或已处理".into()),
    }
}

/// 设置工具审批模式：ask（每次询问）/ auto（危险询问、安全自动通过）/ allow_all（完全放行）
#[tauri::command]
pub async fn set_tool_approval(state: State<'_, Arc<Ctx>>, mode: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    if !["ask", "auto", "allow_all"].contains(&mode.as_str()) {
        return Err("无效的审批模式".into());
    }
    {
        let mut cfg = ctx.config.lock().unwrap();
        cfg.tool_approval = mode.clone();
        cfg.revision += 1;
        cfg.save(&ctx.data_dir);
    }
    crate::audit::record(&ctx, "local-app", "tool.approval_mode", &mode, json!({ "mode": mode }), true);
    Ok(json!({ "mode": mode }))
}

/// 获取当前审批模式
#[tauri::command]
pub async fn get_tool_approval(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let mode = ctx.config.lock().unwrap().tool_approval.clone();
    Ok(json!({ "mode": mode }))
}

/// 读取开机自启真实状态（系统登录项为准；插件不可用时回退配置值）
#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle, state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let configured = ctx(state).config.lock().unwrap().autostart;
    use tauri_plugin_autostart::ManagerExt;
    let enabled = app.autolaunch().is_enabled().unwrap_or(configured);
    json!({ "enabled": enabled })
}

/// 设置开机自启：写/删系统登录项（macOS LaunchAgent / Windows 注册表 Run / Linux autostart），
/// 同步落盘配置保证下次启动的一致性
#[tauri::command]
pub fn set_autostart(
    app: tauri::AppHandle,
    state: State<'_, Arc<Ctx>>,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let c = ctx(state);
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled { manager.enable() } else { manager.disable() };
    match result {
        Ok(()) => {
            {
                let mut cfg = c.config.lock().unwrap();
                cfg.autostart = enabled;
                cfg.revision += 1;
                cfg.save(&c.data_dir);
            }
            crate::audit::record(&c, "local-app", "settings.autostart", &enabled.to_string(), json!({ "enabled": enabled }), true);
            Ok(json!({ "enabled": enabled }))
        }
        Err(e) => {
            crate::audit::record(&c, "local-app", "settings.autostart", &enabled.to_string(), json!({ "error": e.to_string() }), false);
            Err(format!("Failed to update launch-at-login entry: {e}"))
        }
    }
}

/// 读取模型采样参数（温度 / 思考强度）
#[tauri::command]
pub fn get_ai_params(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let cfg = ctx.ai_config.lock().unwrap();
    json!({ "temperature": cfg.temperature, "reasoning_effort": cfg.reasoning_effort })
}

/// 设置模型采样参数：temperature None=默认（0-2）；reasoning_effort ""=默认 / low / medium / high
#[tauri::command]
pub fn set_ai_params(
    state: State<'_, Arc<Ctx>>,
    temperature: Option<f64>,
    reasoning_effort: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let effort = match reasoning_effort.as_str() {
        "low" | "medium" | "high" => reasoning_effort,
        _ => String::new(),
    };
    {
        let mut cfg = ctx.ai_config.lock().unwrap();
        cfg.temperature = temperature.filter(|t| (0.0..=2.0).contains(t));
        cfg.reasoning_effort = effort.clone();
    }
    ctx.save_ai_config();
    crate::audit::record(
        &ctx,
        "local-app",
        "ai.params",
        "set",
        json!({ "temperature": temperature, "reasoning_effort": effort }),
        true,
    );
    Ok(json!({ "ok": true }))
}

/// Claude 协议的模型列表不携带上下文信息，用已知家族默认值兜底（token 数）
fn claude_context_for(id: &str) -> Option<u64> {
    if id.starts_with("claude") {
        Some(200_000)
    } else {
        None
    }
}

/// 从单个模型对象尽力提取上下文长度（各家字段不统一，逐个常见字段尝试）
fn context_len_from(model: &serde_json::Value) -> Option<u64> {
    const FIELDS: [&str; 5] =
        ["context_length", "max_model_len", "context_window", "max_context_length", "max_input_tokens"];
    for f in FIELDS {
        if let Some(v) = model.get(f).and_then(|v| v.as_u64()) {
            return Some(v);
        }
    }
    // OpenRouter 嵌套形态：top_provider.context_length
    model
        .get("top_provider")
        .and_then(|p| p.get("context_length"))
        .and_then(|v| v.as_u64())
}

/// 模型上下文缓存键：base 归一化（去首尾空白与尾斜杠）+ 模型 id
fn ctx_key(base: &str, id: &str) -> String {
    format!("{}|{id}", base.trim().trim_end_matches('/'))
}

/// 从提供方 API 拉取可用模型列表（含尽力获取的上下文长度），返回 (生效 base, models)：
/// - openai 兼容：GET {base}/models（Bearer Key）
/// - gemini：GET {base}/v1beta/models?key=（inputTokenLimit；name 去 "models/" 前缀）
/// - claude：GET {base}/v1/models（x-api-key + anthropic-version；无上下文字段用家族默认）
/// 自动检测：openai 兼容端点要求 base 以 /v1 结尾，用户漏写时自动补试 {base}/v1；
/// claude/gemini 由本函数拼路径前缀，用户多写 /v1、/v1beta 时自动去掉再试。
/// 返回第一个拿到合法模型列表的 base，前端据此把输入框纠正为可直接对话的端点。
pub async fn fetch_provider_models(
    protocol: &str,
    base_url: &str,
    api_key: &str,
) -> Result<(String, Vec<(String, Option<u64>)>), String> {
    let base = base_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err("Base URL 不能为空".into());
    }
    // 候选 base：按可能性排序，第一个是用户原输入（归一化后）
    let mut candidates: Vec<String> = vec![base.clone()];
    match protocol {
        "gemini" => {
            if let Some(stripped) = base.strip_suffix("/v1beta") {
                candidates.insert(0, stripped.to_string());
            }
        }
        "claude" => {
            if let Some(stripped) = base.strip_suffix("/v1") {
                candidates.insert(0, stripped.to_string());
            }
        }
        _ => {
            if !base.ends_with("/v1") {
                candidates.push(format!("{base}/v1"));
            }
        }
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let mut last_err = String::new();
    for cand in &candidates {
        let url = match protocol {
            "gemini" => format!("{cand}/v1beta/models?pageSize=200&key={api_key}"),
            "claude" => format!("{cand}/v1/models?limit=1000"),
            _ => format!("{cand}/models"),
        };
        let mut req = client.get(&url);
        match protocol {
            "gemini" => {} // Key 已在查询参数中
            "claude" => {
                req = req
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01");
            }
            _ => {
                if !api_key.is_empty() {
                    req = req.header("Authorization", format!("Bearer {api_key}"));
                }
            }
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("请求失败: {e}");
                continue;
            }
        };
        let status = resp.status();
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body_text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            // 错误体常含服务端原始 message，优先透传
            let msg = serde_json::from_str::<serde_json::Value>(&body_text)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .and_then(|m| m.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| crate::registry::safe_trunc(&body_text, 200));
            last_err = format!("HTTP {status}: {msg}");
            continue;
        }
        let looks_html = ct.starts_with("text/html") || body_text.trim_start().starts_with('<');
        let body: serde_json::Value = match serde_json::from_str(&body_text) {
            Ok(v) => v,
            Err(_) => {
                last_err = if looks_html {
                    "该地址返回的是网页而非 API 响应：OpenAI 兼容端点的 Base URL 通常要以 /v1 结尾（例如 https://example.com/v1）".to_string()
                } else {
                    format!("响应不是有效的模型列表 JSON（content-type: {ct}）")
                };
                continue;
            }
        };
        let has_list = body.get("data").and_then(|v| v.as_array()).is_some()
            || body.get("models").and_then(|v| v.as_array()).is_some();
        if !has_list {
            last_err = "响应里没有模型列表（缺少 data / models 字段），请确认这是 API 端点而非网页地址".to_string();
            continue;
        }
        let mut models: Vec<(String, Option<u64>)> = match protocol {
            "gemini" => body
                .get("models")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            let id = m.get("name")?.as_str()?.trim_start_matches("models/").to_string();
                            let len = m.get("inputTokenLimit").and_then(|v| v.as_u64());
                            Some((id, len))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            "claude" => body
                .get("data")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            let id = m.get("id")?.as_str()?.to_string();
                            let len = claude_context_for(&id);
                            Some((id, len))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => body
                .get("data")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            let id = m.get("id")?.as_str()?.to_string();
                            Some((id, context_len_from(m)))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        models.sort_by(|a, b| a.0.cmp(&b.0));
        models.dedup_by(|a, b| a.0 == b.0);
        return Ok((cand.clone(), models));
    }
    Err(last_err)
}

/// 把拉取到的上下文长度并入 ai_config.model_context 持久缓存并落盘
fn persist_model_context(ctx: &Arc<Ctx>, base_url: &str, models: &[(String, Option<u64>)]) {
    {
        let mut cfg = ctx.ai_config.lock().unwrap();
        for (id, len) in models {
            if let Some(n) = len {
                cfg.model_context.insert(ctx_key(base_url, id), *n);
            }
        }
    }
    ctx.save_ai_config();
}

/// 启动/配置变更时后台刷新激活提供方的模型上下文缓存（失败静默）
pub async fn refresh_model_context(ctx: &Arc<Ctx>, protocol: &str, base_url: &str, api_key: &str) {
    // 用自动检测后的生效 base 落缓存：与前端纠正后保存的 provider base 对齐
    if let Ok((effective, models)) = fetch_provider_models(protocol, base_url, api_key).await {
        persist_model_context(ctx, &effective, &models);
    }
}

/// 激活模型的最大上下文（token）：优先模型列表获取的缓存，claude 协议用家族默认兜底
pub fn active_max_context(ctx: &Arc<Ctx>) -> Option<u64> {
    let ai = ctx.ai_config.lock().unwrap();
    let p = ai.active()?.clone();
    let base = p.base_url.trim().trim_end_matches('/');
    ai.model_context
        .get(&ctx_key(base, &p.model))
        .copied()
        .or_else(|| claude_context_for(&p.model))
}

/// 从提供方 API 拉取可用模型列表（顺带把上下文长度写入持久缓存）：
/// 返回 {base, models}，base 为自动检测后的生效端点（纠正过 /v1 时前端据此回填输入框），
/// models 元素为 {id, context_length}，context_length 为 null 表示该端点未提供
#[tauri::command]
pub async fn list_provider_models(
    state: State<'_, Arc<Ctx>>,
    protocol: String,
    base_url: String,
    api_key: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let (effective, models) = fetch_provider_models(&protocol, &base_url, &api_key).await?;
    persist_model_context(&ctx, &effective, &models);
    Ok(json!({
        "base": effective,
        "models": models
            .into_iter()
            .map(|(id, len)| json!({ "id": id, "context_length": len }))
            .collect::<Vec<_>>(),
    }))
}

/// AI 接收信息预览：当前会话实际发给模型的 system prompt / 消息 / 工具清单
#[tauri::command]
pub async fn context_preview(state: State<'_, Arc<Ctx>>, session_id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let (convo, tools) = crate::agent::build_context(&ctx, &session_id)?;
    let est_tokens = estimate_context_tokens(&ctx, &session_id, &convo);
    let messages: Vec<serde_json::Value> = convo
        .iter()
        .enumerate()
        .map(|(i, m)| {
            json!({
                "index": i,
                "role": m.role,
                "content": m.content,
                "preview": m.content.chars().take(500).collect::<String>(),
            })
        })
        .collect();
    let tools_list = tools
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| {
            Some(json!({
                "name": t.get("name")?.as_str()?,
                "description": t.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            }))
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "system": convo.first().map(|m| m.content.clone()).unwrap_or_default(),
        "messages": messages,
        "tools": tools_list,
        "est_tokens": est_tokens,
        "max_context": active_max_context(&ctx),
        "approval_mode": ctx.config.lock().unwrap().tool_approval.clone(),
    }))
}

/// 当前会话上下文用量估算：与预览口径一致，包含 system prompt / 历史消息 /
/// 原生函数调用模式下额外发送的 tool definitions。
#[tauri::command]
pub async fn context_metrics(state: State<'_, Arc<Ctx>>, session_id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let (convo, _) = crate::agent::build_context(&ctx, &session_id)?;
    Ok(json!({
        "est_tokens": estimate_context_tokens(&ctx, &session_id, &convo),
        "max_context": active_max_context(&ctx),
    }))
}

/// 解析上传的文件（Excel→Markdown 表格 / Word(.docx)→纯文本 / CSV→原文）。
/// `filename` 用于按后缀分派，`data` 为 base64（可含 data:URL 前缀）。
#[tauri::command]
pub async fn extract_file(filename: String, data: String) -> Result<serde_json::Value, String> {
    // 解析可能较重，放到阻塞线程
    let handle = tauri::async_runtime::spawn_blocking(move || crate::extract::extract(&filename, &data));
    let text = handle.await.map_err(|e| format!("解析任务失败: {e}"))??;
    Ok(json!({ "text": text }))
}

/// 抓取网页并提取正文文字，返回 { title, text }
#[tauri::command]
pub async fn fetch_webpage(url: String) -> Result<serde_json::Value, String> {
    let (title, text) = crate::extract::fetch_webpage(&url).await?;
    Ok(json!({ "title": title, "text": text }))
}

/// 端口冲突检测：true=可用，false=已被占用（保存远程配置前调用）
#[tauri::command]
pub async fn check_port(host: String, port: u16) -> Result<serde_json::Value, String> {
    let addr = crate::config::join_host_port(host.trim(), port);
    match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => {
            drop(l);
            Ok(json!({ "available": true, "addr": addr }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            Ok(json!({ "available": false, "addr": addr, "reason": "端口已被占用" }))
        }
        Err(e) => Err(format!("检测 {addr} 失败: {e}")),
    }
}

/// ── MCP（Model Context Protocol）接入 ──

/// 自动发现：扫描本机端口范围，识别运行中的 MCP 服务器（Streamable HTTP）
#[tauri::command]
pub async fn mcp_discover(host: String, start: u16, end: u16) -> Result<serde_json::Value, String> {
    let found = crate::mcp::discover(&host, start, end).await?;
    Ok(json!({ "servers": found, "scanned": (end as u32 - start as u32 + 1) }))
}

/// 手动接入：对任意 URL 做 MCP 握手，成功则保存并返回服务器信息
#[tauri::command]
pub async fn mcp_connect(state: State<'_, Arc<Ctx>>, url: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let url = url.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return Err("请填写 MCP 服务器 URL".into());
    }
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let (name, version, protocol, session) = crate::mcp::initialize(&http, &url).await?;
    let server = crate::mcp::McpServer {
        id: format!("mcp-{}", uuid::Uuid::new_v4().simple()),
        name: name.clone(),
        url: url.clone(),
        version,
        protocol,
        session,
        enabled: true,
        connected_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    // 同一 URL 只保留一条
    let id = server.id.clone();
    {
        let mut list = ctx.mcp.lock().unwrap();
        list.retain(|s| s.url != url);
        list.push(server.clone());
    }
    ctx.save_mcp();
    crate::audit::record(&ctx, "local-app", "mcp.connect", &name, json!({ "url": url }), true);
    Ok(json!({ "server": server, "id": id }))
}

/// 已接入服务器列表
#[tauri::command]
pub async fn mcp_list(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let list = ctx.mcp.lock().unwrap().clone();
    Ok(json!({ "servers": list }))
}

/// 暂停 / 继续某个 MCP 服务器（暂停后其全部工具拒绝调用）
#[tauri::command]
pub async fn mcp_toggle(state: State<'_, Arc<Ctx>>, id: String, enabled: bool) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let name = {
        let mut list = ctx.mcp.lock().unwrap();
        let s = list.iter_mut().find(|s| s.id == id).ok_or("MCP 服务器不存在")?;
        s.enabled = enabled;
        s.name.clone()
    };
    ctx.save_mcp();
    crate::audit::record(&ctx, "local-app", if enabled { "mcp.enable" } else { "mcp.disable" }, &name, json!({ "enabled": enabled }), true);
    Ok(json!({ "id": id, "enabled": enabled }))
}

/// 移除接入（其导入的工具同步移除）
#[tauri::command]
pub async fn mcp_remove(state: State<'_, Arc<Ctx>>, id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let removed = {
        let mut list = ctx.mcp.lock().unwrap();
        let before = list.len();
        list.retain(|s| s.id != id);
        list.len() != before
    };
    if !removed {
        return Err("MCP 服务器不存在".into());
    }
    // 同步移除该服务器导入的工具
    {
        let mut tools = ctx.tools.lock().unwrap();
        tools.retain(|t| match &t.kind {
            crate::registry::ToolKind::Mcp { server_id, .. } => server_id != &id,
            _ => true,
        });
    }
    ctx.save_mcp();
    ctx.save_tools();
    crate::audit::record(&ctx, "local-app", "mcp.remove", &id, json!({}), true);
    Ok(json!({ "removed": id }))
}

/// 重新拉取某服务器的工具清单并导入注册中心（同名跳过）
#[tauri::command]
pub async fn mcp_import(state: State<'_, Arc<Ctx>>, id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let server = crate::mcp::find(&ctx, &id).ok_or("MCP 服务器不存在")?;
    let tools = crate::mcp::list_tools(&server).await?;
    let mut imported = 0usize;
    let mut skipped = 0usize;
    for t in &tools {
        let ok = crate::registry::register(
            &ctx,
            &t.name,
            &format!("{}（MCP · {}）", t.description, server.name),
            if t.input_schema.is_null() || !t.input_schema.is_object() {
                json!({"type": "object", "properties": {}, "additionalProperties": true})
            } else {
                t.input_schema.clone()
            },
            crate::registry::ToolKind::Mcp { server_id: id.clone(), tool: t.name.clone() },
            "mcp",
        );
        match ok {
            Ok(_) => imported += 1,
            Err(_) => skipped += 1,
        }
    }
    crate::audit::record(&ctx, "local-app", "mcp.import", &server.name, json!({ "imported": imported, "skipped": skipped }), true);
    Ok(json!({ "imported": imported, "skipped": skipped, "total": tools.len() }))
}

/// 手动压缩会话：用 AI 把全部历史总结为一条摘要（system 消息），释放上下文空间。
/// 摘要写入会话后返回新消息列表；压缩不影响会话本身，可继续对话。
#[tauri::command]
pub async fn compress_session(
    state: State<'_, Arc<Ctx>>,
    session_id: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let target = if session_id.is_empty() {
        ctx.sessions.lock().unwrap().active.clone()
    } else {
        session_id
    };

    // 取出历史（在锁外进行 AI 调用，避免阻塞其他会话）
    let history: Vec<crate::ai::ChatMessage> = {
        let store = ctx.sessions.lock().unwrap();
        let sess = store
            .sessions
            .iter()
            .find(|s| s.id == target)
            .ok_or("会话不存在")?;
        sess.messages.iter().filter(|m| m.role != "system").cloned().collect()
    };
    if history.len() < 4 {
        return Err("对话内容太少，无需压缩".into());
    }

    let mut convo = vec![crate::ai::ChatMessage::system(
        "你是对话压缩助手。请把下面这段 AI 对话历史浓缩成一份结构化中文摘要，保留：用户的需求与偏好、已达成的结论、关键事实与数据、未完成的待办。直接输出摘要正文，不要寒暄，控制在 800 字以内。",
    )];
    for m in &history {
        let who = if m.role == "user" { "用户" } else { "AI" };
        convo.push(crate::ai::ChatMessage::user(format!("【{who}】{}", m.content)));
    }
    let summary = crate::ai::chat(&ctx, &convo).await?;

    // 用摘要替换全部历史（摘要以 system 消息存放，前端气泡不显示）
    let before = {
        let mut store = ctx.sessions.lock().unwrap();
        let sess = store.get_mut(&target).ok_or("会话不存在")?;
        let n = sess.messages.len();
        sess.messages = vec![crate::ai::ChatMessage::system(format!(
            "以下是对此前对话的压缩摘要，请结合它继续对话：\n\n{summary}"
        ))];
        sess.touch();
        n
    };
    crate::session::persist(&ctx);
    crate::audit::record(&ctx, "local-app", "session.compress", &target, json!({ "messages_before": before }), true);
    Ok(json!({
        "messages": ctx.sessions.lock().unwrap()
            .sessions.iter().find(|s| s.id == target)
            .map(|s| s.messages.clone()).unwrap_or_default(),
        "summary": summary,
        "messages_before": before
    }))
}

// ---------- 会话（多对话分组） ----------

/// 列出所有会话（不含完整消息，仅元信息 + 预览），并返回当前激活会话 id
#[tauri::command]
pub fn list_sessions(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    crate::session::refresh_from_disk(&ctx);
    let store = ctx.sessions.lock().unwrap();
    let mut list: Vec<serde_json::Value> = store
        .sessions
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "title": s.title,
                "created": s.created,
                "updated": s.updated,
                "count": s.messages.iter().filter(|m| m.role != "system").count(),
                "preview": s.preview(),
            })
        })
        .collect();
    // 最近更新的排在前面
    list.sort_by(|a, b| b["updated"].as_str().unwrap_or("").cmp(a["updated"].as_str().unwrap_or("")));
    json!({ "sessions": list, "active": store.active })
}

/// 读取某会话的完整消息（session_id 为空则读激活会话）
#[tauri::command]
pub fn get_session(state: State<'_, Arc<Ctx>>, session_id: String) -> serde_json::Value {
    let ctx = ctx(state);
    crate::session::refresh_from_disk(&ctx);
    let store = ctx.sessions.lock().unwrap();
    let id = if session_id.is_empty() { store.active.clone() } else { session_id };
    let msgs = store
        .sessions
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.messages.clone())
        .unwrap_or_default();
    json!({ "id": id, "messages": msgs })
}

/// 新建会话并设为激活
#[tauri::command]
pub fn create_session(state: State<'_, Arc<Ctx>>, title: String) -> serde_json::Value {
    let ctx = ctx(state);
    let id;
    {
        let mut store = ctx.sessions.lock().unwrap();
        let s = crate::session::Session::new(&title);
        id = s.id.clone();
        store.sessions.push(s);
        store.active = id.clone();
    }
    ctx.save_sessions();
    json!({ "id": id })
}

/// 切换激活会话
#[tauri::command]
pub fn set_active_session(state: State<'_, Arc<Ctx>>, session_id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    {
        let mut store = ctx.sessions.lock().unwrap();
        if !store.sessions.iter().any(|s| s.id == session_id) {
            return Err("会话不存在".into());
        }
        store.active = session_id.clone();
    }
    ctx.save_sessions();
    Ok(json!({ "active": session_id }))
}

/// 重命名会话
#[tauri::command]
pub fn rename_session(state: State<'_, Arc<Ctx>>, session_id: String, title: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    {
        let mut store = ctx.sessions.lock().unwrap();
        let s = store.get_mut(&session_id).ok_or("会话不存在")?;
        let t = title.trim();
        s.title = if t.is_empty() { "未命名".into() } else { t.to_string() };
    }
    ctx.save_sessions();
    Ok(json!({ "renamed": true }))
}

/// 删除会话（删完若为空自动补一个默认会话；删的是激活项则切到最近一条）
#[tauri::command]
pub fn delete_session(state: State<'_, Arc<Ctx>>, session_id: String) -> serde_json::Value {
    let ctx = ctx(state);
    let active;
    {
        let mut store = ctx.sessions.lock().unwrap();
        store.sessions.retain(|s| s.id != session_id);
        if store.sessions.is_empty() {
            let s = crate::session::Session::new("新对话");
            store.active = s.id.clone();
            store.sessions.push(s);
        } else if store.active == session_id {
            store.active = store.sessions.last().map(|s| s.id.clone()).unwrap_or_default();
        }
        active = store.active.clone();
    }
    ctx.save_sessions();
    json!({ "deleted": true, "active": active })
}

/// 清空某会话的消息（保留会话本身）
#[tauri::command]
pub fn clear_session(state: State<'_, Arc<Ctx>>, session_id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    {
        let mut store = ctx.sessions.lock().unwrap();
        let id = if session_id.is_empty() { store.active.clone() } else { session_id };
        let s = store.get_mut(&id).ok_or("会话不存在")?;
        s.messages.clear();
        s.touch();
    }
    ctx.save_sessions();
    Ok(json!({ "cleared": true }))
}

// ---------- 记忆 / 技�?/ Autopilot ----------

#[tauri::command]
pub fn list_memories(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let mut mem = ctx.memories.lock().unwrap().clone();
    mem.reverse();
    json!({ "memories": mem })
}

#[tauri::command]
pub fn add_memory(state: State<'_, Arc<Ctx>>, content: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    if content.trim().is_empty() {
        return Err("记忆内容不能为空".into());
    }
    let m = crate::memory::add_memory(&ctx, &content, "raw", "user");
    crate::audit::record(&ctx, "local-user", "memory.add", "memories", json!({}), true);
    Ok(json!({ "memory": m }))
}

/// 批量删除记忆（单条删除传一个元素的数组即可）
#[tauri::command]
pub fn delete_memories(
    state: State<'_, Arc<Ctx>>,
    ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    if ids.is_empty() {
        return Err("未选择要删除的记忆".into());
    }
    let removed = crate::memory::delete_memories(&ctx, &ids);
    crate::audit::record(
        &ctx,
        "local-user",
        "memory.delete",
        "memories",
        json!({ "count": removed }),
        true,
    );
    Ok(json!({ "removed": removed }))
}

#[tauri::command]
pub fn list_skills(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let mut skills = ctx.skills.lock().unwrap().clone();
    skills.reverse();
    json!({ "skills": skills })
}

#[tauri::command]
pub fn add_skill(
    state: State<'_, Arc<Ctx>>,
    name: String,
    summary: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    if name.trim().is_empty() || summary.trim().is_empty() {
        return Err("技能名称与说明不能为空".into());
    }
    let s = crate::memory::add_skill(&ctx, &name, &summary, "user");
    crate::audit::record(&ctx, "local-user", "skill.add", &name, json!({}), true);
    Ok(json!({ "skill": s }))
}

/// 批量删除技能（单条删除传一个元素的数组即可）
#[tauri::command]
pub fn delete_skills(
    state: State<'_, Arc<Ctx>>,
    ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    if ids.is_empty() {
        return Err("未选择要删除的技能".into());
    }
    let removed = crate::memory::delete_skills(&ctx, &ids);
    crate::audit::record(
        &ctx,
        "local-user",
        "skill.delete",
        "skills",
        json!({ "count": removed }),
        true,
    );
    Ok(json!({ "removed": removed }))
}

/// 小圆片播放/暂停：控制 SKILL 与记忆的自动总结循环
#[tauri::command]
pub fn toggle_autopilot(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let next = !ctx.autopilot_running.load(Ordering::SeqCst);
    ctx.autopilot_running.store(next, Ordering::SeqCst);
    crate::audit::record(
        &ctx,
        "local-user",
        "autopilot.toggle",
        if next { "play" } else { "pause" },
        json!({}),
        true,
    );
    // 同步托盘菜单文案与其它窗口
    crate::tray::refresh(&ctx.app);
    if let Some(win) = ctx.app.get_webview_window("main") {
        let _ = win.emit("autopilot-changed", next);
    }
    json!({ "running": next })
}

// ---------- 目标 / 待办 ----------

#[tauri::command]
pub fn list_goals(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let mut goals = ctx.goals.lock().unwrap().clone();
    goals.reverse();
    json!({ "goals": goals })
}

#[tauri::command]
pub fn create_goal(
    state: State<'_, Arc<Ctx>>,
    title: String,
    detail: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let g = crate::goal::create_goal(&ctx, &title, &detail, "user", None)?;
    crate::audit::record(&ctx, "local-user", "goal.create", &g.title, json!({}), true);
    Ok(json!({ "goal": g }))
}

#[tauri::command]
pub fn update_goal_status(
    state: State<'_, Arc<Ctx>>,
    id: String,
    status: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let g = crate::goal::update_goal_status(&ctx, &id, &status)?;
    crate::audit::record(&ctx, "local-user", "goal.update", &g.title, json!({ "status": status }), true);
    Ok(json!({ "goal": g }))
}

#[tauri::command]
pub fn remove_goal(state: State<'_, Arc<Ctx>>, id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    crate::goal::remove_goal(&ctx, &id)?;
    crate::audit::record(&ctx, "local-user", "goal.remove", "goal", json!({}), true);
    Ok(json!({ "removed": true }))
}

#[tauri::command]
pub fn list_todos(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let ctx = ctx(state);
    let mut todos = ctx.todos.lock().unwrap().clone();
    todos.reverse();
    json!({ "todos": todos })
}

#[tauri::command]
pub fn add_todo(
    state: State<'_, Arc<Ctx>>,
    content: String,
    goal_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let t = crate::goal::add_todo(&ctx, goal_id, &content, "user", None)?;
    crate::audit::record(&ctx, "local-user", "todo.add", &t.content, json!({}), true);
    Ok(json!({ "todo": t }))
}

#[tauri::command]
pub fn update_todo_status(
    state: State<'_, Arc<Ctx>>,
    id: String,
    status: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let t = crate::goal::update_todo_status(&ctx, &id, &status)?;
    crate::audit::record(&ctx, "local-user", "todo.update", &t.content, json!({ "status": status }), true);
    Ok(json!({ "todo": t }))
}

#[tauri::command]
pub fn remove_todo(state: State<'_, Arc<Ctx>>, id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    crate::goal::remove_todo(&ctx, &id)?;
    crate::audit::record(&ctx, "local-user", "todo.remove", "todo", json!({}), true);
    Ok(json!({ "removed": true }))
}

/// 真正退出应用（关闭窗口只是隐藏到托盘）
#[tauri::command]
pub fn quit_app(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    crate::audit::record(&ctx, "local-app", "app.quit", "BIT", json!({ "via": "ui" }), true);
    ctx.app.exit(0);
    Ok(json!({ "quit": true }))
}

/// 立即触发一次自动总结（不等周期）
#[tauri::command]
pub async fn run_autopilot_now(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let ctx2 = ctx.clone();
    tauri::async_runtime::spawn(async move {
        let _ = crate::autopilot::tick_public(&ctx2).await;
    });
    Ok(json!({ "triggered": true }))
}

/// ── 文件打开（send_file 文件卡片用）──

/// 用系统默认程序打开文件；reveal=true 时打开所在文件夹并定位该文件
#[tauri::command]
pub fn open_path(state: State<'_, Arc<Ctx>>, path: String, reveal: Option<bool>) -> Result<(), String> {
    let cleaned = normalize_user_path(&path);
    let mut p = std::path::PathBuf::from(&cleaned);
    if !p.exists() && p.is_relative() {
        // 兼容旧卡片里的相对路径：send_file 曾原样存储 AI 给的路径
        p = state.data_dir.join(&p);
    }
    if !p.exists() {
        return Err(format!("路径不存在: {cleaned}"));
    }
    // 绝对化：消除符号链接与相对段，确保 Finder/资源管理器定位到真实位置
    let abs = std::fs::canonicalize(&p).unwrap_or(p);
    open_target(&abs, reveal.unwrap_or(false))
}

/// 清理 AI/用户给的路径：去首尾空白、去成对引号、展开 ~。send_file 与 open_path 共用。
pub(crate) fn normalize_user_path(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        let first = bytes[0];
        let last = bytes[s.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            s = s[1..s.len() - 1].trim().to_string();
        }
    }
    if let Some(rest) = s.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            if let Some(home) = std::env::var_os("HOME") {
                s = format!("{}{}", home.to_string_lossy(), rest);
            }
        }
    }
    s
}

/// canonicalize 后转成可传给系统调用的展示路径：Windows 的 std::fs::canonicalize 返回
/// `\\?\` verbatim 前缀（且可能混入正斜杠），explorer `/select,` 解析不了会静默退回
/// 打开默认文件夹（文档）——必须剥前缀、统一反斜杠；`\\?\UNC\` 还原为 `\\`。
/// 非 Windows 平台原样返回字符串。
pub(crate) fn clean_display_path(p: &std::path::Path) -> String {
    #[cfg(target_os = "windows")]
    {
        let s = p.to_string_lossy().replace('/', "\\");
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            rest.to_string()
        } else {
            s
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        p.to_string_lossy().to_string()
    }
}

fn open_target(p: &std::path::Path, reveal: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        if reveal {
            cmd.arg("-R");
        }
        cmd.arg(p);
        // open 是快速返回的 LaunchServices 调用：必须等退出码，失败时不能静默——
        // 否则 Finder 停在原窗口，用户看到的是"定位到了错误的位置"
        let out = cmd.output().map_err(|e| format!("打开失败: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let reason = stderr.trim();
            return Err(format!(
                "打开失败: {}",
                if reason.is_empty() { format!("系统拒绝打开 {}", p.display()) } else { reason.to_string() }
            ));
        }
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        // CREATE_NO_WINDOW：避免后台命令闪黑框
        // 统一走 explorer.exe：不能用 `cmd /C start` 中转——路径/URL 含 & | ^ 等
        // cmd 元字符且无空格时 std 不加引号，cmd.exe 会把它们当命令分隔符执行（注入）
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // 剥 \\?\ verbatim 前缀 + 统一反斜杠：explorer 解析不了 verbatim 路径会退回打开默认文件夹
        let disp = clean_display_path(p);
        if reveal {
            // /select, 与路径必须是单个参数且路径自带引号：std 会给含空格的参数整体加引号，
            // explorer 解析 "/select,C:\a b\c.txt" 会定位到错误位置——必须 raw_arg 预引号
            return std::process::Command::new("explorer")
                .raw_arg(format!("/select,\"{disp}\""))
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("打开失败: {e}"));
        }
        // explorer 对文件按默认关联程序打开、对目录打开文件夹（exit code 不可靠，只 spawn 不判状态）
        return std::process::Command::new("explorer")
            .raw_arg(format!("\"{disp}\""))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开失败: {e}"));
    }
    #[cfg(target_os = "linux")]
    {
        let target = if reveal {
            // xdg-open 无「定位选中」能力，退化为打开所在文件夹
            p.parent().unwrap_or(p).to_path_buf()
        } else {
            p.to_path_buf()
        };
        return std::process::Command::new("xdg-open")
            .arg(&target)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开失败: {e}"));
    }
    #[allow(unreachable_code)]
    Err("不支持的平台".into())
}

/// 用系统默认浏览器打开外部链接（更新提示、官网等）
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    // 只允许 http(s)，防止任意命令注入
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("仅支持 http(s) 链接".into());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(&url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        // 不能用 `cmd /C start` 中转：URL 查询串普遍含 &，无空格时 std 不加引号，
        // cmd.exe 会把 & 当命令分隔符（如 https://x/?a=1&calc 会执行 calc）→ 注入。
        // rundll32 FileProtocolHandler 直接调系统 URL 关联处理，不经 cmd
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", &url]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(&url);
        c
    };
    cmd.spawn().map_err(|e| format!("打开浏览器失败: {e}"))?;
    Ok(())
}

/// ── 高权限模式（管理员 / root）──

/// 当前进程是否已提权：Windows 探测 `net session`（仅管理员可成功，无新依赖）；
/// unix 看 `id -u` 是否为 0
pub(crate) fn is_elevated() -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("net")
            .arg("session")
            .creation_flags(0x0800_0000)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>() == Ok(0))
            .unwrap_or(false)
    }
}

/// 以目标权限重启自身。elevate=true 触发系统授权弹窗（Windows UAC / macOS 管理员授权 /
/// Linux polkit），用户取消则返回 Err；elevate=false 尝试降权拉起（Windows 经 explorer.exe
/// 中完整性级别中转）。数据目录经 `--data-dir` 参数透传——授权弹窗产生的子进程不继承
/// 进程环境变量，仅靠 BIT_DATA_DIR 会在提权后丢失。
fn relaunch_with_elevation(exe: &std::path::Path, data_dir: &str, elevate: bool) -> Result<(), String> {
    let exe_str = clean_display_path(exe);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if elevate {
            // PowerShell 单引号转义（'' ）；-ArgumentList 透传 --data-dir
            let esc = exe_str.replace('\'', "''");
            let dir = data_dir.replace('\'', "''");
            let out = std::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    &format!("Start-Process -FilePath '{esc}' -ArgumentList '--data-dir','{dir}' -Verb RunAs"),
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .map_err(|e| format!("提权启动失败: {e}"))?;
            if !out.status.success() {
                return Err("未获得管理员授权（用户取消或被策略拒绝）".into());
            }
            return Ok(());
        }
        // 降权：explorer.exe 运行在中完整性级别，由它拉起的子进程不再是管理员。
        // explorer 不透传参数；BIT_DATA_DIR 覆盖仅用于测试环境，正常数据目录为标准位置不受影响
        std::process::Command::new("explorer")
            .raw_arg(format!("\"{exe_str}\""))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("降权启动失败: {e}"))
    }
    #[cfg(target_os = "macos")]
    {
        let sh_e = exe_str.replace('\'', "'\\''");
        let sh_d = data_dir.replace('\'', "'\\''");
        if elevate {
            // osascript 成功返回 ≠ 子进程真的活下来了：
            // 1) macOS GUI 进程以 root 身份无法创建 WindowServer 窗口（系统级硬限制）
            //    —— GUI 应用会在 create_webview 时立即崩掉，进程退出码正常
            // 2) nohup 在 osascript 的无 tty shell 里会报 "can't detach from console" 导致失败
            // 修复：不用 nohup，纯 & 后台（osascript shell 本身能正确后台化子进程）；
            // 命令里同时传递 --data-dir 和 BIT_DATA_DIR（环境在 root shell 里会清空）；
            // 最重要：osascript 成功后验证新进程是否真的活着，没活就返回错误，
            // 调用方会保持当前进程运行、不退出老进程（用户不会丢失当前会话）
            let script = format!(
                "do shell script \"BIT_DATA_DIR='{d}' '{e}' --data-dir '{d}' >/dev/null 2>&1 &\" with administrator privileges",
                d = sh_d,
                e = sh_e
            );
            let out = std::process::Command::new("osascript")
                .args(["-e", &script])
                .output()
                .map_err(|e| format!("提权启动失败: {e}"))?;
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                return Err(format!("未获得管理员授权: {}", err.trim()));
            }
            // 等待并验证 root 子进程是否真的存活（最多 5 次，每次 0.5s）
            let exe_basename = exe
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("bit");
            for _ in 0..5 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let alive = std::process::Command::new("pgrep")
                    .args(["-x", exe_basename])
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().to_string().parse::<u32>().ok())
                    .map(|pid| {
                        // 验证该进程是 root 身份
                        std::process::Command::new("ps")
                            .args(["-o", "user=", "-p", &pid.to_string()])
                            .output()
                            .ok()
                            .map(|p| String::from_utf8_lossy(&p.stdout).trim() == "root")
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if alive {
                    return Ok(());
                }
            }
            return Err(
                "提权启动失败：管理员版进程未成功启动（macOS 系统限制 root 身份无法创建 GUI 窗口）".into()
            );
        }
        // 降权：su 到控制台登录用户（root 执行 su 无需密码）
        let user = std::process::Command::new("stat")
            .args(["-f", "%Su", "/dev/console"])
            .output()
            .map_err(|e| format!("降权启动失败: {e}"))?;
        let user = String::from_utf8_lossy(&user.stdout).trim().to_string();
        if user.is_empty() || user == "root" {
            return Err("无法确定控制台用户，降权失败".into());
        }
        std::process::Command::new("su")
            .args(["-l", &user, "-c", &format!("BIT_DATA_DIR='{d}' nohup '{e}' >/dev/null 2>&1 &", d = sh_d, e = sh_e)])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("降权启动失败: {e}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if elevate {
            // pkexec 触发 polkit 授权弹窗；取消时返回非零。环境被清空 → 数据目录走参数
            let mut cmd = std::process::Command::new("pkexec");
            cmd.arg(&exe_str);
            if !data_dir.is_empty() {
                cmd.args(["--data-dir", data_dir]);
            }
            let out = cmd.output().map_err(|e| format!("提权启动失败: {e}"))?;
            if !out.status.success() {
                return Err("未获得管理员授权（用户取消或被策略拒绝）".into());
            }
            return Ok(());
        }
        // 降权：pkexec 提权场景拿 PKEXEC_UID，sudo 场景拿 SUDO_USER，回退 root 同名不可行则报错
        let uid = std::env::var("PKEXEC_UID").ok().or_else(|| std::env::var("SUDO_UID").ok());
        let user = match uid {
            Some(uid) => {
                let out = std::process::Command::new("getent")
                    .args(["passwd", &uid])
                    .output()
                    .map_err(|e| format!("降权启动失败: {e}"))?;
                let line = String::from_utf8_lossy(&out.stdout);
                line.split(':').next().unwrap_or("").trim().to_string()
            }
            None => String::new(),
        };
        if user.is_empty() {
            return Err("无法确定原用户，降权失败".into());
        }
        let mut cmd = std::process::Command::new("runuser");
        cmd.args(["-u", &user, "--", &exe_str]);
        if !data_dir.is_empty() {
            cmd.args(["--data-dir", data_dir]);
        }
        cmd.spawn().map(|_| ()).map_err(|e| format!("降权启动失败: {e}"))
    }
}

/// 工具质量评估快照：每工具近期成功率 / 累计成败 / 平均耗时 / 最近失败原因（失败次数降序）
#[tauri::command]
pub fn get_tool_stats(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    crate::state::toolstats::snapshot(&ctx(state))
}

/// 诊断报告：版本 / 平台 / 提权 / 守护 / 运行时长 / 数据文件清单 / 最近崩溃 / 低成功率工具，
/// 一键自检排障所需的最小信息集
#[tauri::command]
pub fn get_diagnostics(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let c = ctx(state);
    let cfg = c.config.lock().unwrap().clone();
    let (sessions_n, messages_n, tools_n, tools_on) = {
        let sessions = c.sessions.lock().unwrap();
        let tools = c.tools.lock().unwrap();
        (
            sessions.sessions.len(),
            sessions.sessions.iter().map(|s| s.messages.len()).sum::<usize>(),
            tools.len(),
            tools.iter().filter(|t| t.enabled).count(),
        )
    };
    // 数据文件清单：存在性与大小（排障时确认哪些数据在、哪些丢失）
    let file = |name: &str| {
        let p = c.data_dir.join(name);
        json!({
            "name": name,
            "exists": p.exists(),
            "bytes": std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0),
        })
    };
    let names = [
        "config.json", "ai_config.json", "tools.json", "runtimes.json", "skills.json",
        "memories.json", "sessions.json", "goals.json", "todos.json", "mcp_servers.json",
        "audit.json", "tool_stats.json", "guardian.json", "guardian.log", "crash.log",
    ];
    let files: Vec<serde_json::Value> = names.iter().map(|n| file(n)).collect();
    // 低成功率工具：有失败记录的取前 5（快照已按失败次数降序）
    let stats = crate::state::toolstats::snapshot(&c);
    let worst: Vec<serde_json::Value> = stats
        .as_array()
        .map(|a| a.iter().filter(|t| t["fail"].as_u64().unwrap_or(0) > 0).take(5).cloned().collect())
        .unwrap_or_default();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "platform": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        "elevated": is_elevated(),
        "uptime_secs": c.started.elapsed().as_secs(),
        "remote": { "enabled": cfg.remote_enabled, "port": cfg.port },
        "sessions": { "count": sessions_n, "messages": messages_n },
        "tools": { "total": tools_n, "enabled": tools_on },
        "data_dir": c.data_dir.display().to_string(),
        "files": files,
        "worst_tools": worst,
        "guardian": crate::guardian::diagnose(&c.data_dir, &cfg.client_key),
        "crashes": crate::crash::tail(&c.data_dir, 5),
    })
}

/// 查询高权限状态：active=当前进程实际已提权；enabled=配置意图（两者可能不一致：
/// 授权弹窗被取消时配置保持开启但进程未提权）
#[tauri::command]
pub fn get_elevation(state: State<'_, Arc<Ctx>>) -> serde_json::Value {
    let c = ctx(state);
    json!({
        "active": is_elevated(),
        "enabled": c.config.lock().unwrap().elevated,
    })
}

/// 开启/关闭高权限模式：开启即触发系统授权弹窗并以管理员身份重启；关闭以普通权限重启。
/// 成功后当前进程自动退出，新实例接管；失败（授权被取消等）保持当前进程运行并返回错误。
#[tauri::command]
pub async fn set_elevation(app: tauri::AppHandle, state: State<'_, Arc<Ctx>>, enabled: bool) -> Result<serde_json::Value, String> {
    let c = ctx(state);
    let active = is_elevated();
    if enabled == active {
        // 意图与现状一致：仅同步配置（修正在系统外手动提权/降权造成的漂移）
        {
            let mut cfg = c.config.lock().unwrap();
            cfg.elevated = enabled;
            cfg.revision += 1;
            cfg.save(&c.data_dir);
        }
        return Ok(json!({ "active": active, "enabled": enabled }));
    }
    crate::audit::record(&c, "local-app", "settings.elevation", &enabled.to_string(), json!({ "enabled": enabled, "was_active": active }), true);
    let exe = std::env::current_exe().map_err(|e| format!("无法定位可执行文件: {e}"))?;
    let data_dir = c.data_dir.to_string_lossy().to_string();
    // 阻塞等待授权弹窗结果：放入阻塞线程池，避免卡住异步运行时
    let h = tokio::task::spawn_blocking(move || relaunch_with_elevation(&exe, &data_dir, enabled));
    let result = h.await.map_err(|e| format!("重启任务失败: {e}"))?;
    match result {
        Ok(()) => {
            {
                let mut cfg = c.config.lock().unwrap();
                cfg.elevated = enabled;
                cfg.revision += 1;
                cfg.save(&c.data_dir);
            }
            // 正常重启交接：通知守护进程不要按意外死亡接力，新实例会重新布防
            crate::guardian::expect_exit(&c);
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            app.exit(0);
            #[allow(unreachable_code)]
            Ok(json!({ "active": enabled, "enabled": enabled }))
        }
        Err(e) => {
            crate::audit::record(&c, "local-app", "settings.elevation", &enabled.to_string(), json!({ "error": e }), false);
            // 授权被取消：守护进程可能已收到退出标记，立即重新布防补位
            crate::guardian::arm(&c);
            Err(e)
        }
    }
}

/// 安装 `bit` 命令到终端 PATH：macOS/Linux 优先 /usr/local/bin 符号链接（无权限回退 ~/.local/bin），
/// Windows 写 bit.cmd 到用户 PATH 自带的 %LOCALAPPDATA%\Microsoft\WindowsApps（start /wait 等待 GUI 进程）。
/// BIT_CLI_DIR 环境变量可覆盖安装目录（E2E 测试用）。
pub fn install_cli_impl(ctx: &Arc<Ctx>) -> Result<serde_json::Value, String> {
    // AppImage：current_exe() 指向 /tmp/.mount_xxx 临时挂载点（退出即失效），
    // 链接必须指向 $APPIMAGE（AppImage 运行时注入的 .AppImage 本体，参数会透传给内部二进制）
    let exe = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| std::env::current_exe().ok())
        .ok_or("无法定位可执行文件")?;
    let record_audit = |path: &str, hint: &str| {
        crate::audit::record(
            ctx,
            "local-user",
            "cli.install",
            path,
            serde_json::json!({ "hint": hint }),
            true,
        );
    };

    // 测试覆盖目录：直接在其中创建链接/启动器
    if let Ok(dir) = std::env::var("BIT_CLI_DIR") {
        if !dir.trim().is_empty() {
            let dir = std::path::PathBuf::from(dir.trim());
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
            let r = install_cli_into(&dir, &exe)?;
            record_audit(r["path"].as_str().unwrap_or_default(), r["hint"].as_str().unwrap_or_default());
            return Ok(r);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let dir = std::path::PathBuf::from(local).join("Microsoft").join("WindowsApps");
            let r = install_cli_into(&dir, &exe)?;
            record_audit(r["path"].as_str().unwrap_or_default(), r["hint"].as_str().unwrap_or_default());
            return Ok(r);
        }
        return Err("无法定位 %LOCALAPPDATA%".into());
    }
    #[cfg(not(target_os = "windows"))]
    {
        // 优先系统级 /usr/local/bin（已在 PATH）；权限不足回退用户级 ~/.local/bin
        let system = std::path::PathBuf::from("/usr/local/bin");
        if let Ok(r) = install_cli_into(&system, &exe) {
            record_audit(r["path"].as_str().unwrap_or_default(), "");
            return Ok(r);
        }
        let home = std::env::var_os("HOME").ok_or("无法确定用户主目录")?;
        let user = std::path::PathBuf::from(home).join(".local").join("bin");
        let r = install_cli_into(&user, &exe)?;
        let hint = r["hint"].as_str().unwrap_or_default().to_string();
        record_audit(r["path"].as_str().unwrap_or_default(), &hint);
        Ok(r)
    }
}

/// 把可执行文件（符号链接 / Windows 启动脚本）放进指定目录，目录不存在则创建
fn install_cli_into(dir: &std::path::Path, exe: &std::path::Path) -> Result<serde_json::Value, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建目录 {} 失败: {e}", dir.display()))?;

    #[cfg(target_os = "windows")]
    {
        let launcher = dir.join("bit.cmd");
        let script = format!("@echo off\r\nstart \"\" /b /wait \"{}\" tui %*\r\n", exe.display());
        std::fs::write(&launcher, script).map_err(|e| format!("写入启动器失败: {e}"))?;
        Ok(serde_json::json!({ "path": launcher.display().to_string(), "hint": "重新打开终端后生效，运行 bit tui 进入终端模式" }))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let link = dir.join("bit");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(exe, &link).map_err(|e| format!("创建符号链接失败: {e}"))?;
        let hint = if dir.starts_with("/usr") {
            String::new()
        } else {
            format!("请确保 {} 在 PATH 中（写入 shell 配置后重开终端）", dir.display())
        };
        Ok(serde_json::json!({ "path": link.display().to_string(), "hint": hint }))
    }
}

/// 安装 `bit` 终端命令（设置页 / TUI 均可触发）
#[tauri::command]
pub fn install_cli(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    install_cli_impl(&ctx(state))
}

/// 自动更新检测结果
#[derive(serde::Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub has_update: bool,
    pub notes: String,
    pub url: String,
    /// 已下载到升级目录（可点击直接重启换装）
    pub downloaded: bool,
}

/// 版本号比较：a 是否大于 b（按数字段逐位比较）
pub fn version_gt(a: &str, b: &str) -> bool {
    let pa: Vec<u64> = a
        .trim_start_matches('v')
        .split('.')
        .filter_map(|x| x.parse().ok())
        .collect();
    let pb: Vec<u64> = b
        .trim_start_matches('v')
        .split('.')
        .filter_map(|x| x.parse().ok())
        .collect();
    for i in 0..3 {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

/// 自动更新检测：镜像 latest.json（GitHub Pages → osbt.space，国内直连可达），回退 GitHub API。
/// BIT_FAKE_UPDATE_URL 环境变量可将检测源替换为测试注入的地址（e2e 用）。
#[tauri::command]
pub async fn check_updates(app: tauri::AppHandle) -> Result<UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    let latest = crate::update::fetch_latest().await?;
    let has_update = version_gt(&latest.version, &current);
    let downloaded = app
        .try_state::<Arc<Ctx>>()
        .map(|s| crate::update::read_state(&s))
        .unwrap_or(None)
        .is_some_and(|st| {
            st["version"] == latest.version.as_str() && st["state"] == "downloaded"
        });
    Ok(UpdateInfo {
        current,
        latest: latest.version,
        has_update,
        notes: latest.notes,
        url: latest.url,
        downloaded,
    })
}

/// 手动触发下载当前平台更新包（启动后台任务会自动下；此处供 pill/远程 API 主动调用）
#[tauri::command]
pub async fn update_download(app: tauri::AppHandle, state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let status = crate::update::download_update(&ctx).await?;
    let _ = app.emit("update-state", status.clone());
    Ok(status)
}

/// 应用已下载的更新：换装并重启（托盘退出时由 quit 处理器静默换装，不重启）
#[tauri::command]
pub async fn update_apply(app: tauri::AppHandle, state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let msg = crate::update::apply_update(&ctx, true)?;
    let _ = app.emit("update-applied", serde_json::json!({ "msg": msg }));
    // 更新换装属正常重启：先通知守护进程不要按旧哈希接力，避免误报篡改
    crate::guardian::expect_exit(&ctx);
    // 给事件一点送达时间后重启进程（macOS/Linux 已换装；Windows 安装器静默跑）
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    app.restart();
    #[allow(unreachable_code)]
    Ok(serde_json::json!({ "state": "restarting", "msg": msg }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_crash_tail_roundtrip() {
        // crash.log JSONL 追加 → tail 倒序读取；损坏行跳过
        let dir = std::env::temp_dir().join(format!("bit-crash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("crash.log"),
            concat!(
                "{\"time\":\"t1\",\"thread\":\"main\",\"msg\":\"first\",\"loc\":\"a.rs:1:1\",\"backtrace\":\"bt\"}\n",
                "corrupted line\n",
                "{\"time\":\"t2\",\"thread\":\"worker\",\"msg\":\"second\",\"loc\":\"\",\"backtrace\":\"\"}\n"
            ),
        )
        .unwrap();
        let v = crate::crash::tail(&dir, 5);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["msg"], "second");
        assert_eq!(v[1]["msg"], "first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_clean_display_path_passthrough() {
        // 非 Windows：canonicalize 后原样转字符串（Windows 分支的转换由 test_clean_display_path_verbatim 覆盖）
        let tmp = std::env::temp_dir().join(format!("bit-cdp-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let f = tmp.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let c = std::fs::canonicalize(&f).unwrap();
        assert_eq!(super::clean_display_path(&c), c.to_string_lossy());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_clean_display_path_verbatim() {
        // \\?\C:\a\b.txt → C:\a\b.txt（explorer /select 无法解析 verbatim 前缀）
        assert_eq!(super::clean_display_path(std::path::Path::new(r"\\?\C:\a\b.txt")), r"C:\a\b.txt");
        // \\?\UNC\srv\share\f.txt → \\srv\share\f.txt
        assert_eq!(super::clean_display_path(std::path::Path::new(r"\\?\UNC\srv\share\f.txt")), r"\\srv\share\f.txt");
        // 正斜杠统一为反斜杠
        assert_eq!(super::clean_display_path(std::path::Path::new(r"\\?\C:/a/b.txt")), r"C:\a\b.txt");
        // 普通路径原样保留
        assert_eq!(super::clean_display_path(std::path::Path::new(r"C:\a\b.txt")), r"C:\a\b.txt");
    }

    /// 外部集成测试：连接独立运行的 mock AI（e2e/mock-ai.cjs，默认 127.0.0.1:9901），
    /// 走真实 TCP 验证 OpenAI 兼容 /models 拉取逻辑。
    /// 仅在设置 BIT_FAKE_OPENAI_URL 环境变量时运行：
    ///   BIT_FAKE_OPENAI_URL=http://127.0.0.1:9901/v1 cargo test list_models_from_fake_openai
    #[tokio::test]
    async fn test_list_models_from_fake_openai() {
        let Ok(base) = std::env::var("BIT_FAKE_OPENAI_URL") else {
            eprintln!("跳过：未设置 BIT_FAKE_OPENAI_URL");
            return;
        };
        let (effective, models) = super::fetch_provider_models("openai", &base, "")
            .await
            .unwrap();
        let ids: Vec<&str> = models.iter().map(|(id, _)| id.as_str()).collect();
        assert!(
            ids.contains(&"mock-model-a") && ids.contains(&"mock-model-b"),
            "应返回 mock 的模型列表: {models:?}"
        );
        // 自动检测：无论 env 里写没写 /v1，生效 base 都应是能拉到列表的那个端点
        let norm = base.trim_end_matches('/').to_string();
        let expect = if norm.ends_with("/v1") { norm } else { format!("{norm}/v1") };
        assert_eq!(effective, expect, "生效 base 应为自动检测后的端点");
    }
}
