use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

use crate::state::Ctx;

fn ctx<'a>(state: State<'a, Arc<Ctx>>) -> Arc<Ctx> {
    state.inner().clone()
}

// ---------- 概览 ----------

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
        let host = host.trim().to_string();
        if host.is_empty() {
            return Err("监听地址不能为空".into());
        }
        if port < 1024 {
            return Err("端口需不小于 1024".into());
        }
        cfg.host = host;
        cfg.port = port;
        cfg.revision += 1; // 每次保存自动递增版本号
        ctx.save_config();
    }
    crate::audit::record(&ctx, "local-user", "remote.save", "config", json!({ "revision": ctx.config.lock().unwrap().revision }), true);
    let addr = crate::http_api::restart_server(&ctx).await?;
    // 远程地址变化，同步托盘菜单显示
    crate::tray::refresh(&ctx.app);
    Ok(json!({ "addr": addr }))
}

#[tauri::command]
pub fn regenerate_client_key(state: State<'_, Arc<Ctx>>) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let key = {
        let mut cfg = ctx.config.lock().unwrap();
        let key = cfg.new_client_key();
        cfg.revision += 1;
        ctx.save_config();
        key
    };
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
        ctx.save_config();
    }
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
        ctx.save_config();
        pwd
    };
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
    ctx.save_ai_config();
    crate::audit::record(&ctx, "local-user", "ai.provider.remove", &id, json!({}), true);
    Ok(json!({ "removed": true }))
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
        crate::agent::chat_turn_stream(&ctx, &session_id, &message, &ev, images.unwrap_or_default()).await?;
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
    let sender = ctx.approvals.lock().unwrap().remove(&id);
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

/// AI 接收信息预览：当前会话实际发给模型的 system prompt / 消息 / 工具清单
#[tauri::command]
pub async fn context_preview(state: State<'_, Arc<Ctx>>, session_id: String) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let (convo, tools) = crate::agent::build_context(&ctx, &session_id)?;
    // 粗略 token 估算（约 2 字符/token）
    let chars: usize = convo.iter().map(|m| m.content.chars().count()).sum();
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
        "est_tokens": chars / 2,
        "approval_mode": ctx.config.lock().unwrap().tool_approval.clone(),
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
    let addr = format!("{}:{}", host.trim(), port);
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
