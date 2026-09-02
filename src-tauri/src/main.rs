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

use tauri::Manager;

fn main() {
    // WebView2 默认遵循系统代理，而安装版前端经 http://tauri.localhost 加载；
    // 系统代理（如 Clash）未排除该主机时会白屏。前端资源全部本地内嵌，禁用代理无副作用。
    // 追加而非覆盖，保留外部传入的调试参数（如 --remote-debugging-port）。
    let mut webview_args = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
    if !webview_args.is_empty() && !webview_args.contains("no-proxy-server") {
        webview_args.push(' ');
    }
    webview_args.push_str("--no-proxy-server");
    std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", webview_args);

    tauri::Builder::default()
        // 单实例保护：必须最先注册；二次启动时唤起已有实例的主窗口后退出新进程
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let ctx = state::Ctx::load(app.handle().clone());
            audit::record(&ctx, "local-app", "app.start", "BIT", serde_json::json!({}), true);
            app.manage(ctx.clone());

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
