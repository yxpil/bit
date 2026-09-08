// yxpil · BIT
// release 构建隐藏 Windows 控制台窗口；debug 保留便于查看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod ai;
mod audit;
mod autopilot;
mod commands;
mod config;
mod crash;
mod extract;
mod goal;
mod guardian;
mod http_api;
mod mcp;
mod memory;
mod netinfo;
mod registry;
mod relay;
mod repetition;
mod runtime;
mod script;
mod script_runtime;
mod security;
mod session;
mod state;
mod tray;
mod tui;
mod update;

use std::sync::Arc;
use tauri::Manager;
use tauri::webview::Color;

/// 权威退出函数：所有退出路径（托盘、quit_app 命令、信号、兜底）都走这里。
/// 与守护进程握手 + 静默更新 + 最终 exit。不阻塞、不 panic。
fn graceful_quit(ctx: &tauri::AppHandle, via: &str) {
    if let Some(c) = ctx.try_state::<Arc<crate::state::Ctx>>() {
        crate::audit::record(&c, "local-app", "app.quit", "BIT",
            serde_json::json!({ "via": via }), true);
        crate::guardian::expect_exit(&c);
        let _ = crate::update::apply_update(&c, false);
    }
    ctx.exit(0);
}

fn main() {
    // ============ 跨平台渲染 workaround（必须在任何 GTK/WebKit/WebView2 初始化之前） ============

    // Linux (WebKitGTK)：NVIDIA GPU 在 Wayland 上的 DMABUF 渲染器崩溃
    // （Error 71 / AcceleratedSurfaceDMABuf / 白色空白窗口）。
    // 参考 Tauri 官方 https://tauri.app/develop/debug/linux-graphics/
    // 智能检测：只在受影响条件下 + 用户没手动设置过时自动覆盖，
    // 避免误伤稳定系统和高级用户的自定义配置。
    #[cfg(target_os = "linux")]
    {
        let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
        let is_x11 = std::env::var("DISPLAY").is_ok() && !is_wayland;

        if is_wayland {
            // Wayland + NVIDIA：优先只禁用 explicit sync，保留 DMABUF 硬件加速
            if std::env::var("__NV_DISABLE_EXPLICIT_SYNC").is_err() {
                std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
            }
            // 兜底：禁用整个 DMABUF 渲染器（影响性能但最稳）
            if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
                std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
            }
        }

        // X11 + 透明窗口：部分旧 GTK/WebKitGTK 版本需要 GDK_BACKEND=x11
        // 才能让 transparent:true 正常工作（避免黑色/白色方块背景）。
        if is_x11 && std::env::var("GDK_BACKEND").is_err() {
            std::env::set_var("GDK_BACKEND", "x11");
        }
    }

    // Windows：WebView2 透明窗口多层保险。
    // wry 0.55+ 在窗口构建时已通过 COM API（ICoreWebView2ControllerOptions3 /
    // ICoreWebView2Controller2）设 DefaultBackgroundColor 为全透明，Tauri 只要读到
    // transparent:true 就会触发。但某些 WebView2 Runtime 版本或 wry 分支可能不走
    // 这条路径，这里额外设置两个 WebView2 环境变量兜底：
    //   - WEBVIEW2_DEFAULT_BACKGROUND_COLOR：微软官方指定的早期背景色环境变量
    //   - WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS += --default-background-color：Chromium 命令行
    // 都必须在 Builder 创建前设置（wry 构建窗口时读取）。
    #[cfg(target_os = "windows")]
    {
        // 微软官方文档推荐：这个环境变量比 COM API 还早生效，能彻底消除启动白闪
        if std::env::var("WEBVIEW2_DEFAULT_BACKGROUND_COLOR").is_err() {
            std::env::set_var("WEBVIEW2_DEFAULT_BACKGROUND_COLOR", "00000000");
        }
        // Chromium 命令行参数兜底
        let mut args = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
        let need_bg = !args.contains("--default-background-color");
        if need_bg {
            if !args.is_empty() {
                args.push(' ');
            }
            args.push_str("--default-background-color=00000000");
            std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", args);
        }
    }

    // ============================================================================================

    // 终端模式：`bit tui` 或交互式终端里的裸 `bit` 进入简约 TUI（无窗口 / 无单实例 / 不监听端口，
    // 可与桌面端同时运行，共用数据目录）。generate_context! 只能展开一次，
    // 所以 TUI 与桌面端共用同一个 Builder，仅按模式注册不同的插件与启动逻辑。
    // Windows 关键顺序：release 版是 GUI 子系统（无控制台），从终端启动时标准流全部无效，
    // 必须先 attach_console 挂接父进程控制台（CONIN$→stdin），stdin 的 TTY 检测才有意义；
    // 双击 / open 等无父控制台的启动方式 AttachConsole 自然失败，仍走桌面端。
    // stdout 已被管道占用（E2E / CI）时 attach_console 自动跳过，标准流保持原样。
    #[cfg(windows)]
    attach_console();

    // 守护进程模式：本进程由主进程拉起用于看门狗守护，不进入 GUI / TUI（握手文件与日志路径由参数传入）
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() >= 4 && argv[1] == guardian::GUARDIAN_FLAG {
        guardian::run_guardian(argv[2].clone().into(), argv[3].clone().into());
        return;
    }

    // --data-dir <path>：显式指定数据目录（提权重启时授权弹窗产生的子进程拿不到原环境变量，
    // 用参数透传保证数据目录一致；也便于脚本/测试）
    if let Some(pos) = argv.iter().position(|a| a == "--data-dir") {
        if let Some(dir) = argv.get(pos + 1) {
            std::env::set_var("BIT_DATA_DIR", dir);
        }
    }

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

    let mut builder = tauri::Builder::default()
        // 以下插件 TUI 与桌面端共用：autostart 开机自启、notification 系统通知
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ));
    if !tui_mode {
        // 单实例保护：仅桌面端注册（TUI 需要能与桌面端同时运行）；
        // 二次启动时唤起已有实例的主窗口后退出新进程
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }));
    }
    builder
        .setup(move |app| {
            let ctx = state::Ctx::load(app.handle().clone());
            // 全局 panic 钩子：崩溃信息（含回溯）追加到数据目录 crash.log，诊断报告展示
            crash::install(&ctx.data_dir);
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

            // ========== 透明窗口强制设色 ==========
            // Rust 端直接调 set_background_color 比 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS 更可靠——
            // WebView2 的 --default-background-color 参数在某些 wry/WebView2 版本组合下不生效
            // （见 tauri-apps/tauri#1739 / khiops/termora#98）。macOS 上 transparent: true 也只对
            // 窗口级生效，WKWebView 自身默认白底，同样需要显式设透明。
            if let Some(win) = app.get_webview_window("main") {
                if let Err(e) = win.set_background_color(Some(Color(0, 0, 0, 0))) {
                    eprintln!("[BIT] set_background_color failed: {e}");
                }
            }

            // ========== 桌面端：信号兜底 ==========
            // Ctrl+C 兜底（托盘退出、quit_app、ExitRequested 都已经走 graceful_quit，
            // 这里只兜底系统级 kill/终端 Ctrl+C 意外退出场景）
            let handle_sig = app.handle().clone();
            let _ = ctrlc::set_handler(move || {
                graceful_quit(&handle_sig, "signal");
            });

            // 守护进程布防：接力日志转存审计（此前发生的被杀/拉起/篡改拒绝事件）→ 写握手文件 → 拉起守护进程
            guardian::drain_log(&ctx);
            guardian::arm(&ctx);
            tauri::async_runtime::spawn(guardian::watchdog_task(ctx.clone()));

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

            // 开机自启：以配置为准同步系统登录项（配置是唯一真源，修复登录项被系统/用户清理后的漂移）
            {
                use tauri_plugin_autostart::ManagerExt;
                let autostart_wanted = ctx.config.lock().unwrap().autostart;
                let manager = app.autolaunch();
                let cur = manager.is_enabled().unwrap_or(false);
                if cur != autostart_wanted {
                    let r = if autostart_wanted { manager.enable() } else { manager.disable() };
                    if let Err(e) = r {
                        eprintln!("[BIT] autostart sync failed: {e}");
                    }
                }
            }

            // 远程访问 HTTP 服务
            let http_ctx = ctx.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = http_api::restart_server(&http_ctx).await {
                    eprintln!("[BIT] http server error: {e}");
                }
            });

            // 后台拉取激活提供方的模型列表：尽量获取各模型最大上下文（写入 model_context 缓存，失败静默）
            let mf_ctx = ctx.clone();
            tauri::async_runtime::spawn(async move {
                let p = mf_ctx.ai_config.lock().unwrap().active().cloned();
                if let Some(p) = p {
                    commands::refresh_model_context(&mf_ctx, &p.protocol, &p.base_url, &p.api_key).await;
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
            commands::ui_mounted,
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
            commands::clear_audit,
            commands::delete_audit_entry,
            commands::get_remote_config,
            commands::get_remote_status,
            commands::save_remote_config,
            commands::regenerate_client_key,
            commands::get_behavior_settings,
            commands::set_behavior_settings,
            commands::get_guard_limits,
            commands::set_guard_limits,
            commands::get_custom_prompt,
            commands::set_custom_prompt,
            commands::get_system_prompt,
            commands::set_system_prompt,
            commands::save_cloud_relay,
            commands::save_stun_servers,
            commands::get_lan_info,
            commands::get_remote_qr,
            commands::qr_svg_url,
            commands::save_access_password,
            commands::regenerate_access_password,
            commands::test_connectivity,
            commands::list_providers,
            commands::add_provider,
            commands::update_provider,
            commands::remove_provider,
            commands::set_provider_active,
            commands::set_provider_text_fallback,
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
            commands::get_autostart,
            commands::set_autostart,
            commands::get_elevation,
            commands::set_elevation,
            commands::get_tool_stats,
            commands::get_diagnostics,
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

            // Cmd+Q（macOS）/ 系统退出请求：走真正退出链路，
            // 与托盘退出 / quit_app command 一致——通知守护进程 + 静默更新
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(ctx) = app.try_state::<Arc<crate::state::Ctx>>() {
                    crate::audit::record(&ctx, "local-app", "app.quit", "BIT",
                        serde_json::json!({ "via": "exit_requested" }), true);
                    crate::guardian::expect_exit(&ctx);
                    let _ = crate::update::apply_update(&ctx, false);
                }
                app.exit(0);
            }
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
        fn GetConsoleOutputCP() -> u32;
        fn SetConsoleOutputCP(w_code_page_id: u32) -> i32;
        fn GetConsoleCP() -> u32;
        fn SetConsoleCP(w_code_page_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_INPUT_HANDLE: u32 = (-10i32) as u32;
    const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;
    const STD_ERROR_HANDLE: u32 = (-12i32) as u32;
    const CP_UTF8: u32 = 65001;
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
        // Rust 按 UTF-8 直写标准流：中文 Windows 控制台默认 GBK(936) 会把 TUI 中文打成乱码，
        // 读入同理。切到 UTF-8 并记录原值，进程退出前 restore_console_cp() 还原，不污染用户终端。
        let po = GetConsoleOutputCP();
        if po != CP_UTF8 {
            SetConsoleOutputCP(CP_UTF8);
            PREV_OUTPUT_CP.store(po, std::sync::atomic::Ordering::Relaxed);
        }
        let pi = GetConsoleCP();
        if pi != CP_UTF8 {
            SetConsoleCP(CP_UTF8);
            PREV_INPUT_CP.store(pi, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// attach_console 切代码页前的原值（0 = 未改动，无需还原）
#[cfg(windows)]
static PREV_OUTPUT_CP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(windows)]
static PREV_INPUT_CP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// TUI 退出前还原控制台代码页（attach 时改过才还原）
#[cfg(windows)]
pub fn restore_console_cp() {
    extern "system" {
        fn SetConsoleOutputCP(w_code_page_id: u32) -> i32;
        fn SetConsoleCP(w_code_page_id: u32) -> i32;
    }
    let po = PREV_OUTPUT_CP.swap(0, std::sync::atomic::Ordering::Relaxed);
    if po != 0 {
        unsafe {
            SetConsoleOutputCP(po);
        }
    }
    let pi = PREV_INPUT_CP.swap(0, std::sync::atomic::Ordering::Relaxed);
    if pi != 0 {
        unsafe {
            SetConsoleCP(pi);
        }
    }
}

#[cfg(not(windows))]
pub fn restore_console_cp() {}
