// yxpil · BIT
// 行协议 REPL：stdin 逐行、stdout 逐行。管道 / 无 TTY / E2E / --plain 走这里。
use std::io::Write;
use std::sync::Arc;

use crate::state::Ctx;
use crate::tui::{Flow, Out, handle};

/// 不返回（进程内退出）
pub fn run(ctx: Arc<Ctx>, app: tauri::AppHandle) -> ! {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // stdin 线程持有 ctx 副本：/interrupt 必须在回合 await 期间也能即时置位，
    // 不能走 mpsc 队列（队列里的命令要等当前回合结束才被处理）
    let stdin_ctx = ctx.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut buf = String::new();
        loop {
            buf.clear();
            match std::io::BufRead::read_line(&mut stdin.lock(), &mut buf) {
                Ok(0) | Err(_) => break, // EOF（管道关闭）与读错误都视为退出
                Ok(_) => {
                    let line = buf.trim_end().trim();
                    // 中断行就地处理：对当前激活会话置标志，回执直接打到 stdout
                    if line.eq_ignore_ascii_case("/interrupt") {
                        let active = stdin_ctx.sessions.lock().unwrap().active.clone();
                        if crate::agent::request_stop(&stdin_ctx, &active) {
                            println!("[interrupt] 已请求中断当前回合…");
                        } else {
                            println!("[interrupt] 当前会话没有进行中的回合");
                        }
                        let _ = std::io::stdout().flush();
                        continue;
                    }
                    if tx.send(line.to_string()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let inner_ctx = ctx.clone();
    tauri::async_runtime::block_on(async move {
        let ctx = inner_ctx;
        println!("BIT TUI v{}", app.package_info().version);
        println!("/help 查看命令 · /quit 退出");
        if !ctx.ai_config.lock().unwrap().is_configured() {
            println!("[提示] AI 尚未配置：请先在桌面端「AI 设置」配置提供方，对话功能暂不可用。");
        }
        let out = Out::Stdout;
        loop {
            print!("bit> ");
            let _ = std::io::stdout().flush();
            let Some(line) = rx.recv().await else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match handle(&ctx, line, &out).await {
                Ok(Flow::Continue) => {}
                Ok(Flow::Exit) => break,
                Err(e) => println!("错误：{e}"),
            }
        }
        println!("再见。");
    });

    crate::audit::record(&ctx, "local-cli", "app.quit", "tui", serde_json::json!({}), true);
    // Windows：还原 attach_console 改过的控制台代码页（UTF-8 → 原值），不污染用户终端
    crate::restore_console_cp();
    // CLI 直接退出：不依赖 tauri 事件循环收尾（Linux 无窗口场景 app.exit 不可靠）
    std::process::exit(0)
}
