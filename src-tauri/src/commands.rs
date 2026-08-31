use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

use crate::state::Ctx;

fn ctx<'a>(state: State<'a, Arc<Ctx>>) -> Arc<Ctx> {
    state.inner().clone()
}

// ---------- 概览 ----------

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
    crate::registry::invoke(&ctx, &id, params, "local-user").await
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

#[tauri::command]
pub async fn chat(
    state: State<'_, Arc<Ctx>>,
    session_id: String,
    message: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let messages = crate::agent::chat_turn(&ctx, &session_id, &message).await?;
    Ok(json!({ "messages": messages }))
}

/// 流式对话：过程通过 Tauri 事件 `event_name` 推送增量，返回最终完整消息列表
#[tauri::command]
pub async fn chat_stream(
    state: State<'_, Arc<Ctx>>,
    session_id: String,
    message: String,
    event_name: String,
) -> Result<serde_json::Value, String> {
    let ctx = ctx(state);
    let ev = if event_name.trim().is_empty() { "chat-stream".to_string() } else { event_name };
    let messages = crate::agent::chat_turn_stream(&ctx, &session_id, &message, &ev).await?;
    Ok(json!({ "messages": messages }))
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
    let g = crate::goal::create_goal(&ctx, &title, &detail, "user")?;
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
    let t = crate::goal::add_todo(&ctx, goal_id, &content, "user")?;
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
