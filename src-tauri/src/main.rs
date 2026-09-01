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
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let ctx = state::Ctx::load(app.handle().clone());
            audit::record(&ctx, "local-app", "app.start", "BIT", serde_json::json!({}), true);
            app.manage(ctx.clone());

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
            commands::context_preview,
            commands::list_sessions,
            commands::get_session,
            commands::create_session,
            commands::set_active_session,
            commands::rename_session,
            commands::delete_session,
            commands::clear_session,
            commands::list_memories,
            commands::add_memory,
            commands::list_skills,
            commands::add_skill,
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
            commands::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running BIT");
}
