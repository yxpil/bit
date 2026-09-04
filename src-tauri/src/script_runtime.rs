use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 子进程统一执行上限。registry 外层也有 30 秒 tokio 超时，这里负责真正杀掉
/// 失控进程——tokio 超时只能放弃 JoinHandle，杀不掉已 spawn 的子进程。
const CHILD_TIMEOUT: Duration = Duration::from_secs(30);
/// stdout/stderr 采集上限（8MB）：防止失控脚本无限打印把内存吃满
const OUTPUT_CAP: u64 = 8 << 20;

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
        .ok_or_else(|| format!("Runtime `{runtime_id}` is not registered; refresh the runtime detection on the Tools page first"))?;

    if !rt.enabled {
        return Err(format!("Interpreter `{}` is paused; enable it on the Tools page first", rt.name));
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
    write_private(&tmp, code).map_err(|e| format!("Failed to write temp script: {e}"))?;

    let mut cmd = Command::new(&rt.path);
    if rt.id == "deno" {
        cmd.arg("run").arg("--allow-all");
    }
    for a in &rt.run_args {
        cmd.arg(a);
    }
    cmd.arg(&tmp);

    let out = run_with_limit(cmd, Some(params), CHILD_TIMEOUT);
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
    std::fs::create_dir_all(&work).map_err(|e| format!("Failed to create temp dir: {e}"))?;

    let result = match rt.lang.as_str() {
        // Java：JDK 11+ 支持 `java Main.java` 单文件源码直跑（内部自动编译）
        "java" => {
            let src = work.join("Main.java");
            write_private(&src, code).map_err(|e| format!("Failed to write source: {e}"))?;
            let mut cmd = Command::new(&rt.path); // java
            cmd.arg(&src);
            run_with_limit(cmd, Some(params), CHILD_TIMEOUT)
        }
        // Rust：rustc 编译成可执行文件后运行
        "rs" => {
            let src = work.join("main.rs");
            let bin = work.join(if cfg!(windows) { "main.exe" } else { "main" });
            write_private(&src, code).map_err(|e| format!("Failed to write source: {e}"))?;
            let mut compile = Command::new(&rt.path); // rustc
            compile.arg(&src).arg("-O").arg("-o").arg(&bin);
            crate::registry::no_window(&mut compile);
            match run_with_limit(compile, None, CHILD_TIMEOUT) {
                Ok(o) if o.status.success() => {
                    let cmd = Command::new(&bin);
                    run_with_limit(cmd, Some(params), CHILD_TIMEOUT)
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr);
                    let err = if err.len() > 4000 { err[..4000].to_string() } else { err.to_string() };
                    Err(format!("Rust compilation failed:\n{err}"))
                }
                Err(e) => Err(format!("rustc: {e}")),
            }
        }
        // Go：go run 直接编译并运行单文件
        "go" => {
            let src = work.join("main.go");
            write_private(&src, code).map_err(|e| format!("Failed to write source: {e}"))?;
            let mut cmd = Command::new(&rt.path); // go
            cmd.arg("run").arg(&src);
            run_with_limit(cmd, Some(params), CHILD_TIMEOUT)
        }
        // C / C++：gcc / g++ 编译成可执行文件后运行
        "c" | "cpp" => {
            let ext = if rt.lang == "c" { "c" } else { "cpp" };
            let src = work.join(format!("main.{ext}"));
            let bin = work.join(if cfg!(windows) { "main.exe" } else { "main" });
            write_private(&src, code).map_err(|e| format!("Failed to write source: {e}"))?;
            let mut compile = Command::new(&rt.path); // gcc / g++
            compile
                .arg(&src)
                .arg("-O2")
                .arg("-o")
                .arg(&bin);
            crate::registry::no_window(&mut compile);
            match run_with_limit(compile, None, CHILD_TIMEOUT) {
                Ok(o) if o.status.success() => {
                    let cmd = Command::new(&bin);
                    run_with_limit(cmd, Some(params), CHILD_TIMEOUT)
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr);
                    Err(format!("Compilation failed:\n{}", crate::registry::safe_trunc(&err, 4000)))
                }
                Err(e) => Err(format!("compiler: {e}")),
            }
        }
        other => Err(format!("Compiled language not supported yet: {other}")),
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
    finalize(run_with_limit(cmd, Some(params), CHILD_TIMEOUT))
}

/// 写入 0600 权限的私有临时文件：脚本源码可能来自 AI，不对同机其他用户可读
fn write_private(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    std::fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// 带上限地读一个子进程输出流；触顶时置位 capped 标志（由外层杀进程）并停止读取
fn read_capped<R: Read>(mut r: R, capped: Arc<std::sync::atomic::AtomicBool>) -> (bool, Vec<u8>) {
    use std::sync::atomic::Ordering;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() + n > OUTPUT_CAP as usize {
                    let take = OUTPUT_CAP as usize - buf.len();
                    buf.extend_from_slice(&chunk[..take]);
                    capped.store(true, Ordering::Relaxed);
                    return (true, buf);
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(_) => break,
        }
    }
    (false, buf)
}

/// 启动子进程并等待结束：
/// - 超时真正 kill 子进程（tokio 外层超时只能放弃 JoinHandle，杀不掉已 spawn 的进程）
/// - stdin 写入放独立线程：params 超过管道缓冲且子进程不读 stdin 时，write_all 会永远阻塞
/// - stdout/stderr 采集封顶 OUTPUT_CAP：超限视为失控，直接杀进程（finalize 本来就只留 8000 字符）
/// - 采集线程在等退出期间就位：不排空管道的话，子进程写满 64KB 缓冲会阻塞到被误杀
fn run_with_limit(
    mut cmd: Command,
    stdin_payload: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<Output, String> {
    crate::registry::no_window(&mut cmd);
    cmd.stdin(if stdin_payload.is_some() { Stdio::piped() } else { Stdio::null() });
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Failed to start runtime: {e}"))?;

    if let Some(params) = stdin_payload {
        if let Some(mut stdin) = child.stdin.take() {
            let payload = serde_json::to_vec(params).unwrap_or_default();
            std::thread::spawn(move || {
                let _ = stdin.write_all(&payload);
                // 离开作用域 drop 关闭 stdin，子进程读到 EOF
            });
        }
    }

    // 立刻取走管道开始采集：等退出期间必须排空，否则输出 >64KB 时子进程会阻塞
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let capped_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h_out = std::thread::spawn({
        let flag = capped_flag.clone();
        move || stdout_pipe.map(|r| read_capped(r, flag))
    });
    let h_err = std::thread::spawn({
        let flag = capped_flag.clone();
        move || stderr_pipe.map(|r| read_capped(r, flag))
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            // 输出超限 = 失控（finalize 只保留 8000 字符，超限内容本来就用不上）
            Ok(None) if capped_flag.load(std::sync::atomic::Ordering::Relaxed) => {
                let _ = child.kill();
                break None;
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("Failed to wait for process: {e}")),
        }
    };

    // 子进程退出或被 kill 后管道 EOF，采集线程正常很快返回
    let (_, out_buf) = h_out.join().ok().and_then(std::convert::identity).unwrap_or((false, Vec::new()));
    let (_, err_buf) = h_err.join().ok().and_then(std::convert::identity).unwrap_or((false, Vec::new()));

    // capped 优先：读端触顶关管道会让失控进程收到 SIGPIPE 迅速退出，
    // try_wait 可能先观察到这个「退出」——必须按失控处理而不是当正常退出
    if capped_flag.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(format!(
            "Execution aborted: output exceeded {}MB cap, process killed",
            OUTPUT_CAP >> 20
        ));
    }
    match status {
        Some(st) => Ok(Output {
            status: st,
            stdout: out_buf,
            stderr: err_buf,
        }),
        None => Err(format!(
            "Execution timed out ({}s), process killed",
            timeout.as_secs()
        )),
    }
}

/// 统一处理输出：非零退出返回错误，否则优先解析 stdout 为 JSON
fn finalize(out: Result<std::process::Output, String>) -> Result<serde_json::Value, String> {
    let output = out?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        let msg = if stderr.is_empty() { stdout } else { stderr };
        let msg = crate::registry::safe_trunc(&msg, 4000);
        return Err(format!("Execution failed (exit code {:?}): {msg}", output.status.code()));
    }

    let out = crate::registry::safe_trunc(&stdout, 8000);
    match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => Ok(v),
        Err(_) => Ok(serde_json::json!({ "stdout": out })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_run_with_limit_timeout_kills_process() {
        // 挂死进程必须在超时后返回 Err 而不是永远等待（1 秒超时，sleep 120 秒）
        let start = Instant::now();
        let result = run_with_limit(
            {
                let mut c = Command::new("/bin/sleep");
                c.arg("120");
                c
            },
            None,
            Duration::from_secs(1),
        );
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(10), "should return well before 120s sleep ends");
    }

    #[cfg(unix)]
    #[test]
    fn test_run_with_limit_output_cap_kills_runaway() {
        // cat /dev/zero 无限输出：采集触顶 8MB 后必须判定失控并杀掉进程
        //（用 cat 而非 dd：BSD dd 与 GNU dd 的 bs 后缀不兼容，跨平台测试要稳）
        let start = Instant::now();
        let result = run_with_limit(
            {
                let mut c = Command::new("/bin/cat");
                c.arg("/dev/zero");
                c
            },
            None,
            Duration::from_secs(30),
        );
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("output exceeded"));
        assert!(start.elapsed() < Duration::from_secs(15), "runaway must be killed promptly");
    }

    #[cfg(unix)]
    #[test]
    fn test_run_with_limit_stdin_passthrough() {
        // stdin 参数必须完整送达（含 EOF），子进程 cat 回显后正常退出
        let result = run_with_limit(
            {
                let c = Command::new("/bin/cat");
                c
            },
            Some(&serde_json::json!({ "hello": "世界" })),
            Duration::from_secs(10),
        )
        .expect("cat should succeed");
        assert_eq!(result.status.code(), Some(0));
        let echoed = String::from_utf8_lossy(&result.stdout);
        assert!(echoed.contains("hello"));
        assert!(echoed.contains("世界"));
    }

    #[cfg(unix)]
    #[test]
    fn test_write_private_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("bit_test_{}", uuid::Uuid::new_v4().simple()));
        write_private(&path, "secret-code").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(mode & 0o777, 0o600, "temp script must not be world-readable");
    }
}
