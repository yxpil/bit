// yxpil · BIT
use serde_json::json;
use std::sync::Arc;
use tauri::menu::{IsMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

use crate::state::Ctx;

/// 创建系统托盘：状态信息 / 显示窗口 / 远程服务 / 退出
pub fn create(app: &tauri::AppHandle, ctx: &Arc<Ctx>) -> tauri::Result<()> {
    let menu = build_menu(app, ctx)?;
    let quit_ctx = ctx.clone();

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
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => {
                // 直接用闭包捕获的 ctx，不依赖 try_state（Tauri 在某些平台上
                // try_state 在菜单事件回调中会返回 None，导致 expect_exit 丢失）
                crate::audit::record(&quit_ctx, "local-app", "app.quit", "BIT", json!({ "via": "tray" }), true);
                crate::guardian::expect_exit(&quit_ctx);
                // 硬退出兜底：app.exit(0) 依赖事件循环消费，历史上出现过"审计记了但进程不退"
                // （ExitRequested 重入 / 主线程阻塞）。后台线程完成换装后强杀进程，
                // 保证托盘退出 100% 生效；NSIS 安装器在进程退出后自行完成文件替换
                let hard_ctx = quit_ctx.clone();
                std::thread::spawn(move || {
                    let _ = crate::update::apply_update(&hard_ctx, false);
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    std::process::exit(0);
                });
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    tray.set_show_menu_on_left_click(false)?;

    // 状态轮询：每 4 秒重建菜单 + 刷新 tooltip（状态廉价读取，菜单构建仅在主线程）
    let refresh_app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            refresh(&refresh_app);
        }
    });

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

/// 刷新托盘菜单文案（Autopilot 状态 / 远程地址）。
/// Windows 上 tray-icon 的菜单构建/替换必须发生在主线程：从 tokio worker 或
/// command 线程直接 set_menu 会与 UI 事件循环互相等待，表现为整个程序卡死
/// （macOS/Linux 的托盘 API 自带线程安全，因此只在 Windows 复现）。
/// 统一在此转发到主线程执行，调用方（commands/http_api）无需关心所在线程。
pub fn refresh(app: &tauri::AppHandle) {
    let app = app.clone();
    let _ = app.run_on_main_thread({
        let app = app.clone();
        move || {
            // 重建菜单并替换（TrayIcon 不提供菜单项 getter）
            let Some(tray) = app.tray_by_id("bit-tray") else { return };
            let Some(ctx) = app.try_state::<Arc<Ctx>>() else { return };
            let tip = tooltip_text(ctx.inner());
            // 状态签名未变化时跳过重建：避免 macOS 上展开中的菜单被无谓替换闪断
            // （签名涵盖工作状态 / Autopilot / 引擎模式 / 远程地址全部动态文案）
            static LAST_SIG: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
            let sig = status_signature(ctx.inner());
            let changed = LAST_SIG.lock().unwrap().as_str() != sig;
            if changed {
                if let Ok(menu) = build_menu(&app, ctx.inner()) {
                    let _ = tray.set_menu(Some(menu));
                }
                *LAST_SIG.lock().unwrap() = sig;
            }
            let _ = tray.set_tooltip(Some(tip));
        }
    });
}

/// 托盘 tooltip：一句话概括运行状态
fn tooltip_text(ctx: &Arc<Ctx>) -> String {
    if crate::worker::busy(ctx) {
        "BIT · 工作中…".to_string()
    } else {
        "BIT · 空闲".to_string()
    }
}

/// 状态摘要菜单项组（只读，不可点）：工作状态 / Autopilot / 引擎模式
fn status_items(
    app: &tauri::AppHandle,
    ctx: &Arc<Ctx>,
) -> tauri::Result<Vec<tauri::menu::MenuItem<tauri::Wry>>> {
    let turns = ctx.turn_locks.lock().map(|m| m.len()).unwrap_or(0);
    let status = if crate::worker::busy(ctx) {
        format!("状态: 工作中（{turns} 回合在跑）")
    } else {
        "状态: 空闲".to_string()
    };
    let autopilot = if ctx.autopilot_running.load(std::sync::atomic::Ordering::SeqCst) {
        "Autopilot: 运行中"
    } else {
        "Autopilot: 已暂停"
    };
    let engine = engine_label(ctx);
    // enabled=false = 纯展示项，点击无动作
    Ok(vec![
        MenuItem::with_id(app, "st-busy", status, false, None::<&str>)?,
        MenuItem::with_id(app, "st-autopilot", autopilot, false, None::<&str>)?,
        MenuItem::with_id(app, "st-engine", engine, false, None::<&str>)?,
    ])
}

/// 引擎模式文案
fn engine_label(ctx: &Arc<Ctx>) -> &'static str {
    if crate::worker::active() {
        "引擎: worker 子进程"
    } else if crate::worker::enabled(ctx) {
        "引擎: 进程内（worker 回退中）"
    } else {
        "引擎: 进程内（未启用 worker）"
    }
}

/// 全部动态文案拼成的变更签名（托盘防抖重建用）
fn status_signature(ctx: &Arc<Ctx>) -> String {
    let busy = crate::worker::busy(ctx);
    let turns = ctx.turn_locks.lock().map(|m| m.len()).unwrap_or(0);
    format!(
        "{}|{}|{}|{}",
        busy,
        turns,
        ctx.autopilot_running.load(std::sync::atomic::Ordering::SeqCst),
        remote_label(ctx),
    )
}

fn build_menu(
    app: &tauri::AppHandle,
    ctx: &Arc<Ctx>,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let status = status_items(app, ctx)?;
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let remote = MenuItem::with_id(app, "remote", remote_label(ctx), true, None::<&str>)?;
    let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
    let sep2 = tauri::menu::PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出 BIT", true, None::<&str>)?;
    // IsMenuItem trait 统一异构项（Menu/MenuItem/Predefined）的引用数组
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = Vec::new();
    for it in &status {
        items.push(it);
    }
    items.push(&sep);
    items.push(&show);
    items.push(&remote);
    items.push(&sep2);
    items.push(&quit);
    Menu::with_items(app, &items)
}

pub fn show_main_window(app: &tauri::AppHandle) {
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

/// 注册/刷新「唤出主界面」全局快捷键（config.hotkey_show）：
/// 空字符串 = 注销全部热键；启动与设置保存后都会调用（幂等：先注销再注册）。
/// 热键行为 = 切换主窗口显隐（可见时隐藏，隐藏/最小化时唤出聚焦）。
/// 返回注册结果：热键被其他应用占用 / 格式非法时 Err（调用方据此拒绝保存，实现冲突检测）
pub fn register_hotkey(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let hotkey = app
        .try_state::<Arc<crate::state::Ctx>>()
        .map(|c| c.config.lock().unwrap().hotkey_show.trim().to_string())
        .unwrap_or_default();
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| e.to_string())?;
    if hotkey.is_empty() {
        return Ok(());
    }
    let hk = hotkey.clone();
    gs.on_shortcut(hotkey.as_str(), move |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            toggle_main_window(app);
        }
    })
    .map_err(|e| {
        // 热键被其他应用占用 / 格式非法：记审计并上抛原因（设置页展示）
        if let Some(ctx) = app.try_state::<Arc<crate::state::Ctx>>() {
            crate::audit::record(
                &ctx,
                "local-app",
                "hotkey.register_failed",
                "tray",
                serde_json::json!({ "hotkey": hk, "error": e.to_string() }),
                false,
            );
        }
        e.to_string()
    })
}
