use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

/// 通过本机运行时执行一段代码，把 `params`（JSON）通过 stdin 传入，
/// 约定脚本从 stdin 读取 JSON、把结果打印到 stdout（最好是 JSON）。
///
/// 根据运行时的 mode 分三条路径：
/// - interpret：源码写临时文件后直接跑（node/python/php/ruby/ps1/deno）
/// - compile：先编译再运行（java / rust）
/// - exec：直接调用可执行文件（exe，此时 code 为传给程序的命令行参数）
pub fn run(
    ctx: &Arc<crate::state::Ctx>,
    runtime_id: &str,
    code: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let rt = crate::runtime::get(ctx, runtime_id)
        .ok_or_else(|| format!("运行时 `{runtime_id}` 未注册，请先在「工具」页刷新探测"))?;

    if !rt.enabled {
        return Err(format!("解释器 `{}` 已暂停，请先在「工具」页启用", rt.name));
    }

    match rt.mode.as_str() {
        "compile" => run_compiled(&rt, code, params),
        "exec" => run_exec(&rt, code, params),
        _ => run_interpreted(&rt, code, params),
    }
}

/// 解释执行：源码写临时文件，交给解释器直接跑
fn run_interpreted(
    rt: &crate::runtime::Runtime,
    code: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let ext = match rt.lang.as_str() {
        "py" => "py",
        "ts" => "ts",
        "php" => "php",
        "rb" => "rb",
        "ps1" => "ps1",
        "pl" => "pl",
        "lua" => "lua",
        "r" => "r",
        "jl" => "jl",
        "groovy" => "groovy",
        "tcl" => "tcl",
        "exs" => "exs",
        _ => "js",
    };
    let tmp = std::env::temp_dir().join(format!("bit_{}.{ext}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&tmp, code).map_err(|e| format!("写入临时脚本失败: {e}"))?;

    let mut cmd = Command::new(&rt.path);
    if rt.id == "deno" {
        cmd.arg("run").arg("--allow-all");
    }
    for a in &rt.run_args {
        cmd.arg(a);
    }
    cmd.arg(&tmp);

    let out = spawn_with_stdin(cmd, params);
    let _ = std::fs::remove_file(&tmp);
    finalize(out)
}

/// 编译执行：Java / Rust。写源码 → 编译 → 运行产物
fn run_compiled(
    rt: &crate::runtime::Runtime,
    code: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let work = std::env::temp_dir().join(format!("bit_build_{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&work).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let result = match rt.lang.as_str() {
        // Java：JDK 11+ 支持 `java Main.java` 单文件源码直跑（内部自动编译）
        "java" => {
            let src = work.join("Main.java");
            std::fs::write(&src, code).map_err(|e| format!("写入源码失败: {e}"))?;
            let mut cmd = Command::new(&rt.path); // java
            cmd.arg(&src);
            spawn_with_stdin(cmd, params)
        }
        // Rust：rustc 编译成可执行文件后运行
        "rs" => {
            let src = work.join("main.rs");
            let bin = work.join(if cfg!(windows) { "main.exe" } else { "main" });
            std::fs::write(&src, code).map_err(|e| format!("写入源码失败: {e}"))?;
            let compile = Command::new(&rt.path) // rustc
                .arg(&src)
                .arg("-O")
                .arg("-o")
                .arg(&bin)
                .output();
            match compile {
                Ok(o) if o.status.success() => {
                    let cmd = Command::new(&bin);
                    spawn_with_stdin(cmd, params)
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr);
                    let err = if err.len() > 4000 { err[..4000].to_string() } else { err.to_string() };
                    Err(format!("Rust 编译失败:\n{err}"))
                }
                Err(e) => Err(format!("启动 rustc 失败: {e}")),
            }
        }
        // Go：go run 直接编译并运行单文件
        "go" => {
            let src = work.join("main.go");
            std::fs::write(&src, code).map_err(|e| format!("写入源码失败: {e}"))?;
            let mut cmd = Command::new(&rt.path); // go
            cmd.arg("run").arg(&src);
            spawn_with_stdin(cmd, params)
        }
        // C / C++：gcc / g++ 编译成可执行文件后运行
        "c" | "cpp" => {
            let ext = if rt.lang == "c" { "c" } else { "cpp" };
            let src = work.join(format!("main.{ext}"));
            let bin = work.join(if cfg!(windows) { "main.exe" } else { "main" });
            std::fs::write(&src, code).map_err(|e| format!("写入源码失败: {e}"))?;
            let compile = Command::new(&rt.path) // gcc / g++
                .arg(&src)
                .arg("-O2")
                .arg("-o")
                .arg(&bin)
                .output();
            match compile {
                Ok(o) if o.status.success() => {
                    let cmd = Command::new(&bin);
                    spawn_with_stdin(cmd, params)
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr);
                    let err = if err.len() > 4000 { err[..4000].to_string() } else { err.to_string() };
                    Err(format!("编译失败:\n{err}"))
                }
                Err(e) => Err(format!("启动编译器失败: {e}")),
            }
        }
        other => Err(format!("暂不支持的编译型语言: {other}")),
    };

    let _ = std::fs::remove_dir_all(&work);
    finalize(result)
}

/// 直接调用可执行文件：code 视为空白分隔的命令行参数，params 仍从 stdin 传入
fn run_exec(
    rt: &crate::runtime::Runtime,
    code: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut cmd = Command::new(&rt.path);
    for a in &rt.run_args {
        cmd.arg(a);
    }
    for a in code.split_whitespace() {
        cmd.arg(a);
    }
    finalize(spawn_with_stdin(cmd, params))
}

/// 启动子进程，把 params JSON 写入 stdin，收集 stdout/stderr
fn spawn_with_stdin(
    mut cmd: Command,
    params: &serde_json::Value,
) -> Result<std::process::Output, String> {
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("启动运行时失败: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let payload = serde_json::to_vec(params).unwrap_or_default();
        let _ = stdin.write_all(&payload);
    }
    child.wait_with_output().map_err(|e| format!("等待进程结束失败: {e}"))
}

/// 统一处理输出：非零退出返回错误，否则优先解析 stdout 为 JSON
fn finalize(out: Result<std::process::Output, String>) -> Result<serde_json::Value, String> {
    let output = out?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        let msg = if stderr.is_empty() { stdout } else { stderr };
        let msg = if msg.len() > 4000 { msg[..4000].to_string() } else { msg };
        return Err(format!("执行失败（退出码 {:?}）: {msg}", output.status.code()));
    }

    let out = if stdout.len() > 8000 { stdout[..8000].to_string() } else { stdout };
    match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => Ok(v),
        Err(_) => Ok(serde_json::json!({ "stdout": out })),
    }
}
