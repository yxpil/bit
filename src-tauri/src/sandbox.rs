// yxpil · BIT
//! 工作区沙箱：把 AI 的 shell / 文件操作约束在工作区根目录内。
//!
//! - TUI 模式启动时根目录 = 运行 `bit tui` 的当前目录（Agent 只在"执行该命令的路径"下工作）
//! - 桌面端默认无根（None）= 行为与旧版完全一致；可在 config.workspace_root 显式开启
//! - 相对路径一律锚定到根；绝对路径 canonicalize 后必须位于根内，拒绝 `..` 逃逸
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::state::Ctx;

/// 生效中的沙箱根：运行时值（TUI 启动时设置）优先，其次 config.workspace_root
pub fn effective_root(ctx: &Arc<Ctx>) -> Option<PathBuf> {
    if let Some(r) = ctx.workspace_root.lock().unwrap().clone() {
        return Some(r);
    }
    let cfg = ctx.config.lock().unwrap();
    cfg.workspace_root.as_deref().map(PathBuf::from)
}

///  lexical 规范化（不触碰文件系统）：消解 `.` / `..`，保留不存在的路径可继续处理
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 把 AI 给的路径解析到沙箱内：
/// - 无根：原样返回（桌面端旧行为）
/// - 相对路径：锚定根目录
/// - 绝对路径：规范化后必须仍在根内（含根本身）
pub fn resolve_path(ctx: &Arc<Ctx>, raw: &str) -> Result<PathBuf, String> {
    let Some(root) = effective_root(ctx) else {
        return Ok(PathBuf::from(raw));
    };
    let p = Path::new(raw);
    let candidate = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    let norm = normalize(&candidate);
    if !norm.starts_with(&root) {
        return Err(format!(
            "路径 `{raw}` 越出工作区沙箱 `{}`；文件操作只允许在工作区内进行",
            root.display()
        ));
    }
    Ok(norm)
}

/// shell 工作目录解析：
/// - AI 显式传 cwd：同样做沙箱校验（相对锚定 / 绝对禁逃逸）
/// - 未传且有沙箱：兜底为根目录（旧行为是继承进程 cwd，桌面端 GUI 启动时不可控）
/// - 无根且未传：None（继承进程 cwd，与旧行为一致）
pub fn resolve_cwd(ctx: &Arc<Ctx>, cwd: Option<&str>) -> Result<Option<String>, String> {
    match cwd {
        Some(dir) if !dir.trim().is_empty() => Ok(Some(resolve_path(ctx, dir)?.to_string_lossy().into_owned())),
        _ => Ok(effective_root(ctx).map(|r| r.to_string_lossy().into_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 纯函数部分（normalize / 前缀判定逻辑）直接测，不构造 Ctx
    #[test]
    fn normalize_strips_dot_segments() {
        assert_eq!(
            normalize(Path::new("/work/proj/src/../package.json")),
            PathBuf::from("/work/proj/package.json")
        );
        assert_eq!(normalize(Path::new("a/./b")), PathBuf::from("a/b"));
    }

    #[test]
    fn parent_traversal_escapes_root() {
        // 模拟 resolve 的前缀判定：../../etc 规范化后不在根内
        let root = PathBuf::from("/work/proj");
        let norm = normalize(&root.join("../../etc/passwd"));
        assert!(!norm.starts_with(&root));
    }

    #[test]
    fn nested_path_stays_inside() {
        let root = PathBuf::from("/work/proj");
        let norm = normalize(&root.join("src/./main.rs"));
        assert!(norm.starts_with(&root));
    }
}
