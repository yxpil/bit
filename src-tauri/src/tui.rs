// 简约 TUI：`bit tui` 进入终端模式——无前端窗口、不监听 HTTP 端口、不抢单实例，
// 与桌面端共用同一套数据目录与 Agent 链路（对话 / 工具 / 会话 / 记忆全部可用）。
// 行协议 REPL：stdin 逐行读取，stdout 逐行输出，便于交互也便于 E2E 管道测试。
use crate::state::Ctx;
use std::io::Write;
use std::sync::Arc;
use tauri::Manager;

const HELP: &str = "\
命令：
  /help           显示本帮助
  /sessions       列出会话
  /new [标题]      新建会话并切换
  /use <id>       切换会话（id 可只写前几位）
  /tools          列出工具
  /mem <内容>      沉淀一条记忆
  /mems           查看记忆
  /install-cli    把 bit 命令安装到终端 PATH
  /quit           退出
其他任意输入即为对话消息；工具调用过程逐行展示。";

enum Flow {
    Continue,
    Exit,
}

/// 由 main.rs 在独立线程调用：stdin 读取放专用线程（不阻塞 tokio worker），
/// 行通过 channel 交给异步循环处理（chat_turn 是 async）。不返回（进程内退出）。
pub fn run_blocking(ctx: Arc<Ctx>, app: tauri::AppHandle) -> ! {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut buf = String::new();
        loop {
            buf.clear();
            match std::io::BufRead::read_line(&mut stdin.lock(), &mut buf) {
                Ok(0) | Err(_) => break, // EOF（管道关闭）与读错误都视为退出
                Ok(_) => {
                    if tx.send(buf.trim_end().to_string()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let tui_app = app.clone();
    let inner_ctx = ctx.clone();
    tauri::async_runtime::block_on(async move {
        let ctx = inner_ctx;
        println!("BIT TUI v{}", tui_app.package_info().version);
        println!("/help 查看命令 · /quit 退出");
        if !ctx.ai_config.lock().unwrap().is_configured() {
            println!("[提示] AI 尚未配置：请先在桌面端「AI 设置」配置提供方，对话功能暂不可用。");
        }
        loop {
            print!("bit> ");
            let _ = std::io::stdout().flush();
            let Some(line) = rx.recv().await else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match handle(&ctx, line).await {
                Ok(Flow::Continue) => {}
                Ok(Flow::Exit) => break,
                Err(e) => println!("错误：{e}"),
            }
        }
        println!("再见。");
    });

    crate::audit::record(&ctx, "local-cli", "app.quit", "tui", serde_json::json!({}), true);
    // CLI 直接退出：不依赖 tauri 事件循环收尾（Linux 无窗口场景 app.exit 不可靠）
    std::process::exit(0)
}

async fn handle(ctx: &Arc<Ctx>, line: &str) -> Result<Flow, String> {
    // 斜杠命令（大小写不敏感）
    if let Some(cmd) = line.strip_prefix('/') {
        let (cmd, arg) = match cmd.split_once(' ') {
            Some((c, a)) => (c.trim(), a.trim()),
            None => (cmd.trim(), ""),
        };
        return match cmd.to_lowercase().as_str() {
            "help" | "?" => {
                println!("{HELP}");
                Ok(Flow::Continue)
            }
            "sessions" => {
                let store = ctx.sessions.lock().unwrap();
                for s in &store.sessions {
                    let mark = if s.id == store.active { "*" } else { " " };
                    println!("{mark} {}  [{:>2} 条]  {}", &s.id[..s.id.len().min(8)], s.messages.len(), s.title);
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
                println!("已创建会话 {}（已切换）", &id[..8]);
                Ok(Flow::Continue)
            }
            "use" => {
                let arg = arg.to_string();
                if arg.is_empty() {
                    return Err("用法：/use <会话id>".into());
                }
                {
                    let mut store = ctx.sessions.lock().unwrap();
                    let hit = store
                        .sessions
                        .iter()
                        .find(|s| s.id.starts_with(&arg))
                        .map(|s| s.id.clone())
                        .ok_or("会话不存在")?;
                    store.active = hit.clone();
                    println!("已切换到 {}（{}）", &hit[..8], {
                        let s = store.sessions.iter().find(|s| s.id == hit);
                        s.map(|s| s.title.as_str()).unwrap_or("")
                    });
                }
                ctx.save_sessions();
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
                    println!("{:<3} {:<16} {:<8} {}", if t.enabled { "on" } else { "off" }, t.name, kind, t.description);
                }
                Ok(Flow::Continue)
            }
            "mem" => {
                if arg.is_empty() {
                    return Err("用法：/mem <内容>".into());
                }
                let m = crate::memory::add_memory(ctx, arg, "raw", "user");
                crate::audit::record(ctx, "local-cli", "memory.add", "memories", serde_json::json!({}), true);
                println!("已沉淀记忆 {}", &m.id[..8]);
                Ok(Flow::Continue)
            }
            "mems" => {
                let mems = ctx.memories.lock().unwrap();
                if mems.is_empty() {
                    println!("（暂无记忆）");
                }
                for m in mems.iter().rev() {
                    println!("{}  {}  {}", &m.id[..m.id.len().min(8)], m.ts, m.content);
                }
                Ok(Flow::Continue)
            }
            "install-cli" => {
                let r = crate::commands::install_cli_impl(ctx)?;
                println!("已安装 bit 命令：{}", r["path"].as_str().unwrap_or(""));
                if let Some(hint) = r["hint"].as_str() {
                    if !hint.is_empty() {
                        println!("提示：{hint}");
                    }
                }
                Ok(Flow::Continue)
            }
            "quit" | "exit" | "q" => Ok(Flow::Exit),
            other => Err(format!("未知命令 /{other}，/help 查看帮助")),
        };
    }

    // 普通对话：走完整 Agent 链路（含工具调用循环），结束后逐行回放本轮过程
    let sid = ctx.sessions.lock().unwrap().active.clone();
    let before = ctx
        .sessions
        .lock()
        .unwrap()
        .sessions
        .iter()
        .find(|s| s.id == sid)
        .map(|s| s.messages.len())
        .unwrap_or(0);
    let messages = crate::agent::chat_turn(ctx, "", line, Vec::new()).await?;
    let sess = ctx.sessions.lock().unwrap();
    if let Some(s) = sess.sessions.iter().find(|s| s.id == sid) {
        for m in s.messages.iter().skip(before) {
            match m.role.as_str() {
                "assistant" => {
                    if !m.content.trim().is_empty() {
                        println!("{}", m.content.trim());
                    }
                    for tc in &m.tool_calls {
                        println!(
                            "[tool] {} {} → {}",
                            tc.tool,
                            tc.params,
                            if tc.ok { "成功" } else { "失败" }
                        );
                    }
                }
                "tool" => {} // 过程已由 tool_calls 行呈现，不重复打印
                _ => {}
            }
        }
    }
    let _ = messages;
    Ok(Flow::Continue)
}
