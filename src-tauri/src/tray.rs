use serde_json::json;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

use crate::state::Ctx;

/// 创建系统托盘：显示窗口 / 远程服务 / 退出
pub fn create(app: &tauri::AppHandle, ctx: &Arc<Ctx>) -> tauri::Result<()> {
    let menu = build_menu(app, ctx)?;

    let tray = TrayIconBuilder::with_id("bit-tray")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("BIT - Agent Tool Hub")
        .menu(&menu)
        // 左键点击切换窗口显示
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                toggle_main_window(app);
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => {
                if let Some(ctx) = app.try_state::<Arc<Ctx>>() {
                    crate::audit::record(&ctx, "local-app", "app.quit", "BIT", json!({ "via": "tray" }), true);
                }
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    tray.set_show_menu_on_left_click(false)?;
    Ok(())
}

fn remote_label(ctx: &Arc<Ctx>) -> String {
    let cfg = ctx.config.lock().unwrap();
    if cfg.remote_enabled {
        format!("远程服务: {}:{}", cfg.host, cfg.port)
    } else {
        "远程服务: 已关闭".to_string()
    }
}

/// 刷新托盘菜单文案（Autopilot 状态 / 远程地址）
pub fn refresh(app: &tauri::AppHandle) {
    let Some(ctx) = app.try_state::<Arc<Ctx>>() else { return };
    let Some(tray) = app.tray_by_id("bit-tray") else { return };

    // 重建菜单并替换（TrayIcon 不提供菜单项 getter）
    if let Ok(menu) = build_menu(app, &ctx) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn build_menu(
    app: &tauri::AppHandle,
    ctx: &Arc<Ctx>,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let remote = MenuItem::with_id(app, "remote", remote_label(ctx), true, None::<&str>)?;
    let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出 BIT", true, None::<&str>)?;
    Menu::with_items(app, &[&show, &remote, &sep, &quit])
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

fn toggle_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            show_main_window(app);
        }
    }
}
