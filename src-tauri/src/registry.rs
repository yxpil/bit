use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 工具类型：
/// - Builtin: BIT 出厂内置实现（shell / write_file / plan / edit / add_tool）
/// - Remote: Agent 注册的回调端点，BIT 调用该 URL 执行
/// - Script: AI 自己编写、自己注册的 Rhai 插件脚本
/// - Interpreter: 用本机解释器（node / python / …）执行的脚本工具
/// - Mcp: 从 MCP（Model Context Protocol）服务器导入的工具，经 JSON-RPC 调用
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolKind {
    Builtin { handler: String },
    Remote { url: String },
    Script { code: String },
    Interpreter { runtime: String, code: String },
    Mcp { server_id: String, tool: String },
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
            "Execute a shell command (system shell) and return stdout/stderr with exit code",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command to execute" },
                    "cwd": { "type": "string", "description": "Working directory (optional)" }
                },
                "required": ["command"]
            }),
            "shell",
        ),
        // 2. 文档编辑（写 / 覆盖整个文件）
        mk(
            "builtin.write_file",
            "write_file",
            "Write or overwrite a file. Use it to create a file or replace the whole content",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path of the target file" },
                    "content": { "type": "string", "description": "Full file content" }
                },
                "required": ["path", "content"]
            }),
            "write_file",
        ),
        // 3. 制定计划（目标 + 待办清单）
        mk(
            "builtin.plan",
            "plan",
            "Create a plan: register a goal title with steps as a goal plus a todo list",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "The overall goal of the plan" },
                    "steps": { "type": "array", "items": { "type": "string" }, "description": "Todo items, one per step" }
                },
                "required": ["goal", "steps"]
            }),
            "plan",
        ),
        // 4. edit：增量补丁修改文件
        mk(
            "builtin.edit",
            "edit",
            "Patch a file in place: replace an exact old_string with new_string (no full rewrite)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path of the target file" },
                    "old_string": { "type": "string", "description": "Exact original text to replace (must match uniquely)" },
                    "new_string": { "type": "string", "description": "Replacement text" },
                    "replace_all": { "type": "boolean", "description": "Replace all occurrences (default false)" }
                },
                "required": ["path", "old_string", "new_string"]
            }),
            "edit",
        ),
        // 5. 给自己增加工具（用本机解释器把一段代码沉淀为常驻工具）
        mk(
            "builtin.add_tool",
            "add_tool",
            "Create a persistent tool from a code snippet executed by a local interpreter; reusable afterwards",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "New tool name (unique)" },
                    "description": { "type": "string", "description": "What the tool does" },
                    "runtime": { "type": "string", "description": "Interpreter id (see the available runtimes list)" },
                    "code": { "type": "string", "description": "Code in that language: read the params JSON from stdin and print the result JSON to stdout" }
                },
                "required": ["name", "runtime", "code"]
            }),
            "add_tool",
        ),
        // 5.5 子智能体：派生独立会话执行子任务
        mk(
            "builtin.sub_agent",
            "sub_agent",
            "Spawn a sub-agent: opens an independent session with the full task. The sub-agent has ALL your tools and runs autonomously (multi-turn, including file read/write and shell) until it produces a final answer. Ideal for delegating independent large tasks (research, batch processing, writing large files, parallel branches). This tool blocks and returns the sub-agent's final answer VERBATIM into the current conversation - no file location conventions needed; just continue from the returned content. The task must be self-contained: the sub-agent cannot see the current conversation history, so include background, goal and acceptance criteria",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "Complete task description for the sub-agent (self-contained: background + goal + acceptance criteria)" },
                    "title": { "type": "string", "description": "Sub-session title (optional)" }
                },
                "required": ["task"]
            }),
            "sub_agent",
        ),
        // 5.5 删除自己创建的工具（自建解释器/脚本可删；内置、远程、MCP 禁删）
        mk(
            "builtin.delete_tool",
            "delete_tool",
            "Delete a tool you created via add_tool (interpreter/script tools). Builtin, remote and MCP tools cannot be deleted",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Name of the tool to delete" }
                },
                "required": ["name"]
            }),
            "delete_tool",
        ),
        // 5.6 截断历史：保留最近 N 条，其余丢弃
        mk(
            "builtin.truncate_history",
            "truncate_history",
            "Truncate this session's history: keep only the most recent `keep` messages (default 12); earlier content is lost permanently. Use proactively when the history is long and earlier content is no longer valuable",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "keep": { "type": "integer", "description": "How many recent messages to keep (default 12, min 2)" }
                }
            }),
            "truncate_history",
        ),
        // 5.7 压缩对话：用一段摘要替换全部历史（保留最近 2 条现场）
        mk(
            "builtin.compact_history",
            "compact_history",
            "Compact this session: replace all prior history with a summary you write (the last 2 messages are kept verbatim). The summary must include: key conclusions, important decisions, unfinished items, next steps",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "description": "Complete summary of the prior conversation" }
                },
                "required": ["summary"]
            }),
            "compact_history",
        ),
        // 6. SKILL：写入 / 搜索技能
        mk(
            "builtin.skill",
            "skill",
            "Skill library: save a reusable skill (action=save) or search existing skills by keyword (action=search)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["save", "search"], "description": "save=write, search=search" },
                    "name": { "type": "string", "description": "For action=save: skill name (same name overwrites)" },
                    "summary": { "type": "string", "description": "For action=save: skill content / step summary" },
                    "query": { "type": "string", "description": "For action=search: search keyword (empty returns all)" }
                },
                "required": ["action"]
            }),
            "skill",
        ),
        // 7. 发送文件给用户（聊天里出现可打开的文件卡片）
        mk(
            "builtin.send_file",
            "send_file",
            "Deliver an existing file to the user: a clickable file card appears in the chat. Use for results you produced (reports, HTML, images, data files...)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path of the file to send" },
                    "note": { "type": "string", "description": "One-line note (optional)" }
                },
                "required": ["path"]
            }),
            "send_file",
        ),
        // 7.5 看图：把本地图片喂给视觉模型（图片本体注入下一轮请求，不占工具结果文本）
        mk(
            "builtin.view_image",
            "view_image",
            "View a local image: the image is injected into the next turn as visual content that vision models (GPT, Gemini, Claude, etc.) can see directly. Supports png/jpg/jpeg/webp/gif/bmp",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path of the image file" },
                    "note": { "type": "string", "description": "What to focus on (optional, passed as a viewing hint)" }
                },
                "required": ["path"]
            }),
            "view_image",
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
    register_opts(ctx, name, description, parameters, kind, actor, false)
}

/// 注册或覆盖。overwrite=true 时允许更新 AI 自建的解释器/脚本工具（同 id 原位更新）。
pub fn register_opts(
    ctx: &Arc<crate::state::Ctx>,
    name: &str,
    description: &str,
    parameters: serde_json::Value,
    kind: ToolKind,
    actor: &str,
    overwrite: bool,
) -> Result<ToolDef, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Tool name cannot be empty".into());
    }
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut tools = ctx.tools.lock().unwrap();
    if let Some(existing) = tools.iter_mut().find(|t| t.name.eq_ignore_ascii_case(name)) {
        if !overwrite {
            return Err(format!("Tool `{name}` already exists"));
        }
        // 仅允许覆盖 AI 自建的工具（解释器 / 脚本）；内置、远程、MCP 工具不可覆盖
        if !matches!(existing.kind, ToolKind::Interpreter { .. } | ToolKind::Script { .. }) {
            return Err(format!("Tool `{name}` is a system/remote tool and cannot be overwritten; choose a different name"));
        }
        existing.description = description.trim().to_string();
        existing.parameters = if parameters.is_null() {
            serde_json::json!({"type": "object", "properties": {}})
        } else {
            parameters
        };
        existing.kind = kind;
        existing.created_by = actor.to_string();
        existing.created_at = now;
        let updated = existing.clone();
        drop(tools);
        ctx.save_tools();
        return Ok(updated);
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
        created_at: now,
        enabled: true,
    };
    tools.push(tool.clone());
    drop(tools);
    ctx.save_tools();
    Ok(tool)
}

pub fn remove(ctx: &Arc<crate::state::Ctx>, id: &str) -> Result<String, String> {
    let mut tools = ctx.tools.lock().unwrap();
    // 内置工具是 BIT 出厂能力，不允许删除（用户 UI 与 AI delete_tool 都走这里）
    if tools
        .iter()
        .any(|t| t.id == id && matches!(t.kind, ToolKind::Builtin { .. }))
    {
        return Err("Builtin tools cannot be deleted".into());
    }
    let before = tools.len();
    tools.retain(|t| t.id != id);
    if tools.len() == before {
        return Err(format!("Tool `{id}` does not exist"));
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
        .ok_or_else(|| format!("Tool `{id}` does not exist"))?;
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
/// `session`：发起调用的会话 id（用于长任务感知主会话中断，可为 None）
pub async fn invoke(
    ctx: &Arc<crate::state::Ctx>,
    id: &str,
    params: serde_json::Value,
    actor: &str,
    session: Option<&str>,
) -> Result<serde_json::Value, String> {
    let tool = {
        let tools = ctx.tools.lock().unwrap();
        tools
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| format!("Tool `{id}` does not exist"))?
    };

    if !tool.enabled {
        return Err(format!("Tool `{}` is paused; enable it on the Tools page first", tool.name));
    }

    let result = match &tool.kind {
        ToolKind::Builtin { handler } => builtin_invoke(ctx, handler, &params, actor, session).await,
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
                .map_err(|e| format!("Callback failed: {e}"))?;
            let status = resp.status().as_u16();
            let value: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            if status >= 400 {
                Err(format!("Callback returned HTTP {status}: {value}"))
            } else {
                Ok(value)
            }
        }
        ToolKind::Mcp { server_id, tool } => {
            // 服务器级暂停/继续：暂停后该服务器全部工具拒绝调用
            let server = crate::mcp::find(ctx, server_id)
                .ok_or_else(|| format!("MCP server `{server_id}` is not connected"))?;
            if !server.enabled {
                return Err(format!(
                    "MCP server `{}` is paused; enable it on the Tools page first",
                    server.name
                ));
            }
            crate::mcp::call_tool(&server, tool, params.clone()).await
        }
        ToolKind::Script { code } => {
            // 在阻塞线程池中执行 Rhai 沙盒脚本，整体限时 30 秒
            let code = code.clone();
            let params_owned = params.clone();
            let handle = tauri::async_runtime::spawn_blocking(move || {
                crate::script::run(&code, params_owned)
            });
            match tokio::time::timeout(std::time::Duration::from_secs(30), handle).await {
                Ok(res) => res.map_err(|e| format!("Script task failed: {e}"))?,
                Err(_) => Err("Script execution timed out (30s)".into()),
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
                Ok(res) => res.map_err(|e| format!("Script task failed: {e}"))?,
                Err(_) => Err("Script execution timed out (30s)".into()),
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

/// 安全截断：回退到 UTF-8 字符边界，避免多字节字符（中文输出）字节切片 panic。
/// 供 registry / script_runtime / ai 等模块截断命令输出与错误信息共用
pub fn safe_trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Windows 下隐藏子进程控制台窗口（CREATE_NO_WINDOW），避免启动探测/工具执行时黑窗闪烁
#[cfg(windows)]
pub fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
pub fn no_window(_cmd: &mut std::process::Command) {}

/// 同 no_window，用于 tokio 进程
#[cfg(windows)]
pub fn no_window_tokio(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
pub fn no_window_tokio(_cmd: &mut tokio::process::Command) {}

/// 五个出厂内置工具的真实实现
async fn builtin_invoke(
    ctx: &Arc<crate::state::Ctx>,
    handler: &str,
    params: &serde_json::Value,
    actor: &str,
    session: Option<&str>,
) -> Result<serde_json::Value, String> {
    match handler {
        // ── 1. 命令行 ──
        "shell" => {
            let command = params
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or("Missing parameter: command")?
                .to_string();
            let cwd = params.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string());
            let handle = tauri::async_runtime::spawn(async move {
                let mut cmd = if cfg!(windows) {
                    // 强制 PowerShell 以 UTF-8 输出，避免中文被 GBK 编码成乱码
                    let mut c = tokio::process::Command::new("powershell");
                    c.args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        &format!("[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; {command}"),
                    ]);
                    c
                } else {
                    let mut c = tokio::process::Command::new("sh");
                    c.args(["-c", &command]);
                    c
                };
                if let Some(dir) = &cwd {
                    cmd.current_dir(dir);
                }
                no_window_tokio(&mut cmd);
                // 超时后 future 被 drop，kill_on_drop 确保子进程被终止而不是变孤儿继续跑
                cmd.kill_on_drop(true);
                cmd.output().await
            });
            let out = match tokio::time::timeout(std::time::Duration::from_secs(600), handle).await {
                Ok(res) => res.map_err(|e| format!("Command task failed: {e}"))?,
                Err(_) => return Err("Command execution timed out (600s)".into()),
            };
            let out = out.map_err(|e| format!("Failed to spawn command: {e}"))?;
            Ok(serde_json::json!({
                "code": out.status.code(),
                "stdout": safe_trunc(&String::from_utf8_lossy(&out.stdout), 60000),
                "stderr": safe_trunc(&String::from_utf8_lossy(&out.stderr), 60000),
            }))
        }
        // ── 2. 文档编辑（写 / 覆盖）──
        "write_file" => {
            let path = params.get("path").and_then(|v| v.as_str()).ok_or("Missing parameter: path")?;
            let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(parent) = std::path::Path::new(path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, content).map_err(|e| format!("Failed to write: {e}"))?;
            Ok(serde_json::json!({ "path": path, "bytes": content.len() }))
        }
        // ── 2.5 发送文件给用户 ──
        "send_file" => {
            let path = params.get("path").and_then(|v| v.as_str()).ok_or("Missing parameter: path")?;
            let p = std::path::Path::new(path);
            let meta = std::fs::metadata(p).map_err(|_| format!("File not found: {path}"))?;
            if meta.is_dir() {
                return Err(format!("`{path}` is a directory; send_file only accepts a single file"));
            }
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from)
                .unwrap_or_else(|| path.to_string());
            let note = params.get("note").and_then(|v| v.as_str()).unwrap_or("");
            Ok(serde_json::json!({
                "sent": true,
                "path": path,
                "name": name,
                "bytes": meta.len(),
                "note": note,
            }))
        }
        // ── 2.6 看图：读取本地图片，data_url 由 agent 循环注入下一轮请求（视觉模型） ──
        "view_image" => {
            let path = params.get("path").and_then(|v| v.as_str()).ok_or("Missing parameter: path")?;
            let note = params.get("note").and_then(|v| v.as_str()).unwrap_or("");
            let p = std::path::Path::new(path);
            let meta = std::fs::metadata(p).map_err(|_| format!("File not found: {path}"))?;
            if meta.is_dir() {
                return Err(format!("`{path}` is a directory; view_image requires a single image file"));
            }
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            let mime = match ext.as_str() {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                "gif" => "image/gif",
                "bmp" => "image/bmp",
                other => return Err(format!("Unsupported image format `.{other}`; supported: png/jpg/jpeg/webp/gif/bmp")),
            };
            if meta.len() > 20 * 1024 * 1024 {
                return Err(format!("Image too large ({} MB); limit is 20 MB", meta.len() / 1024 / 1024));
            }
            let bytes = std::fs::read(p).map_err(|e| format!("Failed to read: {e}"))?;
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Ok(serde_json::json!({
                "seen": true,
                "path": path,
                "mime": mime,
                "bytes": meta.len(),
                "note": note,
                // agent 循环会把 data_url 抽出注入下一轮请求，并把本结果脱敏后再回喂模型
                "data_url": format!("data:{mime};base64,{b64}"),
            }))
        }
        // ── 3. 制定计划（目标 + 待办）──
        "plan" => {
            let goal = params.get("goal").and_then(|v| v.as_str()).ok_or("Missing parameter: goal")?;
            let steps: Vec<String> = params
                .get("steps")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            let g = crate::goal::create_goal(ctx, goal, "", actor, session)?;
            let mut ids = Vec::new();
            for s in &steps {
                if let Ok(t) = crate::goal::add_todo(ctx, Some(g.id.clone()), s, actor, session) {
                    ids.push(t.id);
                }
            }
            Ok(serde_json::json!({ "goal_id": g.id, "goal": g.title, "todos": ids.len() }))
        }
        // ── 4. edit：增量补丁 ──
        "edit" => {
            let path = params.get("path").and_then(|v| v.as_str()).ok_or("缺少参数 path")?;
            let old = params.get("old_string").and_then(|v| v.as_str()).ok_or("Missing parameter: old_string")?;
            let new = params.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
            let replace_all = params.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
            if old.is_empty() {
                return Err("old_string cannot be empty".into());
            }
            let text = std::fs::read_to_string(path).map_err(|e| format!("Failed to read: {e}"))?;
            // 精确匹配失败时自动适配换行风格（文件 CRLF / 模型给 LF，或反过来）
            let mut count = text.matches(old).count();
            let mut old_eff = old.to_string();
            let mut new_eff = new.to_string();
            if count == 0 {
                let old_norm = old.replace("\r\n", "\n");
                if old_norm != old && text.contains(&old_norm) {
                    old_eff = old_norm;
                    new_eff = new.replace("\r\n", "\n");
                    count = text.matches(&old_eff).count();
                } else if text.contains("\r\n") && old.contains('\n') {
                    let old_crlf = old_norm.replace('\n', "\r\n");
                    let c2 = text.matches(&old_crlf).count();
                    if c2 > 0 {
                        old_eff = old_crlf;
                        new_eff = new.replace("\r\n", "\n").replace('\n', "\r\n");
                        count = c2;
                    }
                }
            }
            if count == 0 {
                // 给模型有用的线索：old_string 首行是否在文件中存在
                let mut hint = String::new();
                if let Some(first) = old.lines().map(str::trim).find(|l| !l.is_empty()) {
                    if let Some(n) = text.lines().enumerate().find(|(_, l)| l.trim() == first).map(|(i, _)| i + 1) {
                        hint = format!("; a line matching the first line exists near line {n}; the difference may be in spaces/indentation or the following lines");
                    }
                }
                return Err(format!(
                    "old_string not found; nothing replaced{hint}. Read the file first to get its current content, make old_string match the file exactly (including spaces and newlines), then retry or use a larger context snippet",
                ));
            }
            if count > 1 && !replace_all {
                return Err(format!("old_string matched {count} locations; provide more precise context or set replace_all=true"));
            }
            let updated =
                if replace_all { text.replace(&old_eff, &new_eff) } else { text.replacen(&old_eff, &new_eff, 1) };
            std::fs::write(path, &updated).map_err(|e| format!("Failed to write back: {e}"))?;
            Ok(serde_json::json!({ "path": path, "replaced": if replace_all { count } else { 1 } }))
        }
        // ── 5. 子智能体：开新会话独立完成任务 ──
        "sub_agent" => {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static SUBAGENT_DEPTH: AtomicUsize = AtomicUsize::new(0);
            struct DecGuard;
            impl Drop for DecGuard {
                fn drop(&mut self) {
                    SUBAGENT_DEPTH.fetch_sub(1, Ordering::SeqCst);
                }
            }
            // 嵌套上限 3 层，防止子智能体再派生子智能体无限递归
            let depth = SUBAGENT_DEPTH.fetch_add(1, Ordering::SeqCst);
            let _guard = DecGuard;
            if depth >= 3 {
                return Err("Sub-agent nesting too deep (max 3 levels); complete the task directly in this session".into());
            }
            let task = params.get("task").and_then(|v| v.as_str()).ok_or("Missing parameter: task")?.to_string();
            if task.trim().is_empty() {
                return Err("task cannot be empty".into());
            }
            let title = params.get("title").and_then(|v| v.as_str()).unwrap_or("子任务").to_string();
            // 新建独立会话（用户可在侧栏看到全过程）
            let sess = crate::session::Session::new(&title);
            let sid = sess.id.clone();
            ctx.sessions.lock().unwrap().sessions.push(sess);
            crate::session::persist(ctx);
            {
                use tauri::Emitter;
                let _ = ctx.app.emit("sessions-updated", &sid);
            }
            // 阻塞执行子任务：完整复用 agent 循环（工具、审批、自动续发全部生效）。
            // Box::pin：builtin_invoke → chat_turn → execute_tool_call → builtin_invoke 递归，需手动打断无限大小
            let mut run = Box::pin(crate::agent::chat_turn(ctx, &sid, &task, Vec::new()));
            const SUB_TIMEOUT_SECS: u64 = 15 * 60;
            let mut sleep = tokio::time::sleep(std::time::Duration::from_secs(SUB_TIMEOUT_SECS));
            // 主会话点「停止」时立刻取消子任务，而不是干等子任务跑完
            let parent = session.map(|s| s.to_string());
            let mut watch = async {
                loop {
                    if parent
                        .as_deref()
                        .map(|p| crate::agent::interrupted(ctx, p))
                        .unwrap_or(false)
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
            };
            // Sleep 与 async block 均非 Unpin，select! 里用 &mut 前需固定
            tokio::pin!(sleep, watch);
            let outcome = loop {
                tokio::select! {
                    _ = &mut sleep => {
                        break Err(format!(
                            "Subtask timed out (15 min). Sub-session {sid} keeps the intermediate progress; you may keep waiting or inspect that session"
                        ));
                    }
                    res = &mut run => {
                        break match res {
                            Ok(msgs) => {
                                // 子代理结论直接完整返回给主会话（无需约定写文件/位置）。
                                // 仅设超高安全上限，防止异常巨文撑爆主会话上下文
                                const SUBANSWER_MAX: usize = 60_000;
                                let final_answer = msgs
                                    .iter()
                                    .rev()
                                    .find(|m| m.role == "assistant" && !m.content.trim().is_empty())
                                    .map(|m| m.content.clone())
                                    .unwrap_or_default();
                                let truncated = final_answer.chars().count() > SUBANSWER_MAX;
                                let answer = if truncated {
                                    safe_trunc(&final_answer, SUBANSWER_MAX)
                                } else {
                                    final_answer
                                };
                                Ok(serde_json::json!({
                                    "session_id": sid,
                                    "final_answer": answer,
                                    "truncated": truncated,
                                    "note": if truncated {
                                        "Answer truncated for length; see the sub-session for the full process and answer (you may send follow-up questions to it)"
                                    } else {
                                        "The sub-session keeps the full execution log; you may send follow-up questions to it"
                                    }
                                }))
                            }
                            Err(e) => Err(format!("Subtask failed: {e} (sub-session {sid} keeps the process log)")),
                        };
                    }
                    _ = &mut watch => {
                        // 同时清掉子会话自己的中断标记，避免下次对话误报「已中断」
                        crate::agent::clear_interrupt(ctx, &sid);
                        break Err(format!("Parent session interrupted; subtask stopped (sub-session {sid} keeps the progress)"));
                    }
                }
            };
            {
                use tauri::Emitter;
                let _ = ctx.app.emit("sessions-updated", &sid);
            }
            outcome
        }
        // ── 6. 给自己增加工具 ──
        "add_tool" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let desc = params.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let runtime = params.get("runtime").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let code = params.get("code").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if name.is_empty() || runtime.is_empty() || code.is_empty() {
                return Err("add_tool requires name / runtime / code parameters".into());
            }
            match crate::runtime::get(ctx, &runtime) {
                None => return Err(format!("Interpreter `{runtime}` is not registered; refresh the runtime detection on the Tools page first")),
                Some(rt) if !rt.enabled => return Err(format!("Interpreter `{runtime}` is paused; cannot be used for a new tool")),
                _ => {}
            }
            let tool = register_opts(
                ctx,
                &name,
                &desc,
                serde_json::json!({"type": "object", "properties": {}, "additionalProperties": true}),
                ToolKind::Interpreter { runtime: runtime.clone(), code },
                actor,
                true, // 同名工具若为 AI 自建则覆盖更新（修正错误实现）
            )?;
            Ok(serde_json::json!({ "registered": tool.name, "id": tool.id, "runtime": runtime }))
        }
        // ── 5.5 删除自己创建的工具 ──
        "delete_tool" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("delete_tool requires the name parameter")?;
            let target = {
                let tools = ctx.tools.lock().unwrap();
                tools
                    .iter()
                    .find(|t| t.name.eq_ignore_ascii_case(name))
                    .cloned()
            };
            let tool = target.ok_or_else(|| format!("Tool `{name}` does not exist"))?;
            // 仅允许删除 AI/用户自建的解释器与脚本工具；内置、远程、MCP 禁删
            if !matches!(tool.kind, ToolKind::Interpreter { .. } | ToolKind::Script { .. }) {
                return Err(format!("Tool `{name}` is a system/remote tool and cannot be deleted"));
            }
            remove(ctx, &tool.id)?;
            Ok(serde_json::json!({ "deleted": tool.name, "id": tool.id }))
        }
        // ── 5.6 截断历史：只保留最近 keep 条 ──
        "truncate_history" => {
            let sid = session.ok_or("Cannot determine the current session")?.to_string();
            let keep = params.get("keep").and_then(|v| v.as_u64()).unwrap_or(12).max(2) as usize;
            let (dropped, kept) = {
                let mut store = ctx.sessions.lock().unwrap();
                let sess = store.get_mut(&sid).ok_or("Session not found")?;
                let total = sess.messages.len();
                if total > keep {
                    let cut = total - keep;
                    sess.messages.drain(0..cut);
                    sess.touch();
                    (cut, keep)
                } else {
                    (0, total)
                }
            };
            if dropped > 0 {
                crate::session::persist(ctx);
                use tauri::Emitter;
                let _ = ctx.app.emit("sessions-updated", &sid);
            }
            Ok(serde_json::json!({ "truncated": true, "dropped": dropped, "kept": kept }))
        }
        // ── 5.7 压缩对话：摘要替换历史，保留最近 2 条现场 ──
        "compact_history" => {
            let sid = session.ok_or("Cannot determine the current session")?.to_string();
            let summary = params
                .get("summary")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("compact_history requires the summary parameter (a complete summary of the prior conversation)")?;
            let (dropped, kept) = {
                let mut store = ctx.sessions.lock().unwrap();
                let sess = store.get_mut(&sid).ok_or("Session not found")?;
                let total = sess.messages.len();
                let keep_tail = 2.min(total);
                // 尾部现场先摘出来，历史整体替换为一条摘要消息
                let mut kept_msgs: Vec<crate::ai::ChatMessage> =
                    sess.messages.split_off(total - keep_tail);
                kept_msgs.insert(
                    0,
                    crate::ai::ChatMessage::user(format!(
                        "(Prior conversation has been compacted into a summary; continue based on the summary and the messages below.)\nHistory summary: {summary}"
                    )),
                );
                let dropped = total - keep_tail;
                sess.messages = kept_msgs;
                sess.touch();
                (dropped, keep_tail + 1)
            };
            crate::session::persist(ctx);
            use tauri::Emitter;
            let _ = ctx.app.emit("sessions-updated", &sid);
            Ok(serde_json::json!({ "compacted": true, "dropped": dropped, "kept": kept }))
        }
        // ── 6. SKILL：写入 / 搜索技能 ──
        "skill" => {
            let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("search");
            match action {
                "save" => {
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                    let summary = params.get("summary").and_then(|v| v.as_str()).unwrap_or_default();
                    if name.trim().is_empty() || summary.trim().is_empty() {
                        return Err("skill(save) requires name and summary".into());
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
                other => Err(format!("skill action must be save or search, got `{other}`")),
            }
        }
        other => Err(format!("Unknown builtin handler `{other}`")),
    }
}
