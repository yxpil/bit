// yxpil · BIT
// TUI 入口与共享命令处理。
// 两种界面：
// - plain：行协议 REPL（stdin/stdout 逐行）——管道、无 TTY、E2E、--plain 强制
// - full：Ratatui 全屏界面（真终端）——状态栏 + 滚动消息区 + 输入框
// 命令处理（handle）两者共用，输出统一走 Out（plain 直打 stdout，full 送 UI channel）
#[cfg(feature = "tui-ui")]
mod full;
mod plain;

use std::sync::Arc;

use crate::state::Ctx;

pub(crate) const HELP: &str = "\
命令：
  /help            显示本帮助
  /sessions        列出会话
  /new [标题]       新建会话并切换
  /use <id>        切换会话（id 可只写前几位）
  /rename [id] <标题>  重命名当前（或指定）会话
  /delete [id]     删除当前（或指定）会话
  /clear           清空当前会话的消息（保留会话）
  /goals           列出目标与状态
  /todo            列出待办
  /approval [模式] 查看/切换审批：ask | auto | allow_all
  /interrupt       中断当前进行中的回合（执行中也可随时输入）
  /tools           列出工具
  /runtimes        列出本地解释器运行时
  /mem <内容>       沉淀一条记忆
  /mems            查看记忆
  /pwd             显示工作区（沙箱根）
  /cd <目录>        切换工作区（Agent 的 shell/文件操作锚定于此）
  /install-cli     把 bit 命令安装到终端 PATH
  /quit            退出
其他任意输入即为对话消息；工具调用过程逐行展示。";

pub(crate) enum Flow {
    Continue,
    Exit,
}

/// 命令/对话输出目标：两种界面各取一种，handle 内不直接 println
#[derive(Clone)]
pub(crate) enum Out {
    /// 行协议：直接打到 stdout（同步，提示符顺序天然正确）
    Stdout,
    /// 全屏 UI：送消息 channel，由渲染循环追加到滚动区
    Chan(tokio::sync::mpsc::UnboundedSender<String>),
}

impl Out {
    pub(crate) fn line(&self, s: impl Into<String>) {
        match self {
            Out::Stdout => println!("{}", s.into()),
            Out::Chan(tx) => {
                let _ = tx.send(s.into());
            }
        }
    }
}

/// 由 main.rs 在独立线程调用：按终端形态分流，不返回（进程内退出）
pub fn run_blocking(ctx: Arc<Ctx>, app: tauri::AppHandle) -> ! {
    use std::io::IsTerminal;
    let force_plain = std::env::args().any(|a| a == "--plain");
    if force_plain || !std::io::stdout().is_terminal() {
        plain::run(ctx, app)
    } else {
        #[cfg(feature = "tui-ui")]
        {
            full::run(ctx, app)
        }
        #[cfg(not(feature = "tui-ui"))]
        {
            plain::run(ctx, app)
        }
    }
}

/// 斜杠命令分发 + 普通对话 Agent 链路。两种界面共用。
pub(crate) async fn handle(ctx: &Arc<Ctx>, line: &str, out: &Out) -> Result<Flow, String> {
    // 斜杠命令（大小写不敏感）
    if let Some(cmd) = line.strip_prefix('/') {
        let (cmd, arg) = match cmd.split_once(' ') {
            Some((c, a)) => (c.trim(), a.trim()),
            None => (cmd.trim(), ""),
        };
        return match cmd.to_lowercase().as_str() {
            "help" | "?" => {
                out.line(HELP);
                Ok(Flow::Continue)
            }
            "sessions" => {
                let store = ctx.sessions.lock().unwrap();
                for s in &store.sessions {
                    let mark = if s.id == store.active { "*" } else { " " };
                    out.line(format!("{mark} {}  [{:>2} 条]  {}", &s.id[..s.id.len().min(8)], s.messages.len(), s.title));
                }
                Ok(Flow::Continue)
            }
            "new" => {
                let id;
                {
                    let mut store = ctx.sessions.lock().unwrap();
                    let s = crate::session::Session::new(arg);
                    id = s.id.clone();
                    store.sessions.push(s);
                    store.active = id.clone();
                }
                ctx.save_sessions();
                out.line(format!("已创建会话 {}（已切换）", &id[..8]));
                Ok(Flow::Continue)
            }
            "use" => {
                let arg = arg.to_string();
                if arg.is_empty() {
                    return Err("用法：/use <会话id>".into());
                }
                let switched = {
                    let mut store = ctx.sessions.lock().unwrap();
                    let hit = store
                        .sessions
                        .iter()
                        .find(|s| s.id.starts_with(&arg))
                        .map(|s| s.id.clone())
                        .ok_or("会话不存在")?;
                    store.active = hit.clone();
                    let title = store.sessions.iter().find(|s| s.id == hit).map(|s| s.title.clone()).unwrap_or_default();
                    (hit, title)
                };
                ctx.save_sessions();
                out.line(format!("已切换到 {}（{}）", &switched.0[..8], switched.1));
                Ok(Flow::Continue)
            }
            "tools" => {
                let tools = ctx.tools.lock().unwrap();
                for t in tools.iter() {
                    let kind = match &t.kind {
                        crate::registry::ToolKind::Builtin { .. } => "builtin",
                        crate::registry::ToolKind::Remote { .. } => "remote",
                        crate::registry::ToolKind::Script { .. } => "script",
                        crate::registry::ToolKind::Interpreter { .. } => "interp",
                        crate::registry::ToolKind::Mcp { .. } => "mcp",
                    };
                    out.line(format!("{:<3} {:<16} {:<8} {}", if t.enabled { "on" } else { "off" }, t.name, kind, t.description));
                }
                Ok(Flow::Continue)
            }
            "mem" => {
                if arg.is_empty() {
                    return Err("用法：/mem <内容>".into());
                }
                let m = crate::memory::add_memory(ctx, arg, "raw", "user");
                crate::audit::record(ctx, "local-cli", "memory.add", "memories", serde_json::json!({}), true);
                out.line(format!("已沉淀记忆 {}", &m.id[..m.id.len().min(8)]));
                Ok(Flow::Continue)
            }
            "mems" => {
                let mems = ctx.memories.lock().unwrap();
                if mems.is_empty() {
                    out.line("（暂无记忆）");
                }
                for m in mems.iter().rev() {
                    out.line(format!("{}  {}  {}", &m.id[..m.id.len().min(8)], m.ts, m.content));
                }
                Ok(Flow::Continue)
            }
            "install-cli" => {
                let r = crate::commands::install_cli_impl(ctx)?;
                out.line(format!("已安装 bit 命令：{}", r["path"].as_str().unwrap_or("")));
                if let Some(hint) = r["hint"].as_str() {
                    if !hint.is_empty() {
                        out.line(format!("提示：{hint}"));
                    }
                }
                Ok(Flow::Continue)
            }
            // ── 会话整理：重命名 / 删除 / 清空 ──
            "rename" => {
                if arg.is_empty() {
                    return Err("用法：/rename [id前缀] <新标题>".into());
                }
                // 第一段若能匹配会话 id 则视为指定会话，否则整串都是当前会话的新标题
                let (id, title) = {
                    let store = ctx.sessions.lock().unwrap();
                    match arg.split_once(' ') {
                        Some((prefix, rest)) if store.sessions.iter().any(|s| s.id.starts_with(prefix)) => {
                            let id = store.sessions.iter().find(|s| s.id.starts_with(prefix)).unwrap().id.clone();
                            (id, rest.trim().to_string())
                        }
                        _ => (store.active.clone(), arg.to_string()),
                    }
                };
                {
                    let mut store = ctx.sessions.lock().unwrap();
                    let s = store.get_mut(&id).ok_or("会话不存在")?;
                    s.title = if title.trim().is_empty() { "未命名".into() } else { title.trim().to_string() };
                }
                ctx.save_sessions();
                out.line(format!("已重命名会话 {} → {}", &id[..id.len().min(8)], title.trim()));
                Ok(Flow::Continue)
            }
            "delete" | "rm" => {
                // 无参删当前会话；带参按 id 前缀删。删空自动补默认会话，删的是激活项则切到最后一条
                let target_id = if arg.is_empty() {
                    ctx.sessions.lock().unwrap().active.clone()
                } else {
                    ctx.sessions
                        .lock()
                        .unwrap()
                        .sessions
                        .iter()
                        .find(|s| s.id.starts_with(arg))
                        .map(|s| s.id.clone())
                        .ok_or("会话不存在")?
                };
                let (active, removed_title) = {
                    let mut store = ctx.sessions.lock().unwrap();
                    let title = store.sessions.iter().find(|s| s.id == target_id).map(|s| s.title.clone()).unwrap_or_default();
                    store.sessions.retain(|s| s.id != target_id);
                    if store.sessions.is_empty() {
                        let s = crate::session::Session::new("新对话");
                        store.active = s.id.clone();
                        store.sessions.push(s);
                    } else if store.active == target_id {
                        store.active = store.sessions.last().map(|s| s.id.clone()).unwrap_or_default();
                    }
                    (store.active.clone(), title)
                };
                ctx.save_sessions();
                out.line(format!("已删除会话 {}（{}）", &target_id[..target_id.len().min(8)], removed_title));
                let now = ctx.sessions.lock().unwrap();
                if let Some(s) = now.sessions.iter().find(|s| s.id == active) {
                    out.line(format!("当前会话：{}（{}）", &s.id[..s.id.len().min(8)], s.title));
                }
                Ok(Flow::Continue)
            }
            "clear" => {
                let id = ctx.sessions.lock().unwrap().active.clone();
                let n = {
                    let mut store = ctx.sessions.lock().unwrap();
                    let s = store.get_mut(&id).ok_or("会话不存在")?;
                    let n = s.messages.len();
                    s.messages.clear();
                    s.touch();
                    n
                };
                ctx.save_sessions();
                out.line(format!("已清空 {n} 条消息（会话保留）"));
                Ok(Flow::Continue)
            }
            // ── 目标 / 待办速览 ──
            "goals" => {
                let goals = ctx.goals.lock().unwrap();
                if goals.is_empty() {
                    out.line("（暂无目标）");
                }
                for g in goals.iter().rev() {
                    out.line(format!("{:<8} [{}] {}", &g.id[..g.id.len().min(8)], g.status, g.title));
                }
                Ok(Flow::Continue)
            }
            "todo" | "todos" => {
                let todos = ctx.todos.lock().unwrap();
                if todos.is_empty() {
                    out.line("（暂无待办）");
                }
                let goals = ctx.goals.lock().unwrap();
                for t in todos.iter().rev() {
                    let g = t
                        .goal_id
                        .as_deref()
                        .and_then(|gid| goals.iter().find(|g| g.id == gid))
                        .map(|g| g.title.as_str())
                        .unwrap_or("独立待办");
                    out.line(format!("[{}] {:<8} {} · {}", t.status, &t.id[..t.id.len().min(8)], t.content, g));
                }
                Ok(Flow::Continue)
            }
            // ── 审批模式 ──
            "approval" => {
                if arg.is_empty() {
                    out.line(format!(
                        "当前审批模式：{}（ask=每次询问 / auto=危险操作询问 / allow_all=全放行）",
                        ctx.config.lock().unwrap().tool_approval
                    ));
                    return Ok(Flow::Continue);
                }
                match arg {
                    "ask" | "auto" | "allow_all" => {
                        ctx.config.lock().unwrap().tool_approval = arg.to_string();
                        ctx.save_config();
                        out.line(format!("审批模式已切换为：{arg}"));
                    }
                    _ => return Err("模式只支持 ask / auto / allow_all".into()),
                }
                Ok(Flow::Continue)
            }
            // ── 运行时速览 ──
            "runtimes" | "rt" => {
                let runtimes = ctx.runtimes.lock().unwrap();
                if runtimes.is_empty() {
                    out.line("（未探测到解释器，可在桌面端工具页刷新）");
                }
                for r in runtimes.iter() {
                    out.line(format!("{:<3} {:<8} {:<10} {}", if r.enabled { "on" } else { "off" }, r.lang, r.id, r.name));
                }
                Ok(Flow::Continue)
            }
            // ── 工作区沙箱 ──
            "pwd" => {
                match crate::sandbox::effective_root(ctx) {
                    Some(ws) => out.line(ws.display().to_string()),
                    None => out.line("（未设置工作区：shell 继承进程目录，文件路径不限）"),
                }
                Ok(Flow::Continue)
            }
            "cd" => {
                if arg.is_empty() {
                    return match crate::sandbox::effective_root(ctx) {
                        Some(ws) => {
                            out.line(ws.display().to_string());
                            Ok(Flow::Continue)
                        }
                        None => Err("用法：/cd <绝对目录>（当前无工作区）".into()),
                    };
                }
                let p = std::path::PathBuf::from(arg);
                if !p.is_absolute() {
                    return Err("请给绝对目录（工作区必须是绝对路径）".into());
                }
                if !p.is_dir() {
                    return Err(format!("目录不存在或不是目录：{}", p.display()));
                }
                *ctx.workspace_root.lock().unwrap() = Some(p.clone());
                out.line(format!("工作区已切换：{}", p.display()));
                Ok(Flow::Continue)
            }
            // ── 中断（全屏下 Esc 即时中断；文本命令作为兜底；plain 的 stdin 线程也会就地拦截）──
            "interrupt" => {
                let active = ctx.sessions.lock().unwrap().active.clone();
                if crate::agent::request_stop(ctx, &active) {
                    out.line("[interrupt] 已请求中断当前回合…".to_string());
                } else {
                    out.line("[interrupt] 当前会话没有进行中的回合".to_string());
                }
                Ok(Flow::Continue)
            }
            "quit" | "exit" | "q" => Ok(Flow::Exit),
            other => Err(format!("未知命令 /{other}，/help 查看帮助")),
        };
    }

    // 普通对话：走完整 Agent 链路（含工具调用循环），结束后逐行回放本轮过程
    let sid = ctx.sessions.lock().unwrap().active.clone();
    let before = {
        let store = ctx.sessions.lock().unwrap();
        store.sessions.iter().find(|s| s.id == sid).map(|s| s.messages.len()).unwrap_or(0)
    };
    let messages = crate::agent::chat_turn_auto(ctx, "", line, Vec::new()).await?;
    let sess = ctx.sessions.lock().unwrap();
    if let Some(s) = sess.sessions.iter().find(|s| s.id == sid) {
        for m in s.messages.iter().skip(before) {
            if m.role == "assistant" {
                if !m.content.trim().is_empty() {
                    out.line(m.content.trim().to_string());
                }
                for tc in &m.tool_calls {
                    out.line(format!("[tool] {} {} → {}", tc.tool, tc.params, if tc.ok { "成功" } else { "失败" }));
                }
            }
        }
    }
    let _ = messages;
    Ok(Flow::Continue)
}
