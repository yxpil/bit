// yxpil · BIT
// 权限检测：进程内 FFI（归属 BIT 本体，不再借道 osascript —— TCC 以实际可执行身份记录授权，
// 借道子进程会让系统把授权记到 osascript 头上，导致「授权了却一直不生效」）。
//
// 说明：screen/mouse/keyboard 仅 Windows/Linux 开放，ax_trusted 的唯一调用方 registry.rs
// 也被 cfg(not macos) 排除在 macOS 之外 → 本模块在 macOS 上属于「保留实现 + 单元测试」，
// 允许 dead_code，避免平台特有的误报。
#![cfg_attr(target_os = "macos", allow(dead_code))]
//
// 本机操控工具 screen / mouse / keyboard 按既定方案仅 Windows/Linux 开放
// （macOS 因 TCC 授权体验问题整体移除，见 config.rs::tool_gate / registry.rs 出厂清单），
// 因此：
//   - 非 macOS：没有「辅助功能 / 屏幕录制」这类系统授权概念，ax_trusted 恒为 true；
//     功能是否可用完全由设置页闸门（tool_screen/mouse/keyboard）控制，与权限无关。
//   - macOS：保留 AX 授权查询实现（AXIsProcessTrustedWithOptions 只读探测，不弹窗、不 panic）；
//     仅供可能的历史引用 / 未来在非 TCC 通道复用。探测失败一律按「未授权」处理，绝不崩溃。

/// 辅助功能（Accessibility）是否已对本进程授权。
/// 语义由调用方解释：未授权时应拒绝执行并给出可操作指引（见 registry::AX_DENIED_MSG）。
pub fn ax_trusted() -> bool {
    ax_trusted_impl()
}

#[cfg(not(target_os = "macos"))]
fn ax_trusted_impl() -> bool {
    // Windows/Linux 无 TCC 辅助功能授权；操控类功能由 config 闸门 + 平台实现各自把控
    true
}

#[cfg(target_os = "macos")]
fn ax_trusted_impl() -> bool {
    use std::ffi::c_void;
    // AXIsProcessTrustedWithOptions(NULL)：仅查询当前进程是否已勾选辅助功能。
    // 返回 0 = 未授权；非 0 = 已授权。传入 NULL options 不会触发授权弹窗。
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> i32;
    }
    // FFI 调用本身不抛异常；即便系统服务异常也只返回 0（按未授权处理，不影响主进程）
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ax_trusted 永不 panic、永远返回布尔：非 macOS 恒 true（无授权概念），macOS 返回探测结果
    #[test]
    fn ax_trusted_is_total_and_never_panics() {
        let _: bool = ax_trusted();
        if !cfg!(target_os = "macos") {
            assert!(ax_trusted());
        }
    }
}
