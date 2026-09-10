// yxpil · BIT
// BIT toolhomes：AI 代码与工具环境的统一收纳目录 + 默认 shell 识别。
//
// toolhomes/（位于数据目录下）
//   pyvenv/        python 虚拟环境：首次启动自动创建，AI 自建工具与 run_script 的
//                  py 代码默认跑在这里面（pip 装的包互不污染系统环境）
//   node_modules/  npm 包：NODE_PATH 指向，AI 装的 js 依赖统一落在这里
//   jobs/          自定义工具后台任务的输出日志
//
// 设计原则：环境初始化全自动、失败静默（缺 python 就跳过 venv，不阻塞启动）。
use std::path::PathBuf;
use std::sync::Arc;

/// toolhomes 根目录
pub fn dir(ctx: &Arc<crate::state::Ctx>) -> PathBuf {
    ctx.data_dir.join("toolhomes")
}

/// python 虚拟环境目录
pub fn pyvenv_dir(ctx: &Arc<crate::state::Ctx>) -> PathBuf {
    dir(ctx).join("pyvenv")
}

/// venv 内的 python 可执行文件（存在才返回 Some）
pub fn venv_python(ctx: &Arc<crate::state::Ctx>) -> Option<PathBuf> {
    let p = if cfg!(windows) {
        pyvenv_dir(ctx).join("Scripts").join("python.exe")
    } else {
        pyvenv_dir(ctx).join("bin").join("python")
    };
    p.exists().then_some(p)
}

/// 启动时调用：建目录 + 缺 venv 时后台补建。全部静默失败（不阻塞启动）。
/// 返回是否新建了 venv（供日志/事件提示）。
pub fn ensure_init(ctx: &Arc<crate::state::Ctx>) -> bool {
    let root = dir(ctx);
    let _ = std::fs::create_dir_all(root.join("node_modules"));
    let _ = std::fs::create_dir_all(root.join("jobs"));
    if venv_python(ctx).is_some() {
        return false;
    }
    // 找系统 python 建 venv：优先 py 启动器（Windows），其次 PATH 上的 python/python3
    let candidates: Vec<&str> = if cfg!(windows) {
        vec!["py", "python", "python3"]
    } else {
        vec!["python3", "python"]
    };
    for c in candidates {
        let mut cmd = std::process::Command::new(c);
        if c == "py" {
            cmd.arg("-3");
        }
        cmd.arg("-m").arg("venv").arg(pyvenv_dir(ctx));
        crate::registry::no_window(&mut cmd);
        if let Ok(out) = cmd.output() {
            if out.status.success() && venv_python(ctx).is_some() {
                crate::trace::event("toolenv", "pyvenv created");
                return true;
            }
        }
    }
    false
}

/// 在 PATH 上查找可执行文件（跨平台，不依赖外部命令）
fn find_in_path(program: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let exe = if cfg!(windows) {
            dir.join(format!("{program}.exe"))
        } else {
            dir.join(program)
        };
        if exe.is_file() {
            return true;
        }
        // Windows 上 PowerShell 7 的可执行名是 pwsh.exe；powershell 在 System32 无需后缀场景已覆盖
        if cfg!(windows) && program == "pwsh" && dir.join("pwsh.exe").is_file() {
            return true;
        }
    }
    false
}

/// 检测本机可用的 shell 列表（供设置页下拉）：按推荐顺序返回
pub fn available_shells() -> Vec<String> {
    let mut out = Vec::new();
    if cfg!(windows) {
        for s in ["pwsh", "powershell", "cmd"] {
            if find_in_path(s) {
                out.push(s.to_string());
            }
        }
    } else {
        // Unix：登录用户的 $SHELL 优先（最顺手），再补常见项
        if let Some(sh) = std::env::var("SHELL").ok().filter(|s| !s.is_empty()) {
            out.push(sh);
        }
        for s in ["bash", "zsh", "fish", "sh"] {
            if find_in_path(s) && !out.iter().any(|x| x.ends_with(s)) {
                out.push(s.to_string());
            }
        }
    }
    out
}

/// 解析默认 shell：返回 (程序, 包装参数模板占位说明)。
/// 模板约定：包装参数里不含命令本身，调用方把命令作为最后一个参数追加。
///   pwsh/powershell → ["-NoProfile", "-NonInteractive", "-Command"]
///   cmd             → ["/C"]
///   bash/zsh/fish/sh→ ["-c"]
/// pref 为空时自动识别（Windows pwsh→powershell；Unix $SHELL→bash）。
/// 返回 Err 表示配置指定的 shell 不存在（调用方可回退自动识别）。
pub fn resolve_shell(pref: &str) -> Result<(String, Vec<String>), String> {
    let pick = |prog: &str| -> Option<(String, Vec<String>)> {
        let base = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
        let args: Vec<String> = match base {
            "pwsh" | "powershell" => vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
            ],
            "cmd" => vec!["/C".into()],
            "bash" | "zsh" | "sh" | "fish" | "dash" | "ksh" => vec!["-c".into()],
            _ => return None,
        };
        Some((prog.to_string(), args))
    };
    if !pref.trim().is_empty() {
        if let Some(found) = pick(pref.trim()) {
            return Ok(found);
        }
        return Err(format!("shell `{pref}` not found"));
    }
    // 自动识别
    if cfg!(windows) {
        for s in ["pwsh", "powershell"] {
            if find_in_path(s) {
                return Ok(pick(s).unwrap());
            }
        }
        Ok(pick("cmd").unwrap())
    } else {
        if let Some(sh) = std::env::var("SHELL").ok().filter(|s| !s.is_empty()) {
            if let Some(found) = pick(&sh) {
                return Ok(found);
            }
        }
        for s in ["bash", "zsh", "sh"] {
            if find_in_path(s) {
                return Ok(pick(s).unwrap());
            }
        }
        Err("no shell found".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_shell_auto_works() {
        // 自动识别在常见环境必然能找到一个可用 shell
        let (prog, args) = resolve_shell("").expect("auto shell resolution must succeed");
        assert!(!prog.is_empty());
        assert!(!args.is_empty());
    }

    #[test]
    fn resolve_shell_pref_missing_falls_back() {
        // 指定不存在的 shell 时报错（调用方回退），而不是 panic
        assert!(resolve_shell("definitely-not-a-shell-xyz").is_err());
    }

    #[test]
    fn resolve_shell_explicit() {
        if cfg!(windows) {
            if find_in_path("pwsh") {
                let (p, a) = resolve_shell("pwsh").unwrap();
                assert_eq!(p, "pwsh");
                assert!(a.contains(&"-Command".to_string()));
            }
        } else if find_in_path("bash") {
            let (p, a) = resolve_shell("bash").unwrap();
            assert_eq!(p, "bash");
            assert_eq!(a, vec!["-c".to_string()]);
        }
    }
}
