// yxpil · BIT
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
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
            "Run a shell command; returns stdout/stderr + exit code",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command to execute" },
                    "cwd": { "type": "string", "description": "Working directory (optional)" },
                    "background": { "type": "boolean", "description": "Known long tasks: run in background; job_id returns at once, result auto-reports when done" }
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
        // 3.5 更新计划状态（目标状态 + 待办状态）——唯一的更新入口
        mk(
            "builtin.plan_update",
            "plan_update",
            "Update a plan: change goal status and/or todo statuses. This is the ONLY tool for updating plan/todo states.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "goal_id": { "type": "string", "description": "Goal id (returned by plan tool)" },
                    "goal_status": { "type": "string", "description": "Goal status: active | achieved | abandoned (optional)" },
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "Todo id" },
                                "status": { "type": "string", "description": "Todo status: pending | in_progress | completed" }
                            },
                            "required": ["id", "status"]
                        },
                        "description": "Todo status updates (optional, batch all you need in one call)"
                    }
                },
                "required": ["goal_id"]
            }),
            "plan_update",
        ),
        // 3.6 写入/重写待办清单（整体替换某目标下的待办；等价于 TodoWrite）
        mk(
            "builtin.todo_write",
            "todo_write",
            "Write or rewrite the todo list for a goal (or standalone). Replaces the entire todo list under the given goal_id. Pass items as either strings or {content, status} objects.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "goal_id": { "type": "string", "description": "Goal id returned by the plan tool (optional; omit for a standalone list)" },
                    "items": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                { "type": "string", "description": "Todo content (status defaults to pending)" },
                                { "type": "object", "properties": {
                                    "content": { "type": "string" },
                                    "status": { "type": "string", "description": "pending | in_progress | completed" }
                                }, "required": ["content"] }
                            ]
                        },
                        "description": "Todo items: strings or {content, status} objects"
                    }
                }
            }),
            "todo_write",
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
        // 5.5 子智能体：派生独立会话执行子任务（模型可自主调用；子代理会话内被宿主拦截，禁止嵌套）
        mk(
            "builtin.sub_agent",
            "sub_agent",
            "Spawn an independent sub-session agent (all tools) for a long self-contained subtask; its final answer returns to you. Task text must be self-contained. No nesting",
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
            "View a local image (png/jpg/webp/gif/bmp): injected into the next turn as visual content",
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
        // 7.6-7.8 screen / mouse / keyboard：本机操控三件套（实验性功能）。
        // 背景：最初为 macOS 实现（screencapture / osascript / CoreGraphics / System Events）。
        // 现状与决策：
        //   1) macOS 整体不注册不启用（TCC 授权体验差，用户决定 macOS 屏蔽）；
        //   2) Windows/Linux 构建仍保留入口作为「实验性」预留，默认关闭（config 闸门三者恒默认
        //      false，设置里可尝试开启）。⚠️ 平台后端尚未落地：实际调用会返回明确的
        //      “实验性功能、平台实现未就绪”错误，不会执行任何本机操控、也不会误报成功。
        //   3) 历史遗留的注册项会在启动加载时按出厂清单自动移除。
        #[cfg(not(target_os = "macos"))]
        mk(
            "builtin.screen",
            "screen",
            "(Experimental) Capture the screen or a region to a PNG for view_image. Platform backend not shipped yet for Windows/Linux — keep OFF in Settings; calling it returns an explicit error.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number", "description": "Region left (optional; all four fields required to capture a region)" },
                    "y": { "type": "number", "description": "Region top (optional)" },
                    "width": { "type": "number", "description": "Region width (optional)" },
                    "height": { "type": "number", "description": "Region height (optional)" }
                }
            }),
            "screen",
        ),
        #[cfg(not(target_os = "macos"))]
        mk(
            "builtin.mouse",
            "mouse",
            "(Experimental) Read/control the mouse. Platform backend not shipped yet for Windows/Linux — keep OFF in Settings; calling it returns an explicit error. Actions: position / move / click / double_click / right_click (x,y) / drag (x,y→x2,y2) / scroll (dx,dy)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["position", "move", "click", "double_click", "right_click", "drag", "scroll"], "description": "What to do" },
                    "x": { "type": "number", "description": "Cursor X in screen coordinates (origin top-left)" },
                    "y": { "type": "number", "description": "Cursor Y in screen coordinates" },
                    "x2": { "type": "number", "description": "Drag target X (drag only)" },
                    "y2": { "type": "number", "description": "Drag target Y (drag only)" },
                    "dx": { "type": "number", "description": "Horizontal scroll pixels (scroll only)" },
                    "dy": { "type": "number", "description": "Vertical scroll pixels (scroll only)" }
                },
                "required": ["action"]
            }),
            "mouse",
        ),
        // 7.8 keyboard：键盘输入（实验性；同上——Windows/Linux 后端未就绪，默认关闭）
        #[cfg(not(target_os = "macos"))]
        mk(
            "builtin.keyboard",
            "keyboard",
            "(Experimental) Type text or press keys. Platform backend not shipped yet for Windows/Linux — keep OFF in Settings; calling it returns an explicit error. Actions: type (text), key (named key or single char, optional modifiers)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["type", "key"], "description": "What to do" },
                    "text": { "type": "string", "description": "Text to type (type only)" },
                    "key": { "type": "string", "description": "Named key (return/tab/space/delete/escape/home/end/pageup/pagedown/left/right/up/down/f1-f12) or a single character (key only)" },
                    "cmd": { "type": "boolean", "description": "Hold ⌘ (key only)" },
                    "shift": { "type": "boolean", "description": "Hold ⇧ (key only)" },
                    "option": { "type": "boolean", "description": "Hold ⌥ (key only)" },
                    "ctrl": { "type": "boolean", "description": "Hold ⌃ (key only)" }
                },
                "required": ["action"]
            }),
            "keyboard",
        ),
    ]
}

/// 把 screen / mouse / keyboard 三个操控工具的 enabled 同步为设置页闸门状态。
/// 启动时与闸门变更时调用：闸门关 = 工具页显示关闭 + 不下发模型 + 调用拒绝，三层一致。
/// 仅 Windows/Linux 有这三个工具（macOS 已整体移除，无需也不应触碰 tools 列表）
#[cfg(not(target_os = "macos"))]
pub fn sync_gate_enabled(ctx: &Arc<crate::state::Ctx>) {
    // 锁序纪律 + 单次加锁：config 快照克隆后立即释放，tools 只在一次锁内完成「改 + 序列化」，
    // 不在持锁期间写盘、也不对同一 Mutex 二次加锁（std Mutex 不可重入，二次 lock 即死锁）
    let cfg = ctx.config.lock().unwrap().clone();
    let json = {
        let mut tools = ctx.tools.lock().unwrap();
        for t in tools.iter_mut() {
            match t.name.as_str() {
                "screen" | "mouse" | "keyboard" => t.enabled = cfg.tool_gate(&t.name),
                _ => {}
            }
        }
        serde_json::to_string(&*tools).unwrap_or_default()
    };
    // 落盘失败静默（下次启动会再同步），不让存储问题拖垮启动
    let _ = std::fs::write(ctx.data_dir.join("tools.json"), json);
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
    // 参数 schema 必须可读：空 properties 会让模型猜参数名（曾导致 csv_stats 收不到 column 只回元信息）。
    // 例外：Interpreter / Script（add_tool 自建、run_script 沉淀）走「stdin 全 JSON」协议，参数天然自由，
    // 由模型按 description + additionalProperties 传参——空 schema 是它们的正常形态，不能一刀切拦截
    // （否则 AI 自建工具全部注册失败，E2E add-tool / overwrite / delete-tool 链路回归）。
    let freeform = matches!(&kind, ToolKind::Interpreter { .. } | ToolKind::Script { .. });
    if !freeform && parameters.get("type").and_then(|v| v.as_str()) == Some("object") {
        let empty = parameters
            .get("properties")
            .and_then(|v| v.as_object())
            .map(|p| p.is_empty())
            .unwrap_or(true);
        if empty {
            return Err(
                "parameters.properties cannot be empty: declare every parameter with name, type and description — the model relies on this schema to pass arguments correctly".into(),
            );
        }
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

/// 在途子代理计数（进程级）：「现在有几个子代理在跑」。
/// 宿主调度据此限流（delegation 按 config.subagent_max 判断是否还能再派）。
static SUBAGENT_DEPTH: AtomicUsize = AtomicUsize::new(0);

/// 当前在跑的子代理数量
pub fn subagent_depth() -> usize {
    SUBAGENT_DEPTH.load(Ordering::SeqCst)
}

/// 在跑的子代理会话 id 集合：标记「哪些会话本身是子代理」。
/// sub_agent 工具已下发给模型（AI 自主决定派生），靠它显式禁止子代理再派生子代理（防递归）。
static SUB_SESSIONS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn sub_sessions() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    SUB_SESSIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// 该会话是否本身是一个子代理会话
pub fn is_sub_session(sid: &str) -> bool {
    sub_sessions().lock().unwrap().contains(sid)
}

fn register_sub_session(sid: &str) {
    sub_sessions().lock().unwrap().insert(sid.to_string());
}

fn unregister_sub_session(sid: &str) {
    sub_sessions().lock().unwrap().remove(sid);
}

/// 子代理生命周期事件（宿主调度模型的对外通知面）。
/// 模型侧可自主调用 sub_agent 派生（也受宿主开关/上限约束）；
/// 无论宿主入口还是模型调用，都会依次广播 spawn → start → done|error。
/// phase: spawn（会话已建） / start（开始跑） / done（拿到结论） / error（失败或超时/中断）
fn emit_subagent(
    ctx: &Arc<crate::state::Ctx>,
    phase: &str,
    sid: &str,
    parent: Option<&str>,
    title: &str,
    depth: usize,
    extra: Option<serde_json::Value>,
) {
    use tauri::Emitter;
    let mut payload = serde_json::json!({
        "phase": phase,
        "session_id": sid,
        "parent": parent,
        "title": title,
        "depth": depth,
        "at": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    });
    if let (Some(obj), Some(ext)) =
        (payload.as_object_mut(), extra.as_ref().and_then(|v| v.as_object()))
    {
        for (k, v) in ext {
            obj.insert(k.clone(), v.clone());
        }
    }
    let _ = ctx.app.emit("subagent-lifecycle", payload);
}

/// 停止一个在跑的子代理：对子会话的中断标志置位，其 agent 回合在下一个检查点停止。
/// 子会话保留中途进度（与主会话中断行为一致），stop 只影响该子代理自身。
/// 返回 false 表示该会话当前没有在跑回合（可能已结束/从未开始）。
pub fn stop_subagent(ctx: &Arc<crate::state::Ctx>, sid: &str) -> bool {
    use std::sync::atomic::Ordering;
    let map = ctx.interrupts.lock().unwrap();
    match map.get(sid) {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
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

    // 工具质量评估：计时并记录成功/失败/耗时/失败原因（所有调用路径统一收口）
    let t0 = std::time::Instant::now();
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

    crate::state::toolstats::record(
        ctx,
        &tool.id,
        result.is_ok(),
        t0.elapsed().as_millis() as u64,
        result.as_ref().err().map(|e| safe_trunc(e, 200)).as_deref(),
    );
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

/// 辅助功能权限检测（macOS）：未授权时 CGEventPost 会静默丢弃（不报错但事件不生效），
/// 必须显式检测并给用户明确指引，否则模型会以为操作成功。
/// 进程内 FFI 检测（归属 BIT 本体），不再借道 osascript（TCC 会误判成 osascript 的授权状态）
/// 仅 Windows/Linux 的操控工具在调用前探测（macOS 上这些工具已不注册）
#[cfg(not(target_os = "macos"))]
fn ax_trusted() -> bool {
    crate::perms::ax_trusted()
}

#[cfg(not(target_os = "macos"))]
const AX_DENIED_MSG: &str =
    "辅助功能权限未授予：打开 系统设置 → 隐私与安全性 → 辅助功能，勾选 BIT 后重试（授权后无需重启）";

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
            // AI 显式标记长任务：跳过前台窗口直接转后台（对话继续，完成后自动唤回本会话 AI）
            let force_bg = params.get("background").and_then(|v| v.as_bool()).unwrap_or(false);
            // 后台 shell：短命令秒回；长命令自动转后台（shell-job 事件 + 可停止 + 完成时顶层 worker 自动唤回会话 AI）
            crate::shellbg::run(ctx, &command, cwd.as_deref(), session, force_bg).await
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
            let raw = params.get("path").and_then(|v| v.as_str()).ok_or("Missing parameter: path")?;
            // 规范化 + 绝对化：卡片存自包含的绝对路径，点击打开/定位时不再受当时上下文影响
            let path = crate::commands::normalize_user_path(raw);
            let p = std::path::Path::new(&path);
            let meta = std::fs::metadata(p).map_err(|_| format!("File not found: {raw}"))?;
            if meta.is_dir() {
                return Err(format!("`{path}` is a directory; send_file only accepts a single file"));
            }
            // 卡片存自包含的干净绝对路径：canonicalize 绝对化后剥掉 Windows `\\?\` verbatim
            // 前缀（否则 explorer /select 解析失败退回打开"文档"文件夹）
            let card_path = crate::commands::clean_display_path(
                &std::fs::canonicalize(p).unwrap_or_else(|_| std::path::PathBuf::from(&path)),
            );
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from)
                .unwrap_or_else(|| path.to_string());
            let note = params.get("note").and_then(|v| v.as_str()).unwrap_or("");
            Ok(serde_json::json!({
                "sent": true,
                "path": card_path,
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
        // ── 2.7 screen：截屏（macOS，screencapture），返回路径供 view_image 查看 ──
        #[cfg(not(target_os = "macos"))]
        "screen" => {
            if !cfg!(target_os = "macos") {
                return Err("screen：本机操控为实验性功能，Windows/Linux 实现尚未提供（设置中默认关闭，请保持关闭）".into());
            }
            if !ctx.config.lock().unwrap().tool_gate("screen") {
                return Err("screen 能力未开启：请在 设置 → AI 行为设置 → 工具权限 打开并完成屏幕录制授权".into());
            }
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S%3f");
            let path = format!("/tmp/bit-screen-{ts}.png");
            let mut cmd = tokio::process::Command::new("screencapture");
            cmd.arg("-x").arg("-t").arg("png");
            let (x, y, w, h) = (
                params.get("x").and_then(|v| v.as_f64()),
                params.get("y").and_then(|v| v.as_f64()),
                params.get("width").and_then(|v| v.as_f64()),
                params.get("height").and_then(|v| v.as_f64()),
            );
            if let (Some(x), Some(y), Some(w), Some(h)) = (x, y, w, h) {
                cmd.arg(format!("-R{x},{y},{w},{h}"));
            }
            cmd.arg(&path);
            let out = cmd
                .output()
                .await
                .map_err(|e| format!("Failed to run screencapture: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "screencapture failed: {} (首次使用需在 系统设置 → 隐私与安全性 → 屏幕录制 中授权 BIT)",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Ok(serde_json::json!({
                "path": path,
                "note": "Screenshot saved. Now call view_image with this path to see the screen.",
            }))
        }
        // ── 2.8 mouse：查看/操作鼠标（macOS，osascript JXA + CoreGraphics）──
        #[cfg(not(target_os = "macos"))]
        "mouse" => {
            if !cfg!(target_os = "macos") {
                return Err("mouse：本机操控为实验性功能，Windows/Linux 实现尚未提供（设置中默认关闭，请保持关闭）".into());
            }
            if !ctx.config.lock().unwrap().tool_gate("mouse") {
                return Err("mouse 能力未开启：请在 设置 → AI 行为设置 → 工具权限 打开并完成辅助功能授权".into());
            }
            let action = params
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or("Missing parameter: action")?;
            // 未授权时 CGEventPost 静默丢弃——先检测，给出可操作的指引
            if action != "position" && !ax_trusted() {
                return Err(AX_DENIED_MSG.into());
            }
            let xy = |k1: &str, k2: &str| -> Result<(f64, f64), String> {
                let x = params
                    .get(k1)
                    .and_then(|v| v.as_f64())
                    .ok_or(format!("Missing parameter: {k1}"))?;
                let y = params
                    .get(k2)
                    .and_then(|v| v.as_f64())
                    .ok_or(format!("Missing parameter: {k2}"))?;
                Ok((x, y))
            };
            let script = match action {
                "position" => {
                    r#"ObjC.import('CoreGraphics'); const p=$.CGEventGetLocation($.CGEventCreate($())); JSON.stringify({x:p.x, y:p.y})"#.to_string()
                }
                "move" => {
                    let (x, y) = xy("x", "y")?;
                    format!(
                        r#"ObjC.import('CoreGraphics'); $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), $.kCGEventMouseMoved, $.CGPointMake({x},{y}), $.kCGMouseButtonLeft)); 'moved'"#
                    )
                }
                "click" | "double_click" | "right_click" => {
                    let (x, y) = xy("x", "y")?;
                    let pairs = if action == "double_click" { 2 } else { 1 };
                    let (down, up, btn) = if action == "right_click" {
                        (
                            "$.kCGEventRightMouseDown",
                            "$.kCGEventRightMouseUp",
                            "$.kCGMouseButtonRight",
                        )
                    } else {
                        (
                            "$.kCGEventLeftMouseDown",
                            "$.kCGEventLeftMouseUp",
                            "$.kCGMouseButtonLeft",
                        )
                    };
                    let mut s = format!(
                        r#"ObjC.import('CoreGraphics'); const p=$.CGPointMake({x},{y}); $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), $.kCGEventMouseMoved, p, {btn}));"#
                    );
                    for _ in 0..pairs {
                        s.push_str(&format!(
                            r#" $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), {down}, p, {btn})); $.NSThread.sleepForTimeInterval(0.03); $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), {up}, p, {btn})); $.NSThread.sleepForTimeInterval(0.06);"#
                        ));
                    }
                    s.push_str(" 'clicked'");
                    s
                }
                "drag" => {
                    let (x, y) = xy("x", "y")?;
                    let (tx, ty) = xy("x2", "y2")?;
                    format!(
                        r#"ObjC.import('CoreGraphics');
                        function mv(px,py){{ $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), $.kCGEventLeftMouseDragged, $.CGPointMake(px,py), $.kCGMouseButtonLeft)); }}
                        $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), $.kCGEventMouseMoved, $.CGPointMake({x},{y}), $.kCGMouseButtonLeft));
                        $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), $.kCGEventLeftMouseDown, $.CGPointMake({x},{y}), $.kCGMouseButtonLeft));
                        for (let i=1;i<=12;i++){{ mv({x}+({tx}-{x})*i/12, {y}+({ty}-{y})*i/12); $.NSThread.sleepForTimeInterval(0.02); }}
                        $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent($(), $.kCGEventLeftMouseUp, $.CGPointMake({tx},{ty}), $.kCGMouseButtonLeft));
                        'dragged'"#
                    )
                }
                "scroll" => {
                    let dx = params.get("dx").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
                    let dy = params.get("dy").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
                    if dx == 0 && dy == 0 {
                        return Err("scroll needs a non-zero dx or dy".into());
                    }
                    format!(
                        r#"ObjC.import('CoreGraphics'); const e=$.CGEventCreateScrollWheelEvent($(), $.kCGScrollEventUnitPixel, 1, {dy}); if ({dx} !== 0) {{ $.CGEventSetIntegerValueField(e, $.kCGScrollWheelEventDeltaAxis2, {dx}); }} $.CGEventPost($.kCGHIDEventTap, e); 'scrolled'"#
                    )
                }
                other => {
                    return Err(format!(
                        "Unknown action '{other}'; available: position, move, click, double_click, right_click, drag, scroll"
                    ))
                }
            };
            let out = tokio::process::Command::new("osascript")
                .args(["-l", "JavaScript", "-e", &script])
                .output()
                .await
                .map_err(|e| format!("Failed to run osascript: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "mouse {action} failed: {} (首次使用需在 系统设置 → 隐私与安全性 → 辅助功能 中授权 BIT)",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if action == "position" {
                // position 返回 {"x":..,"y":..} JSON；解析失败则原样返回
                return Ok(match serde_json::from_str::<serde_json::Value>(&stdout) {
                    Ok(v) => v,
                    Err(_) => serde_json::json!({ "raw": stdout }),
                });
            }
            Ok(serde_json::json!({ "ok": true, "action": action, "result": stdout }))
        }
        // ── 2.9 keyboard：键盘输入（macOS，System Events keystroke / key code）──
        #[cfg(not(target_os = "macos"))]
        "keyboard" => {
            if !cfg!(target_os = "macos") {
                return Err("keyboard：本机操控为实验性功能，Windows/Linux 实现尚未提供（设置中默认关闭，请保持关闭）".into());
            }
            if !ctx.config.lock().unwrap().tool_gate("keyboard") {
                return Err(
                    "keyboard 能力未开启：请在 设置 → AI 行为设置 → 工具权限 打开并完成辅助功能授权".into(),
                );
            }
            if !ax_trusted() {
                return Err(AX_DENIED_MSG.into());
            }
            let action = params
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or("Missing parameter: action")?;
            let mods = {
                let mut m: Vec<&str> = Vec::new();
                if params.get("cmd").and_then(|v| v.as_bool()).unwrap_or(false) {
                    m.push("command down");
                }
                if params.get("shift").and_then(|v| v.as_bool()).unwrap_or(false) {
                    m.push("shift down");
                }
                if params.get("option").and_then(|v| v.as_bool()).unwrap_or(false) {
                    m.push("option down");
                }
                if params.get("ctrl").and_then(|v| v.as_bool()).unwrap_or(false) {
                    m.push("control down");
                }
                if m.is_empty() { String::new() } else { format!(" using {{{}}}", m.join(", ")) }
            };
            let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            let script = match action {
                "type" => {
                    let text = params
                        .get("text")
                        .and_then(|v| v.as_str())
                        .ok_or("Missing parameter: text")?;
                    if text.is_empty() {
                        return Err("text cannot be empty".into());
                    }
                    format!(r#"tell application "System Events" to keystroke "{}""#, esc(text))
                }
                "key" => {
                    let key = params
                        .get("key")
                        .and_then(|v| v.as_str())
                        .ok_or("Missing parameter: key")?;
                    let named = match key.to_lowercase().as_str() {
                        "return" | "enter" => Some(36),
                        "tab" => Some(48),
                        "space" => Some(49),
                        "delete" | "backspace" => Some(51),
                        "forwarddelete" => Some(117),
                        "escape" | "esc" => Some(53),
                        "home" => Some(115),
                        "end" => Some(119),
                        "pageup" => Some(116),
                        "pagedown" => Some(121),
                        "left" => Some(123),
                        "right" => Some(124),
                        "down" => Some(125),
                        "up" => Some(126),
                        "f1" => Some(122),
                        "f2" => Some(120),
                        "f3" => Some(99),
                        "f4" => Some(118),
                        "f5" => Some(96),
                        "f6" => Some(97),
                        "f7" => Some(98),
                        "f8" => Some(100),
                        "f9" => Some(101),
                        "f10" => Some(109),
                        "f11" => Some(103),
                        "f12" => Some(111),
                        _ => None,
                    };
                    match named {
                        Some(code) => {
                            format!(r#"tell application "System Events" to key code {code}{mods}"#)
                        }
                        None if key.chars().count() == 1 => {
                            format!(r#"tell application "System Events" to keystroke "{}"{mods}"#, esc(key))
                        }
                        None => {
                            return Err(format!(
                                "Unknown key '{key}'; use a single character or a named key (return/tab/space/delete/escape/home/end/pageup/pagedown/left/right/up/down/f1-f12)"
                            ))
                        }
                    }
                }
                other => return Err(format!("Unknown action '{other}'; available: type, key")),
            };
            let out = tokio::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .output()
                .await
                .map_err(|e| format!("Failed to run osascript: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "keyboard {action} failed: {} (需辅助功能权限：系统设置 → 隐私与安全性 → 辅助功能)",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Ok(serde_json::json!({ "ok": true, "action": action }))
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
        // ── 3.5 更新计划状态（唯一更新入口）──
        "plan_update" => {
            let goal_id = params.get("goal_id").and_then(|v| v.as_str()).ok_or("Missing parameter: goal_id")?.to_string();
            // 目标状态更新（可选）
            let goal_status = params.get("goal_status").and_then(|v| v.as_str()).map(|s| s.to_string());
            let mut goal_result = None;
            if let Some(status) = goal_status.as_deref() {
                let g = crate::goal::update_goal_status(ctx, &goal_id, status)?;
                goal_result = Some(serde_json::json!({ "id": g.id, "title": g.title, "status": g.status }));
            }
            // 待办状态批量更新（可选）
            let todos: Vec<serde_json::Value> = params
                .get("todos")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let mut todo_results = Vec::with_capacity(todos.len());
            for t in &todos {
                let tid = t.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                let tstatus = t.get("status").and_then(|v| v.as_str()).unwrap_or_default();
                if tid.is_empty() || tstatus.is_empty() {
                    continue;
                }
                match crate::goal::update_todo_status(ctx, tid, tstatus) {
                    Ok(updated) => {
                        todo_results.push(serde_json::json!({ "id": updated.id, "content": updated.content, "status": updated.status }));
                    }
                    Err(e) => {
                        todo_results.push(serde_json::json!({ "id": tid, "error": e }));
                    }
                }
            }
            Ok(serde_json::json!({
                "goal": goal_result,
                "todos_updated": todo_results.len(),
                "todos": todo_results,
            }))
        }
        // ── 3.6 写入/重写待办清单 ──
        "todo_write" => {
            let goal_id = params.get("goal_id").and_then(|v| v.as_str()).map(|s| s.to_string());
            let items: Vec<serde_json::Value> = params
                .get("items")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let written = crate::goal::rewrite_todos(ctx, goal_id, &items, actor, session)?;
            Ok(serde_json::json!({ "written": written }))
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
            struct DecGuard;
            impl Drop for DecGuard {
                fn drop(&mut self) {
                    SUBAGENT_DEPTH.fetch_sub(1, Ordering::SeqCst);
                }
            }
            // sub_agent 已下发给模型（AI 自主决定派生），递归防护从「模型看不到工具」
            // 改为显式拦截：子代理会话再调用 sub_agent 直接拒绝。
            if session.map(is_sub_session).unwrap_or(false) {
                return Err(
                    "Sub-agents cannot spawn further sub-agents — finish the work in this session"
                        .into(),
                );
            }
            // 并行上限与宿主委派共用 config.subagent_max（1..=8，设置页可调）
            let (running, limit) = {
                let cfg = ctx.config.lock().unwrap();
                (
                    subagent_depth(),
                    cfg.subagent_max.clamp(1, crate::delegation::MAX_CFG_SUBAGENTS as u32) as usize,
                )
            };
            if running >= limit {
                return Err(format!(
                    "已有 {running} 个子代理在跑（并行上限 {limit}，可在设置里调整），等有位置再派生"
                ));
            }
            // 防御性硬顶：正常由上面的并行上限拦住，这里防计数失控自增
            const SANITY_MAX: usize = 32;
            let depth = SUBAGENT_DEPTH.fetch_add(1, Ordering::SeqCst);
            let _guard = DecGuard;
            if depth >= SANITY_MAX {
                return Err("Too many sub-agents running (safety cap hit)".into());
            }
            let task = params.get("task").and_then(|v| v.as_str()).ok_or("Missing parameter: task")?.to_string();
            if task.trim().is_empty() {
                return Err("task cannot be empty".into());
            }
            let title = params.get("title").and_then(|v| v.as_str()).unwrap_or("子任务").to_string();
            let parent = session.map(|s| s.to_string());
            let started = std::time::Instant::now();
            // 新建独立会话（用户可在侧栏看到全过程）
            let sess = crate::session::Session::new(&title);
            let sid = sess.id.clone();
            ctx.sessions.lock().unwrap().sessions.push(sess);
            crate::session::persist(ctx);
            // 登记「该会话是子代理」：子代理回合内再调 sub_agent 会被上面的检查拒绝
            register_sub_session(&sid);
            // spawn/start 都携带任务预览（task 截断），供委派面板「看它派了什么出去」
            let task_preview = safe_trunc(&task, 240);
            let spawn_extra = Some(serde_json::json!({ "task": task_preview }));
            emit_subagent(ctx, "spawn", &sid, parent.as_deref(), &title, depth, spawn_extra.clone());
            {
                use tauri::Emitter;
                let _ = ctx.app.emit("sessions-updated", &sid);
            }
            emit_subagent(ctx, "start", &sid, parent.as_deref(), &title, depth, spawn_extra);
            // 阻塞执行子任务：完整复用 agent 循环（工具、审批、自动续发全部生效）。
            // Box::pin：builtin_invoke → chat_turn → execute_tool_call → builtin_invoke 递归，需手动打断无限大小
            let mut run = Box::pin(crate::agent::chat_turn(ctx, &sid, &task, Vec::new()));
            const SUB_TIMEOUT_SECS: u64 = 15 * 60;
            let sleep = tokio::time::sleep(std::time::Duration::from_secs(SUB_TIMEOUT_SECS));
            // 主会话点「停止」时立刻取消子任务，而不是干等子任务跑完
            let watch = async {
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
                        // 主会话中断：取消子任务（run future 被 drop 即终止）。
                        // 子会话残留的中断条目无需在此清理：下一回合 register 时会复位，
                        // 且 chat_turn 的摘除按 Arc 指针比对，外部无法拿到子回合的标志
                        break Err(format!("Parent session interrupted; subtask stopped (sub-session {sid} keeps the progress)"));
                    }
                }
            };
            // 生命周期收尾事件：宿主/前端据此知道子代理跑完还是失败（含耗时与结论长度）
            let elapsed_ms = started.elapsed().as_millis();
            let extra = match &outcome {
                Ok(v) => serde_json::json!({
                    "ms": elapsed_ms,
                    "answer_chars": v
                        .get("final_answer")
                        .and_then(|s| s.as_str())
                        .map(|s| s.chars().count())
                        .unwrap_or(0),
                    "truncated": v.get("truncated").and_then(|b| b.as_bool()).unwrap_or(false),
                }),
                Err(e) => serde_json::json!({ "ms": elapsed_ms, "error": e }),
            };
            emit_subagent(
                ctx,
                if outcome.is_ok() { "done" } else { "error" },
                &sid,
                parent.as_deref(),
                &title,
                depth,
                Some(extra),
            );
            {
                use tauri::Emitter;
                let _ = ctx.app.emit("sessions-updated", &sid);
            }
            unregister_sub_session(&sid);
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
