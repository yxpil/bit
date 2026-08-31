use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 工具类型：
/// - Builtin: BIT 出厂内置实现（shell / write_file / plan / edit / add_tool）
/// - Remote: Agent 注册的回调端点，BIT 调用该 URL 执行
/// - Script: AI 自己编写、自己注册的 Rhai 插件脚本
/// - Interpreter: 用本机解释器（node / python / …）执行的脚本工具
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolKind {
    Builtin { handler: String },
    Remote { url: String },
    Script { code: String },
    Interpreter { runtime: String, code: String },
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ToolDef {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 类 JSON Schema 的参数描述
    pub parameters: serde_json::Value,
    pub kind: ToolKind,
    pub created_by: String,
    pub created_at: String,
    /// 是否启用。暂停（false）后 AI 与远程都不能调用，但保留定义。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

pub fn builtin_tools() -> Vec<ToolDef> {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mk = |id: &str, name: &str, desc: &str, params: serde_json::Value, handler: &str| ToolDef {
        id: id.into(),
        name: name.into(),
        description: desc.into(),
        parameters: params,
        kind: ToolKind::Builtin { handler: handler.into() },
        created_by: "system".into(),
        created_at: now.clone(),
        enabled: true,
    };
    vec![
        // 1. 命令行
        mk(
            "builtin.shell",
            "shell",
            "执行一条命令行（系统 shell），返回 stdout/stderr 与退出码",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要执行的命令" },
                    "cwd": { "type": "string", "description": "工作目录（可选）" }
                },
                "required": ["command"]
            }),
            "shell",
        ),
        // 2. 文档编辑（写 / 覆盖整个文件）
        mk(
            "builtin.write_file",
            "write_file",
            "写入或覆盖一个文件（文档编辑）。用于创建文件或整体替换内容",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "目标文件绝对路径" },
                    "content": { "type": "string", "description": "文件完整内容" }
                },
                "required": ["path", "content"]
            }),
            "write_file",
        ),
        // 3. 制定计划（目标 + 待办清单）
        mk(
            "builtin.plan",
            "plan",
            "制定计划：给出一个目标标题与若干步骤，自动登记为目标与待办清单",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "计划总目标" },
                    "steps": { "type": "array", "items": { "type": "string" }, "description": "分步待办" }
                },
                "required": ["goal", "steps"]
            }),
            "plan",
        ),
        // 4. edit：增量补丁修改文件
        mk(
            "builtin.edit",
            "edit",
            "增量补丁修改文件：把文件中的 old_string 精确替换为 new_string（不重写整份文件）",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "目标文件绝对路径" },
                    "old_string": { "type": "string", "description": "要被替换的原文（需唯一匹配）" },
                    "new_string": { "type": "string", "description": "替换后的新文本" },
                    "replace_all": { "type": "boolean", "description": "是否替换全部匹配（默认 false）" }
                },
                "required": ["path", "old_string", "new_string"]
            }),
            "edit",
        ),
        // 5. 给自己增加工具（用本机解释器把一段代码沉淀为常驻工具）
        mk(
            "builtin.add_tool",
            "add_tool",
            "给自己增加工具：用本机某个解释器把一段代码沉淀为常驻工具，之后可反复调用",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "新工具名称（唯一）" },
                    "description": { "type": "string", "description": "工具用途描述" },
                    "runtime": { "type": "string", "description": "解释器 id（见本机可用解释器清单）" },
                    "code": { "type": "string", "description": "该语言代码：从 stdin 读参数 JSON，把结果打印到 stdout" }
                },
                "required": ["name", "runtime", "code"]
            }),
            "add_tool",
        ),
        // 6. SKILL：写入 / 搜索技能
        mk(
            "builtin.skill",
            "skill",
            "技能库：写入一条可复用技能（action=save）或按关键词搜索已有技能（action=search）",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["save", "search"], "description": "save=写入，search=搜索" },
                    "name": { "type": "string", "description": "action=save 时：技能名称（同名覆盖）" },
                    "summary": { "type": "string", "description": "action=save 时：技能内容/步骤总结" },
                    "query": { "type": "string", "description": "action=search 时：搜索关键词（留空返回全部）" }
                },
                "required": ["action"]
            }),
            "skill",
        ),
    ]
}

/// 注册新工具。name 唯一；Remote 需要回调 URL。
pub fn register(
    ctx: &Arc<crate::state::Ctx>,
    name: &str,
    description: &str,
    parameters: serde_json::Value,
    kind: ToolKind,
    actor: &str,
) -> Result<ToolDef, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("工具名称不能为空".into());
    }
    let mut tools = ctx.tools.lock().unwrap();
    if tools.iter().any(|t| t.name.eq_ignore_ascii_case(name)) {
        return Err(format!("工具 `{name}` 已存在"));
    }
    let tool = ToolDef {
        id: format!("tool-{}", uuid::Uuid::new_v4().simple()),
        name: name.to_string(),
        description: description.trim().to_string(),
        parameters: if parameters.is_null() {
            serde_json::json!({"type": "object", "properties": {}})
        } else {
            parameters
        },
        kind,
        created_by: actor.to_string(),
        created_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        enabled: true,
    };
    tools.push(tool.clone());
    drop(tools);
    ctx.save_tools();
    Ok(tool)
}

pub fn remove(ctx: &Arc<crate::state::Ctx>, id: &str) -> Result<String, String> {
    let mut tools = ctx.tools.lock().unwrap();
    let before = tools.len();
    tools.retain(|t| t.id != id);
    if tools.len() == before {
        return Err(format!("工具 `{id}` 不存在"));
    }
    drop(tools);
    ctx.save_tools();
    Ok(id.to_string())
}

/// 暂停 / 启用工具。返回该工具最新的 enabled 状态。
pub fn set_enabled(ctx: &Arc<crate::state::Ctx>, id: &str, enabled: bool) -> Result<bool, String> {
    let mut tools = ctx.tools.lock().unwrap();
    let tool = tools
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("工具 `{id}` 不存在"))?;
    tool.enabled = enabled;
    drop(tools);
    ctx.save_tools();
    crate::audit::record(
        ctx,
        "local-app",
        if enabled { "tool.enable" } else { "tool.disable" },
        id,
        serde_json::json!({ "enabled": enabled }),
        true,
    );
    Ok(enabled)
}

/// 执行工具：内置实现或转发到 Agent 回调端点
pub async fn invoke(
    ctx: &Arc<crate::state::Ctx>,
    id: &str,
    params: serde_json::Value,
    actor: &str,
) -> Result<serde_json::Value, String> {
    let tool = {
        let tools = ctx.tools.lock().unwrap();
        tools
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| format!("工具 `{id}` 不存在"))?
    };

    if !tool.enabled {
        return Err(format!("工具 `{}` 已暂停，请先在「工具」页启用", tool.name));
    }

    let result = match &tool.kind {
        ToolKind::Builtin { handler } => builtin_invoke(ctx, handler, &params, actor).await,
        ToolKind::Remote { url } => {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client
                .post(url)
                .json(&serde_json::json!({
                    "tool_id": tool.id,
                    "tool": tool.name,
                    "invoked_by": actor,
                    "params": params,
                }))
                .send()
                .await
                .map_err(|e| format!("回调失败: {e}"))?;
            let status = resp.status().as_u16();
            let value: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            if status >= 400 {
                Err(format!("回调返回 HTTP {status}: {value}"))
            } else {
                Ok(value)
            }
        }
        ToolKind::Script { code } => {
            // 在阻塞线程池中执行 Rhai 沙盒脚本，整体限时 30 秒
            let code = code.clone();
            let params_owned = params.clone();
            let handle = tauri::async_runtime::spawn_blocking(move || {
                crate::script::run(&code, params_owned)
            });
            match tokio::time::timeout(std::time::Duration::from_secs(30), handle).await {
                Ok(res) => res.map_err(|e| format!("脚本任务失败: {e}"))?,
                Err(_) => Err("脚本执行超时（30 秒）".into()),
            }
        }
        ToolKind::Interpreter { runtime, code } => {
            // 通过本机解释器（node/python/…）执行，限时 30 秒
            let ctx_cloned = ctx.clone();
            let runtime = runtime.clone();
            let code = code.clone();
            let params_owned = params.clone();
            let handle = tauri::async_runtime::spawn_blocking(move || {
                crate::script_runtime::run(&ctx_cloned, &runtime, &code, &params_owned)
            });
            match tokio::time::timeout(std::time::Duration::from_secs(30), handle).await {
                Ok(res) => res.map_err(|e| format!("脚本任务失败: {e}"))?,
                Err(_) => Err("脚本执行超时（30 秒）".into()),
            }
        }
    };

    crate::audit::record(
        ctx,
        actor,
        "tool.invoke",
        &tool.name,
        serde_json::json!({ "tool_id": tool.id, "params": params }),
        result.is_ok(),
    );
    result
}

/// 五个出厂内置工具的真实实现
async fn builtin_invoke(
    ctx: &Arc<crate::state::Ctx>,
    handler: &str,
    params: &serde_json::Value,
    actor: &str,
) -> Result<serde_json::Value, String> {
    match handler {
        // ── 1. 命令行 ──
        "shell" => {
            let command = params
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or("缺少参数 command")?
                .to_string();
            let cwd = params.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string());
            let handle = tauri::async_runtime::spawn_blocking(move || {
                let mut cmd = if cfg!(windows) {
                    let mut c = std::process::Command::new("powershell");
                    c.args(["-NoProfile", "-NonInteractive", "-Command", &command]);
                    c
                } else {
                    let mut c = std::process::Command::new("sh");
                    c.args(["-c", &command]);
                    c
                };
                if let Some(dir) = &cwd {
                    cmd.current_dir(dir);
                }
                cmd.output()
            });
            let out = match tokio::time::timeout(std::time::Duration::from_secs(60), handle).await {
                Ok(res) => res.map_err(|e| format!("命令任务失败: {e}"))?,
                Err(_) => return Err("命令执行超时（60 秒）".into()),
            };
            let out = out.map_err(|e| format!("启动命令失败: {e}"))?;
            let trunc = |s: String| if s.len() > 6000 { s[..6000].to_string() } else { s };
            Ok(serde_json::json!({
                "code": out.status.code(),
                "stdout": trunc(String::from_utf8_lossy(&out.stdout).to_string()),
                "stderr": trunc(String::from_utf8_lossy(&out.stderr).to_string()),
            }))
        }
        // ── 2. 文档编辑（写 / 覆盖）──
        "write_file" => {
            let path = params.get("path").and_then(|v| v.as_str()).ok_or("缺少参数 path")?;
            let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(parent) = std::path::Path::new(path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, content).map_err(|e| format!("写入失败: {e}"))?;
            Ok(serde_json::json!({ "path": path, "bytes": content.len() }))
        }
        // ── 3. 制定计划（目标 + 待办）──
        "plan" => {
            let goal = params.get("goal").and_then(|v| v.as_str()).ok_or("缺少参数 goal")?;
            let steps: Vec<String> = params
                .get("steps")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            let g = crate::goal::create_goal(ctx, goal, "", actor)?;
            let mut ids = Vec::new();
            for s in &steps {
                if let Ok(t) = crate::goal::add_todo(ctx, Some(g.id.clone()), s, actor) {
                    ids.push(t.id);
                }
            }
            Ok(serde_json::json!({ "goal_id": g.id, "goal": g.title, "todos": ids.len() }))
        }
        // ── 4. edit：增量补丁 ──
        "edit" => {
            let path = params.get("path").and_then(|v| v.as_str()).ok_or("缺少参数 path")?;
            let old = params.get("old_string").and_then(|v| v.as_str()).ok_or("缺少参数 old_string")?;
            let new = params.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
            let replace_all = params.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
            if old.is_empty() {
                return Err("old_string 不能为空".into());
            }
            let text = std::fs::read_to_string(path).map_err(|e| format!("读取失败: {e}"))?;
            let count = text.matches(old).count();
            if count == 0 {
                return Err("未找到 old_string，无法替换".into());
            }
            if count > 1 && !replace_all {
                return Err(format!("old_string 匹配到 {count} 处，请提供更精确的上下文，或设 replace_all=true"));
            }
            let updated = if replace_all { text.replace(old, new) } else { text.replacen(old, new, 1) };
            std::fs::write(path, &updated).map_err(|e| format!("写回失败: {e}"))?;
            Ok(serde_json::json!({ "path": path, "replaced": if replace_all { count } else { 1 } }))
        }
        // ── 5. 给自己增加工具 ──
        "add_tool" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let desc = params.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let runtime = params.get("runtime").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let code = params.get("code").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if name.is_empty() || runtime.is_empty() || code.is_empty() {
                return Err("add_tool 需要 name / runtime / code 参数".into());
            }
            match crate::runtime::get(ctx, &runtime) {
                None => return Err(format!("解释器 `{runtime}` 未注册，请先在「工具」页刷新探测")),
                Some(rt) if !rt.enabled => return Err(format!("解释器 `{runtime}` 已暂停，无法用于新工具")),
                _ => {}
            }
            let tool = register(
                ctx,
                &name,
                &desc,
                serde_json::json!({"type": "object", "properties": {}, "additionalProperties": true}),
                ToolKind::Interpreter { runtime: runtime.clone(), code },
                actor,
            )?;
            Ok(serde_json::json!({ "registered": tool.name, "id": tool.id, "runtime": runtime }))
        }
        // ── 6. SKILL：写入 / 搜索技能 ──
        "skill" => {
            let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("search");
            match action {
                "save" => {
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                    let summary = params.get("summary").and_then(|v| v.as_str()).unwrap_or_default();
                    if name.trim().is_empty() || summary.trim().is_empty() {
                        return Err("skill(save) 需要 name 与 summary".into());
                    }
                    let s = crate::memory::add_skill(ctx, name, summary, actor);
                    Ok(serde_json::json!({ "saved": s.name, "id": s.id }))
                }
                "search" => {
                    let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                    let skills = ctx.skills.lock().unwrap();
                    let hits: Vec<serde_json::Value> = skills
                        .iter()
                        .filter(|s| {
                            query.is_empty()
                                || s.name.to_lowercase().contains(&query)
                                || s.summary.to_lowercase().contains(&query)
                        })
                        .rev()
                        .take(20)
                        .map(|s| serde_json::json!({ "name": s.name, "summary": s.summary, "ts": s.ts }))
                        .collect();
                    Ok(serde_json::json!({ "count": hits.len(), "skills": hits }))
                }
                other => Err(format!("skill 的 action 只能是 save / search，收到 `{other}`")),
            }
        }
        other => Err(format!("未知内置处理器 `{other}`")),
    }
}
