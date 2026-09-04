// release 构建隐藏 Windows 控制台窗口；debug 保留便于查看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod ai;
mod audit;
mod autopilot;
mod commands;
mod config;
mod extract;
mod goal;
mod http_api;
mod mcp;
mod memory;
mod registry;
mod runtime;
mod script;
mod script_runtime;
mod session;
mod state;
mod tray;
mod tui;
mod update;

use tauri::Manager;

fn main() {
    // 终端模式：`bit tui` 或交互式终端里的裸 `bit` 进入简约 TUI（无窗口 / 无单实例 / 不监听端口，
    // 可与桌面端同时运行，共用数据目录）。generate_context! 只能展开一次，
    // 所以 TUI 与桌面端共用同一个 Builder，仅按模式注册不同的插件与启动逻辑。
    // Windows 关键顺序：release 版是 GUI 子系统（无控制台），从终端启动时标准流全部无效，
    // 必须先 attach_console 挂接父进程控制台（CONIN$→stdin），stdin 的 TTY 检测才有意义；
    // 双击 / open 等无父控制台的启动方式 AttachConsole 自然失败，仍走桌面端。
    // stdout 已被管道占用（E2E / CI）时 attach_console 自动跳过，标准流保持原样。
    #[cfg(windows)]
    attach_console();
    let explicit_tui = std::env::args().any(|a| a == "tui");
    let bare_tty_tui = !explicit_tui
        && std::env::args().count() == 1
        && std::env::var_os("BIT_HEADLESS").is_none()
        && std::io::IsTerminal::is_terminal(&std::io::stdin());
    let tui_mode = explicit_tui || bare_tty_tui;

    // WebView2 默认遵循系统代理，而安装版前端经 http://tauri.localhost 加载；
    // 系统代理（如 Clash）未排除该主机时会白屏。前端资源全部本地内嵌，禁用代理无副作用。
    // 追加而非覆盖，保留外部传入的调试参数（如 --remote-debugging-port）。
    if !tui_mode {
        let mut webview_args = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
        if !webview_args.is_empty() && !webview_args.contains("no-proxy-server") {
            webview_args.push(' ');
        }
        webview_args.push_str("--no-proxy-server");
        std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", webview_args);
    }

    let mut builder = tauri::Builder::default();
    if !tui_mode {
        // 单实例保护：仅桌面端注册（TUI 需要能与桌面端同时运行）；
        // 二次启动时唤起已有实例的主窗口后退出新进程
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                tray::show_main_window(app);
            }))
            .plugin(tauri_plugin_notification::init());
    }
    builder
        .setup(move |app| {
            let ctx = state::Ctx::load(app.handle().clone());
            let (actor, target) = if tui_mode { ("local-cli", "tui") } else { ("local-app", "BIT") };
            audit::record(&ctx, actor, "app.start", target, serde_json::json!({}), true);
            app.manage(ctx.clone());

            if tui_mode {
                // TUI：无窗口、无托盘、无 HTTP 服务、无 Autopilot（与桌面端零冲突）。
                // 解释器探测同步执行：CLI 场景不赶时间，脚本类工具需要完整列表。
                let _ = ctx.refresh_runtimes();
                let tui_ctx = ctx.clone();
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    // 内部 std::process::exit，不会返回
                    tui::run_blocking(tui_ctx, handle);
                });
                return Ok(());
            }

            // 解释器探测移到后台：不阻塞窗口显示（修复启动慢/白屏）
            let rt_ctx = ctx.clone();
            let rt_app = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                if rt_ctx.refresh_runtimes() {
                    use tauri::Emitter;
                    let _ = rt_app.emit("runtimes-updated", ());
                }
            });

            // 系统托盘（关闭窗口后程序驻留后台）
            tray::create(app.handle(), &ctx)?;

            // 远程访问 HTTP 服务
            let http_ctx = ctx.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = http_api::restart_server(&http_ctx).await {
                    eprintln!("[BIT] http server error: {e}");
                }
            });

            // Autopilot：记忆/技能自动总结循环（小圆片播放/暂停）
            let auto_ctx = ctx.clone();
            tauri::async_runtime::spawn(async move {
                autopilot::run(auto_ctx).await;
            });

            // 自动更新：启动后静默检测 + 下载（下载完成发 update-state 事件）
            let upd_ctx = ctx.clone();
            let upd_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                update::auto_update_task(upd_app, upd_ctx).await;
            });

            Ok(())
        })
        // 关闭窗口 = 最小化到托盘（后台继续运行 HTTP 服务与 Autopilot）
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::is_headless,
            commands::check_updates,
            commands::update_download,
            commands::update_apply,
            commands::open_external,
            commands::install_cli,
            commands::mem_usage,
            commands::get_overview,
            commands::list_tools,
            commands::register_tool,
            commands::register_script_tool,
            commands::remove_tool,
            commands::set_tool_enabled,
            commands::invoke_tool,
            commands::list_runtimes,
            commands::refresh_runtimes,
            commands::add_runtime,
            commands::remove_runtime,
            commands::set_runtime_enabled,
            commands::run_script,
            commands::list_audit,
            commands::get_remote_config,
            commands::save_remote_config,
            commands::regenerate_client_key,
            commands::save_access_password,
            commands::regenerate_access_password,
            commands::test_connectivity,
            commands::list_providers,
            commands::add_provider,
            commands::update_provider,
            commands::remove_provider,
            commands::set_provider_active,
            commands::chat,
            commands::chat_stream,
            commands::extract_file,
            commands::fetch_webpage,
            commands::check_port,
            commands::compress_session,
            commands::mcp_discover,
            commands::mcp_connect,
            commands::mcp_list,
            commands::mcp_toggle,
            commands::mcp_remove,
            commands::mcp_import,
            commands::chat_interrupt,
            commands::tool_approve,
            commands::set_tool_approval,
            commands::get_tool_approval,
            commands::get_ai_params,
            commands::set_ai_params,
            commands::list_provider_models,
            commands::context_preview,
            commands::context_metrics,
            commands::list_sessions,
            commands::get_session,
            commands::create_session,
            commands::set_active_session,
            commands::rename_session,
            commands::delete_session,
            commands::clear_session,
            commands::list_memories,
            commands::add_memory,
            commands::delete_memories,
            commands::list_skills,
            commands::add_skill,
            commands::delete_skills,
            commands::toggle_autopilot,
            commands::run_autopilot_now,
            commands::list_goals,
            commands::create_goal,
            commands::update_goal_status,
            commands::remove_goal,
            commands::list_todos,
            commands::add_todo,
            commands::update_todo_status,
            commands::remove_todo,
            commands::open_path,
            commands::quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building BIT")
        .run(|app, event| {
            // macOS：点击 Dock 图标时若主窗口隐藏则重新显示
            // （Windows 任务栏点击自带唤起，macOS 需要 Reopen 事件处理）
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                tray::show_main_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

/// Windows release 版是 GUI 子系统（无控制台），`bit tui` 从终端启动时
/// 需先挂接父进程控制台并重新打开标准流，否则输出会静默丢失。
#[cfg(windows)]
fn attach_console() {
    extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
        fn SetStdHandle(n_std_handle: u32, handle: isize) -> i32;
        fn GetStdHandle(n_std_handle: u32) -> isize;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_INPUT_HANDLE: u32 = (-10i32) as u32;
    const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;
    const STD_ERROR_HANDLE: u32 = (-12i32) as u32;
    unsafe {
        // stdout 已有有效句柄（父进程管道重定向，如 E2E / CI）→ 绝不能覆盖，
        // 否则输出会改道 CONOUT$ 导致管道收不到任何内容
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out != 0 && out != -1 {
            return;
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        // File 对象 Drop 会 CloseHandle：SetStdHandle 登记后若放任作用域结束，
        // 标准流句柄立即失效（句柄值还可能被后续 CreateFile 复用），TUI 秒退且输出全丢。
        // 故意 mem::forget 泄漏，让句柄存活到进程结束。
        use std::os::windows::io::AsRawHandle;
        if let Ok(f) = std::fs::OpenOptions::new().read(true).open("CONIN$") {
            SetStdHandle(STD_INPUT_HANDLE, f.as_raw_handle() as _);
            std::mem::forget(f);
        }
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            SetStdHandle(STD_OUTPUT_HANDLE, f.as_raw_handle() as _);
            SetStdHandle(STD_ERROR_HANDLE, f.as_raw_handle() as _);
            std::mem::forget(f);
        }
    }
}
