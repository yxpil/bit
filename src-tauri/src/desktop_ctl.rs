// yxpil · BIT
//! 本机操控三件套（screen / mouse / keyboard）的跨平台阻塞实现。
//! 全部在 spawn_blocking 中执行（enigo/screenshots 是阻塞 API）；
//! 平台差异：
//!   - Windows：Win32 SendInput / DXGI 截屏，开箱即用
//!   - macOS：CGEvent / CoreGraphics 截屏，首次使用需 TCC 授权（屏幕录制 + 辅助功能）
//!   - Linux：X11（XTest / XGetImage）；Wayland 下截屏/输入合成受限，报错指引

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};

/// enigo 实例不便跨线程共享（非 Sync），每次调用新建（开销 ~ms 级，可接受）
fn enigo_new() -> Result<Enigo, String> {
    Enigo::new(&Settings::default()).map_err(|e| format!("输入合成初始化失败: {e}"))
}

fn ie2s(e: enigo::InputError) -> String {
    format!("输入合成失败: {e}（macOS 需辅助功能授权；Linux Wayland 受限请用 X11）")
}

// ── 截屏 ────────────────────────────────────────────────────────────────────

/// 截屏：display=显示器序号（0=主屏），region=(x,y,w,h) 可选裁剪（物理像素）。
/// 返回 PNG 落盘路径（媒体缓存目录，对话 UI 自动出图）
pub fn screenshot(
    ctx: &std::sync::Arc<crate::state::Ctx>,
    display: usize,
    region: Option<(u32, u32, u32, u32)>,
) -> Result<String, String> {
    let screens = screenshots::Screen::all().map_err(|e| format!("枚举显示器失败: {e}"))?;
    let screen = screens
        .get(display)
        .ok_or(format!("显示器 {display} 不存在（共 {} 个，序号从 0 开始）", screens.len()))?;
    let img = screen.capture().map_err(|e| format!("截屏失败: {e}（macOS 需在 系统设置 → 隐私与安全性 → 屏幕录制 中授权 BIT；Linux Wayland 受限请用 X11）"))?;

    let mut rgba = image::RgbaImage::from_raw(img.width(), img.height(), img.into_raw())
        .ok_or("截屏数据尺寸异常")?;
    // 可选区域裁剪（按请求参数钳制到图像边界）
    if let Some((x, y, w, h)) = region {
        let (iw, ih) = rgba.dimensions();
        let x = x.min(iw.saturating_sub(1));
        let y = y.min(ih.saturating_sub(1));
        let w = w.min(iw.saturating_sub(x)).max(1);
        let h = h.min(ih.saturating_sub(y)).max(1);
        rgba = image::imageops::crop_imm(&rgba, x, y, w, h).to_image();
    }
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S%3f");
    let path = ctx.image_dir().join(format!("shot_{ts}.png"));
    let path_str = path.to_string_lossy().to_string();
    rgba.save(&path).map_err(|e| format!("截图保存失败: {e}"))?;
    Ok(path_str)
}

// ── 鼠标 ────────────────────────────────────────────────────────────────────

/// 鼠标操作：position / move / click / double_click / right_click / drag / scroll
pub fn mouse(action: &str, params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let xy = |k1: &str, k2: &str| -> Result<(i32, i32), String> {
        let x = params.get(k1).and_then(|v| v.as_f64()).ok_or(format!("Missing parameter: {k1}"))?;
        let y = params.get(k2).and_then(|v| v.as_f64()).ok_or(format!("Missing parameter: {k2}"))?;
        Ok((x as i32, y as i32))
    };
    let mut eg = enigo_new()?;
    match action {
        "position" => {
            let (x, y) = eg.location().map_err(ie2s)?;
            Ok(serde_json::json!({ "x": x, "y": y }))
        }
        "move" => {
            let (x, y) = xy("x", "y")?;
            eg.move_mouse(x, y, Coordinate::Abs).map_err(ie2s)?;
            Ok(serde_json::json!({ "ok": true, "action": "move", "x": x, "y": y }))
        }
        "click" | "double_click" | "right_click" => {
            let (x, y) = xy("x", "y")?;
            eg.move_mouse(x, y, Coordinate::Abs).map_err(ie2s)?;
            let button = if action == "right_click" { Button::Right } else { Button::Left };
            let times = if action == "double_click" { 2 } else { 1 };
            for _ in 0..times {
                eg.button(button, Direction::Click).map_err(ie2s)?;
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            Ok(serde_json::json!({ "ok": true, "action": action }))
        }
        "drag" => {
            let (x, y) = xy("x", "y")?;
            let (tx, ty) = xy("x2", "y2")?;
            eg.move_mouse(x, y, Coordinate::Abs).map_err(ie2s)?;
            eg.button(Button::Left, Direction::Press).map_err(ie2s)?;
            // 分 12 段平滑拖动：部分应用（画板/网页）不响应瞬移 drag
            for i in 1..=12 {
                let px = x + (tx - x) * i / 12;
                let py = y + (ty - y) * i / 12;
                eg.move_mouse(px, py, Coordinate::Abs).map_err(ie2s)?;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            eg.button(Button::Left, Direction::Release).map_err(ie2s)?;
            Ok(serde_json::json!({ "ok": true, "action": "drag", "from": [x, y], "to": [tx, ty] }))
        }
        "scroll" => {
            let dx = params.get("dx").and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
            let dy = params.get("dy").and_then(|v| v.as_f64()).unwrap_or(0.0) as i32;
            if dx == 0 && dy == 0 {
                return Err("scroll needs a non-zero dx or dy".into());
            }
            if dy != 0 {
                eg.scroll(dy, Axis::Vertical).map_err(ie2s)?;
            }
            if dx != 0 {
                eg.scroll(dx, Axis::Horizontal).map_err(ie2s)?;
            }
            Ok(serde_json::json!({ "ok": true, "action": "scroll" }))
        }
        other => Err(format!(
            "Unknown action '{other}'; available: position, move, click, double_click, right_click, drag, scroll"
        )),
    }
}

// ── 键盘 ────────────────────────────────────────────────────────────────────

/// 键名 → enigo Key 映射（命名键集 + 单字符 Unicode 直传）
fn named_key(k: &str) -> Option<Key> {
    Some(match k.to_lowercase().as_str() {
        "return" | "enter" => Key::Return,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "delete" | "backspace" => Key::Backspace,
        "forwarddelete" | "del" => Key::Delete,
        "escape" | "esc" => Key::Escape,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "left" | "leftarrow" => Key::LeftArrow,
        "right" | "rightarrow" => Key::RightArrow,
        "down" | "downarrow" => Key::DownArrow,
        "up" | "uparrow" => Key::UpArrow,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        _ => return None,
    })
}

/// 键盘操作：type（整段文本，支持 Unicode）/ key（单键 + cmd/ctrl/shift/option 修饰）
pub fn keyboard(action: &str, params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut eg = enigo_new()?;
    match action {
        "type" => {
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("Missing parameter: text")?;
            if text.is_empty() {
                return Err("text cannot be empty".into());
            }
            eg.text(text).map_err(|e| format!("文本输入失败: {e}"))?;
            Ok(serde_json::json!({ "ok": true, "action": "type", "chars": text.chars().count() }))
        }
        "key" => {
            let key = params
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or("Missing parameter: key")?;
            // 修饰键：按下 → 主键 → 释放（顺序保证组合生效）
            let mut mods: Vec<Key> = Vec::new();
            if params.get("cmd").and_then(|v| v.as_bool()).unwrap_or(false) {
                mods.push(Key::Meta);
            }
            if params.get("ctrl").and_then(|v| v.as_bool()).unwrap_or(false) {
                mods.push(Key::Control);
            }
            if params.get("shift").and_then(|v| v.as_bool()).unwrap_or(false) {
                mods.push(Key::Shift);
            }
            if params.get("option").and_then(|v| v.as_bool()).unwrap_or(false) {
                mods.push(Key::Alt);
            }
            for m in &mods {
                eg.key(*m, Direction::Press).map_err(ie2s)?;
            }
            let k = named_key(key)
                .or_else(|| {
                    // 单字符键：Unicode 直传（字母/数字/符号/中文）
                    let mut cs = key.chars();
                    let (Some(c), None) = (cs.next(), cs.next()) else { return None };
                    Some(Key::Unicode(c))
                })
                .ok_or(format!(
                    "Unknown key '{key}'; use a single character or a named key (return/tab/space/delete/escape/home/end/pageup/pagedown/left/right/up/down/f1-f12)"
                ))?;
            eg.key(k, Direction::Click).map_err(ie2s)?;
            for m in mods.into_iter().rev() {
                let _ = eg.key(m, Direction::Release);
            }
            Ok(serde_json::json!({ "ok": true, "action": "key", "key": key }))
        }
        other => Err(format!("Unknown action '{other}'; available: type, key")),
    }
}
