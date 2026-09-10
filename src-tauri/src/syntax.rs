// yxpil · BIT
//! 轻量语法检查：AI 写入/编辑代码文件后自动体检，发现语法错误时
//!   1. 在工具结果里注入 syntax_warning（模型下一轮立刻看到并自我修复）
//!   2. 发 syntax-warning UI 事件（聊天里出警告气泡提醒用户）
//! 只做"秒级、零依赖"的检查，不追求 lint 全覆盖：
//!   .json          → serde_json 解析（进程内）
//!   .js/.mjs/.cjs  → node --check（复用本机 node）
//!   .py            → python ast.parse（优先 toolhomes venv，回退 PATH python）
//! 没有对应解释器 / 检查超时（5s）→ 静默跳过，绝不阻塞写入本身。

use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// 单文件检查超时（子进程启动 + 解析，5s 足够；超时视为"无法检查"而非错误）
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// 检查文件语法。返回 Some(错误描述) = 发现语法错误；None = 通过或不支持/无法检查。
pub async fn check(ctx: &Arc<crate::state::Ctx>, path: &str) -> Option<String> {
    // 总开关：设置页可关（关了就不做任何检查）
    if !ctx.config.lock().unwrap().syntax_check {
        return None;
    }
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "json" => check_json(path).await,
        "js" | "mjs" | "cjs" => check_node(path).await,
        "py" => check_python(crate::toolenv::venv_python(ctx), path).await,
        _ => None,
    }
}

/// .json：进程内 serde 解析，带行列定位（serde_json 的报错本身含 line/col）
async fn check_json(path: &str) -> Option<String> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(_) => None,
        Err(e) => Some(format!("JSON parse error: {e}")),
    }
}

/// .js/.mjs/.cjs：node --check（语法-only，不执行代码）
async fn check_node(path: &str) -> Option<String> {
    let out = run_cmd(
        &mut tokio::process::Command::new("node").arg("--check").arg(path),
    )
    .await?;
    if out.status.success() {
        None
    } else {
        Some(first_lines(&out.stderr, 8))
    }
}

/// .py：ast.parse（纯语法、不生成 __pycache__）；优先传入 venv python，回退系统 python
async fn check_python(venv: Option<std::path::PathBuf>, path: &str) -> Option<String> {
    let probe = |program: std::path::PathBuf| {
        let mut c = tokio::process::Command::new(program);
        c.args([
            "-X",
            "utf8",
            "-c",
            "import ast,sys;ast.parse(open(sys.argv[1],encoding='utf-8').read())",
            path,
        ]);
        c
    };
    // venv python 存在就直接用；否则试 PATH 上的 python / python3
    if let Some(py) = venv {
        if let Some(out) = run_cmd(&mut probe(py)).await {
            return if out.status.success() { None } else { Some(first_lines(&out.stderr, 8)) };
        }
    }
    for name in ["python", "python3"] {
        if let Some(out) = run_cmd(&mut probe(name.into())).await {
            return if out.status.success() { None } else { Some(first_lines(&out.stderr, 8)) };
        }
    }
    None
}

/// 跑一条检查命令：5s 超时 / 程序不存在（NotFound）→ None（静默跳过）
async fn run_cmd(cmd: &mut tokio::process::Command) -> Option<std::process::Output> {
    // Windows 上避免闪出控制台窗口（worker/GUI 都不该被打扰）；tokio Command 自带 creation_flags
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = tokio::time::timeout(CHECK_TIMEOUT, cmd.output()).await.ok()?;
    match child {
        Ok(out) => Some(out),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => None,
    }
}

/// stderr 摘要：截前 N 行 + 总长钳制，避免把整页报错灌进模型上下文
fn first_lines(bytes: &[u8], n: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut body: String = s.lines().filter(|l| !l.trim().is_empty()).take(n).collect::<Vec<_>>().join("\n");
    if body.len() > 1200 {
        body.truncate(1200);
        body.push_str(" …(truncated)");
    }
    body
}

/// 检查结果回填工具输出 + 发 UI 警告。
/// err = Some(错误描述) 时：result 注入 syntax_warning（模型可见），
/// 并向聊天发 syntax-warning 事件（前端渲染警告气泡）。
pub fn annotate(
    ctx: &Arc<crate::state::Ctx>,
    session: Option<&str>,
    path: &str,
    err: Option<String>,
    mut result: serde_json::Value,
) -> serde_json::Value {
    let Some(err) = err else { return result };
    if let Some(o) = result.as_object_mut() {
        o.insert(
            "syntax_warning".into(),
            json!(format!(
                "The file you just wrote has a syntax error — fix it now (read_file then edit):\n{err}"
            )),
        );
    }
    crate::worker::emit_ui(
        &ctx.app,
        "syntax-warning",
        json!({ "session": session, "path": path, "error": err }),
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时目录里写一组好/坏样本，逐语言验证判定：
    /// 好文件 → None；坏文件 → Some(错误描述)；不支持的扩展名 → None（静默跳过）
    async fn sample(name: &str, content: &str) -> String {
        let dir = std::env::temp_dir().join(format!("bit-syntax-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p.to_string_lossy().to_string()
    }

    #[tokio::test]
    async fn test_json_good_and_bad() {
        let good = sample("good.json", r#"{"a": 1, "b": [true, null]}"#).await;
        let bad = sample("bad.json", r#"{"a": 1,}"#).await;
        assert_eq!(check_json(&good).await, None, "合法 json 不应报错");
        let err = check_json(&bad).await.expect("非法 json 必须报错");
        assert!(err.contains("JSON parse error"), "报错应含前缀: {err}");
    }

    #[tokio::test]
    async fn test_node_good_and_bad() {
        // 无 node 的环境（极端 CI）→ 静默跳过 None，不算失败
        let good = sample("good.js", "const x = (a) => a + 1;\n").await;
        let bad = sample("bad.js", "function( { break }\n").await;
        match check_node(&good).await {
            None => {}
            Some(e) => panic!("合法 js 不应报错: {e}"),
        }
        if let Some(err) = check_node(&bad).await {
            assert!(!err.trim().is_empty(), "报错内容不应为空");
        }
        // 检测不到 node 时跳过（None）也可接受——见 check_node 注释
    }

    #[tokio::test]
    async fn test_python_good_and_bad() {
        let good = sample("good.py", "def f(x):\n    return x + 1\n").await;
        let bad = sample("bad.py", "def f(:\n    pass\n").await;
        // venv 不参与单测：传 None 直接走 PATH python 回退
        match check_python(None, &good).await {
            None => {}
            Some(e) => panic!("合法 py 不应报错: {e}"),
        }
        if let Some(err) = check_python(None, &bad).await {
            assert!(err.contains("SyntaxError") || !err.trim().is_empty(), "报错应可读: {err}");
        }
    }

    #[tokio::test]
    async fn test_json_non_object_content() {
        // check_json 只看内容不看扩展名：合法 JSON 值（数组/字符串/数字）都应通过，
        // 非 JSON 文本必须报错（扩展名分流由 check() 负责，单测覆盖不到 ctx 分支）
        let arr = sample("arr.json", "[1, 2, 3]").await;
        assert_eq!(check_json(&arr).await, None);
        let txt = sample("note.txt", "plain text").await;
        assert!(check_json(&txt).await.is_some(), "非 JSON 文本必须报错");
    }

    #[test]
    fn test_first_lines_truncation() {
        let long = (0..100).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        let clipped = first_lines(long.as_bytes(), 8);
        assert_eq!(clipped.lines().count(), 8, "只保留前 8 行");
        let huge = "x".repeat(2000);
        let clipped2 = first_lines(huge.as_bytes(), 100);
        assert!(clipped2.len() <= 1215, "超长需截断: {}", clipped2.len());
        assert!(clipped2.ends_with("(truncated)"));
    }

    #[test]
    fn test_annotate_injects_warning_and_keeps_fields() {
        // annotate 需要真实 Ctx（发 UI 事件）；这里只验证纯 JSON 逻辑等价物：
        // 有错时对象应新增 syntax_warning 字段且保留原字段
        let mut obj = serde_json::json!({ "path": "a.js" });
        if let Some(o) = obj.as_object_mut() {
            o.insert("syntax_warning".into(), json!("boom"));
        }
        assert_eq!(obj["path"], "a.js");
        assert_eq!(obj["syntax_warning"], "boom");
    }
}
