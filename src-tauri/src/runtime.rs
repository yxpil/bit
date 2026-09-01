use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::Arc;

/// 一个可执行解释器/运行时（node / python / deno / java / rust / …）
/// AI 只要写一段该语言的代码，就能被 BIT 通过对应运行时执行，从而成为工具。
#[derive(Serialize, Deserialize, Clone)]
pub struct Runtime {
    /// 唯一标识，如 "node" / "python" / "java" / "rust"
    pub id: String,
    /// 展示名称
    pub name: String,
    /// 可执行文件路径（探测到的绝对路径或命令名）
    pub path: String,
    /// 版本字符串（探测所得）
    pub version: String,
    /// 语言类型标签：js / py / ts / java / rs / exe …（提示 AI 该写什么语言）
    pub lang: String,
    /// 执行方式：
    /// - "interpret" 解释执行：源码写临时文件后直接跑（js/py/php/rb/ps1/ts）
    /// - "compile"   编译执行：先编译再运行（java/rust）
    /// - "exec"      直接调用可执行文件（exe，无源码）
    #[serde(default = "default_mode")]
    pub mode: String,
    /// 运行代码时附加的参数
    #[serde(default)]
    pub run_args: Vec<String>,
    /// 是否手动添加（手动添加的在刷新探测时保留）
    #[serde(default)]
    pub manual: bool,
    /// 是否启用。暂停（false）后 AI 不能用它执行代码/注册工具，但保留探测结果。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_mode() -> String {
    "interpret".into()
}

fn default_enabled() -> bool {
    true
}

/// 内置候选运行时：命令名 -> (展示名, 语言, 执行方式, 版本探测参数)
/// 全部走「自动扫描、探测到才注册」——本机没有的语言不会出现。
fn candidates() -> Vec<(&'static str, &'static str, &'static str, &'static str, &'static str)> {
    vec![
        // ── 解释型 ──
        ("node", "Node.js", "js", "interpret", "--version"),
        ("python", "Python", "py", "interpret", "--version"),
        ("python3", "Python 3", "py", "interpret", "--version"),
        ("deno", "Deno", "ts", "interpret", "--version"),
        ("bun", "Bun", "js", "interpret", "--version"),
        ("php", "PHP", "php", "interpret", "--version"),
        ("ruby", "Ruby", "rb", "interpret", "--version"),
        ("perl", "Perl", "pl", "interpret", "--version"),
        ("lua", "Lua", "lua", "interpret", "-v"),
        ("luajit", "LuaJIT", "lua", "interpret", "-v"),
        ("Rscript", "R", "r", "interpret", "--version"),
        ("julia", "Julia", "jl", "interpret", "--version"),
        ("groovy", "Groovy", "groovy", "interpret", "--version"),
        ("tclsh", "Tcl", "tcl", "interpret", "% puts [info patchlevel]; exit"),
        ("elixir", "Elixir", "exs", "interpret", "--version"),
        ("pwsh", "PowerShell 7+", "ps1", "interpret", "-Command $PSVersionTable.PSVersion.ToString()"),
        ("powershell", "PowerShell", "ps1", "interpret", "-Command $PSVersionTable.PSVersion.ToString()"),
        // ── 编译型（写源码 → 编译 → 运行）──
        ("java", "Java", "java", "compile", "-version"),
        ("rustc", "Rust", "rs", "compile", "--version"),
        ("go", "Go", "go", "compile", "version"),
        ("gcc", "C (gcc)", "c", "compile", "--version"),
        ("g++", "C++ (g++)", "cpp", "compile", "--version"),
        ("dotnet", "C# (.NET)", "cs", "compile", "--version"),
        ("kotlinc", "Kotlin", "kt", "compile", "-version"),
        ("swiftc", "Swift", "swift", "compile", "--version"),
    ]
}

/// 探测某个命令是否可用，返回其路径与版本
fn probe(cmd: &str, version_arg: &str) -> Option<(String, String)> {
    // 版本探测参数可能含空格（powershell），拆分处理
    let args: Vec<&str> = version_arg.split(' ').filter(|s| !s.is_empty()).collect();
    // 启动时批量探测，必须隐藏子进程控制台窗口（否则启动瞬间黑窗闪烁）
    let mut c = Command::new(cmd);
    c.args(&args);
    crate::registry::no_window(&mut c);
    let output = c.output().ok()?;
    if !output.status.success() && output.stdout.is_empty() && output.stderr.is_empty() {
        return None;
    }
    let mut ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if ver.is_empty() {
        ver = String::from_utf8_lossy(&output.stderr).trim().to_string();
    }
    // 取首行、去掉多余前缀
    let ver = ver.lines().next().unwrap_or("").trim().to_string();
    // 尝试解析可执行文件的真实路径
    let path = which(cmd).unwrap_or_else(|| cmd.to_string());
    Some((path, if ver.is_empty() { "unknown".into() } else { ver }))
}

/// 类似 `where` / `which`：定位命令的绝对路径
fn which(cmd: &str) -> Option<String> {
    #[cfg(windows)]
    let finder = "where";
    #[cfg(not(windows))]
    let finder = "which";
    let mut c = Command::new(finder);
    c.arg(cmd);
    crate::registry::no_window(&mut c);
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
}

/// 扫描本机可用运行时
pub fn detect() -> Vec<Runtime> {
    let mut found: Vec<Runtime> = Vec::new();
    for (cmd, name, lang, mode, ver_arg) in candidates() {
        // python 与 python3 只保留一个可用的即可，避免重复
        if cmd == "python3" && found.iter().any(|r| r.lang == "py") {
            continue;
        }
        if let Some((path, version)) = probe(cmd, ver_arg) {
            let id = if cmd == "python3" { "python".to_string() } else { cmd.to_string() };
            if found.iter().any(|r| r.id == id) {
                continue;
            }
            found.push(Runtime {
                id,
                name: name.to_string(),
                path,
                version,
                lang: lang.to_string(),
                mode: mode.to_string(),
                run_args: Vec::new(),
                manual: false,
                enabled: true,
            });
        }
    }
    found
}

/// 刷新解释器列表：重新探测，同时保留用户手动添加的项与各自的启用状态
pub fn refresh(ctx: &Arc<crate::state::Ctx>) -> Vec<Runtime> {
    // 记录旧的启用状态（按 id），刷新后沿用，避免暂停状态被重置
    let (manual, prev_enabled): (Vec<Runtime>, std::collections::HashMap<String, bool>) = {
        let list = ctx.runtimes.lock().unwrap();
        (
            list.iter().filter(|r| r.manual).cloned().collect(),
            list.iter().map(|r| (r.id.clone(), r.enabled)).collect(),
        )
    };
    let mut detected = detect();
    for r in detected.iter_mut() {
        if let Some(&en) = prev_enabled.get(&r.id) {
            r.enabled = en;
        }
    }
    for m in manual {
        if !detected.iter().any(|r| r.id == m.id) {
            detected.push(m);
        }
    }
    {
        let mut list = ctx.runtimes.lock().unwrap();
        *list = detected.clone();
    }
    ctx.save_runtimes();
    detected
}

/// 手动添加一个解释器
pub fn add_manual(
    ctx: &Arc<crate::state::Ctx>,
    id: &str,
    name: &str,
    path: &str,
    lang: &str,
) -> Result<Runtime, String> {
    let id = id.trim();
    let path = path.trim();
    if id.is_empty() || path.is_empty() {
        return Err("解释器 id 与 path 不能为空".into());
    }
    // 试探版本
    let version = probe(path, "--version").map(|(_, v)| v).unwrap_or_else(|| "manual".into());
    let lang = if lang.trim().is_empty() { "js".to_string() } else { lang.trim().to_string() };
    // 依据语言推断执行方式
    let mode = match lang.as_str() {
        "java" | "rs" | "go" | "c" | "cpp" | "cs" | "kt" | "swift" => "compile",
        "exe" => "exec",
        _ => "interpret",
    }
    .to_string();
    let rt = Runtime {
        id: id.to_string(),
        name: if name.trim().is_empty() { id.to_string() } else { name.trim().to_string() },
        path: path.to_string(),
        version,
        lang,
        mode,
        run_args: Vec::new(),
        manual: true,
        enabled: true,
    };
    {
        let mut list = ctx.runtimes.lock().unwrap();
        list.retain(|r| r.id != rt.id);
        list.push(rt.clone());
    }
    ctx.save_runtimes();
    Ok(rt)
}

pub fn remove(ctx: &Arc<crate::state::Ctx>, id: &str) -> Result<(), String> {
    let mut list = ctx.runtimes.lock().unwrap();
    let before = list.len();
    list.retain(|r| r.id != id);
    if list.len() == before {
        return Err(format!("解释器 `{id}` 不存在"));
    }
    drop(list);
    ctx.save_runtimes();
    Ok(())
}

pub fn get(ctx: &Arc<crate::state::Ctx>, id: &str) -> Option<Runtime> {
    ctx.runtimes.lock().unwrap().iter().find(|r| r.id == id).cloned()
}

/// 暂停 / 启用某个解释器。暂停后 AI 不能用它执行代码或注册工具。
pub fn set_enabled(ctx: &Arc<crate::state::Ctx>, id: &str, enabled: bool) -> Result<bool, String> {
    let mut list = ctx.runtimes.lock().unwrap();
    let rt = list
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("解释器 `{id}` 不存在"))?;
    rt.enabled = enabled;
    drop(list);
    ctx.save_runtimes();
    Ok(enabled)
}
