// yxpil · BIT
// 全屏 TUI（Ratatui）：顶部状态栏 + 滚动消息区 + 圆角输入框。
// 键盘事件由专用 OS 线程读取（crossterm event::read 阻塞），跨线程送 tokio 循环；
// Agent 回合在独立 task 跑，输出走 Out::Chan 回流，打字/中断不被 await 卡住。
use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use ratatui::Terminal;

use crate::state::Ctx;
use crate::tui::handle;
use crate::tui::{Flow, Out, HELP};

/// 键盘事件线程 → UI 循环
type KeyTx = tokio::sync::mpsc::UnboundedSender<crossterm::event::KeyEvent>;
/// Agent 回合结束通知
type DoneTx = tokio::sync::mpsc::UnboundedSender<Result<Flow, String>>;

/// RAII 终端恢复：任何退出路径（含 unwinding panic）都还原用户终端
struct TerminalGuard {
    _t: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let t = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { _t: t })
    }
    fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self._t
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

struct App {
    lines: Vec<String>,
    input: String,
    /// 距底部的滚动行数：0 = 始终贴底；PageUp 翻历史时挂住
    scroll_back: usize,
    busy: bool,
}

impl App {
    fn new() -> Self {
        Self { lines: vec![HELP.to_string()], input: String::new(), scroll_back: 0, busy: false }
    }
    fn push(&mut self, s: String) {
        const MAX_LINES: usize = 4000;
        self.lines.push(s);
        if self.lines.len() > MAX_LINES {
            let drop_n = self.lines.len() - MAX_LINES;
            self.lines.drain(0..drop_n);
        }
        // 用户没翻历史时保持贴底
    }
}

/// 不返回（进程内退出）
pub fn run(ctx: Arc<Ctx>, app: tauri::AppHandle) -> ! {
    let mut term = match TerminalGuard::enter() {
        Ok(t) => t,
        // raw mode 进不去（某些奇葩终端）：回退行协议，保证可用
        Err(_) => return super::plain::run(ctx, app),
    };

    let (key_tx, mut key_rx) = tokio::sync::mpsc::unbounded_channel::<crossterm::event::KeyEvent>();
    spawn_key_thread(key_tx);

    let result = tauri::async_runtime::block_on(async_main(ctx.clone(), app.clone(), &mut term, &mut key_rx));

    // drop guard 还原终端后再打印收尾/退出
    drop(term);
    if let Err(e) = result {
        println!("BIT TUI 异常退出：{e}");
    }
    crate::audit::record(&ctx, "local-cli", "app.quit", "tui", serde_json::json!({ "ui": "ratatui" }), true);
    crate::restore_console_cp();
    std::process::exit(0)
}

async fn async_main(
    ctx: Arc<Ctx>,
    app: tauri::AppHandle,
    term: &mut TerminalGuard,
    key_rx: &mut tokio::sync::mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
) -> Result<(), String> {
    let mut ui = App::new();
    ui.push(format!("BIT TUI v{}（全屏界面）", app.package_info().version));
    if !ctx.ai_config.lock().unwrap().is_configured() {
        ui.push("[提示] AI 尚未配置：请先在桌面端「AI 设置」配置提供方，对话功能暂不可用。".into());
    }

    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<Result<Flow, String>>();

    draw(term.terminal(), &ctx, &ui).ok();

    loop {
        tokio::select! {
            // Agent/命令输出行
            Some(line) = out_rx.recv() => {
                ui.push(line);
                draw(term.terminal(), &ctx, &ui).ok();
            }
            // 回合结束
            Some(res) = done_rx.recv() => {
                ui.busy = false;
                match res {
                    Ok(Flow::Exit) => {
                        ui.push("再见。".into());
                        draw(term.terminal(), &ctx, &ui).ok();
                        tokio::time::sleep(Duration::from_millis(120)).await;
                        return Ok(());
                    }
                    Ok(Flow::Continue) => {}
                    Err(e) => ui.push(format!("错误：{e}")),
                }
                draw(term.terminal(), &ctx, &ui).ok();
            }
            // 键盘
            maybe_key = key_rx.recv() => {
                let Some(key) = maybe_key else { return Ok(()); };
                // Windows 下 release/repeat 都会来，只吃 press 避免重复触发
                if key.kind != crossterm::event::KeyEventKind::Press && key.kind != crossterm::event::KeyEventKind::Repeat {
                    continue;
                }
                let mut want_quit = false;
                match key.code {
                    KeyCode::Enter if !ui.busy => {
                        let line = ui.input.trim().to_string();
                        ui.input.clear();
                        ui.scroll_back = 0;
                        if !line.is_empty() {
                            ui.push(format!("bit> {line}"));
                            spawn_turn(ctx.clone(), line, out_tx.clone(), done_tx.clone());
                            ui.busy = true;
                        }
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => want_quit = true,
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) && ui.input.is_empty() => want_quit = true,
                    KeyCode::Esc => {
                        // Esc = 中断当前回合（不退出；退出请 /quit 或 Ctrl+C）
                        let active = ctx.sessions.lock().unwrap().active.clone();
                        if crate::agent::request_stop(&ctx, &active) {
                            ui.push("[interrupt] 已请求中断当前回合…".into());
                        }
                    }
                    KeyCode::Backspace => {
                        ui.input.pop();
                    }
                    KeyCode::PageUp => ui.scroll_back = ui.scroll_back.saturating_add(10),
                    KeyCode::PageDown => ui.scroll_back = ui.scroll_back.saturating_sub(10),
                    KeyCode::End => ui.scroll_back = 0,
                    KeyCode::Char(c) => ui.input.push(c),
                    _ => {}
                }
                if want_quit {
                    return Ok(());
                }
                draw(term.terminal(), &ctx, &ui).ok();
            }
        }
    }
}

/// 跑一条命令/对话：输出行与结束通知分别走两个 channel
fn spawn_turn(
    ctx: Arc<Ctx>,
    line: String,
    out_tx: tokio::sync::mpsc::UnboundedSender<String>,
    done_tx: DoneTx,
) {
    tauri::async_runtime::spawn(async move {
        let out = Out::Chan(out_tx);
        let res = handle(&ctx, &line, &out).await;
        let _ = done_tx.send(res);
    });
}

/// 键盘读取线程：event::read 阻塞直到有键，跨线程送出后继续
fn spawn_key_thread(tx: KeyTx) {
    std::thread::spawn(move || loop {
        match crossterm::event::read() {
            Ok(Event::Key(k)) => {
                if tx.send(k).is_err() {
                    break;
                }
            }
            Ok(_) => {} // 鼠标等事件忽略（EnableMouseCapture 已开，留给以后）
            Err(_) => break,
        }
    });
}

fn draw(t: &mut Terminal<CrosstermBackend<Stdout>>, ctx: &Arc<Ctx>, ui: &App) -> io::Result<()> {
    t.draw(|f| {
        let area = f.area();
        let chunks = Layout::vertical([
            Constraint::Length(1), // 状态栏
            Constraint::Min(3),   // 消息区
            Constraint::Length(3), // 输入框（边框 2 行 + 留白）
        ])
        .split(area);

        // ── 状态栏：左信息 / 右状态 ──
        let (title, approval, ws) = {
            let store = ctx.sessions.lock().unwrap();
            let title = store.sessions.iter().find(|s| s.id == store.active).map(|s| s.title.clone()).unwrap_or_default();
            let approval = ctx.config.lock().unwrap().tool_approval.clone();
            let ws = crate::sandbox::effective_root(ctx)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "-".into());
            (title, approval, ws)
        };
        let status_cols = Layout::horizontal([Constraint::Percentage(72), Constraint::Percentage(28)]).split(chunks[0]);
        let left = Line::from(vec![Span::styled(
            format!(" BIT · {title} · 审批:{approval} · 工作区:{ws} "),
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
        )]);
        let right_text = if ui.busy { " ● 思考中… " } else { " ○ 就绪 " };
        let right = Line::from(Span::styled(
            right_text,
            Style::default()
                .fg(if ui.busy { Color::Black } else { Color::Black })
                .bg(if ui.busy { Color::Yellow } else { Color::Green })
                .add_modifier(Modifier::BOLD),
        ));
        f.render_widget(Paragraph::new(left), status_cols[0]);
        f.render_widget(Paragraph::new(right).right_aligned(), status_cols[1]);

        // ── 消息区：自动换行 + 滚动（scroll_back 距底行数）──
        let text: Vec<Line> = ui.lines.iter().map(|s| Line::from(s.as_str())).collect();
        let body = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((ui.scroll_back as u16, 0))
            .block(Block::default());
        f.render_widget(body, chunks[1]);

        // ── 输入框：圆角，标题带快捷键提示，busy 时变色 ──
        let border_color = if ui.busy { Color::Yellow } else { Color::Cyan };
        let input = Paragraph::new(ui.input.as_str()).block(
            Block::default()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(border_color))
                .title(Span::styled(
                    " 输入（Enter 发送 · Esc 中断 · PgUp/PgDn 历史 · /quit 退出） ",
                    Style::default().fg(Color::DarkGray),
                )),
        );
        f.render_widget(input, chunks[2]);
    })?;
    Ok(())
}
