use rhai::Engine;
use std::time::{Duration, Instant};

/// 单次脚本执行的墙钟总预算。max_operations 只数 Rhai 操作，http_get 阻塞等待不计数，
/// 循环里大量网络调用可远超操作数预期——这里按墙钟统一收口（registry 外层 30s 兜底）。
const SCRIPT_TIME_BUDGET: Duration = Duration::from_secs(25);

/// 在 Rhai 沙盒中执行 AI 编写的插件脚本。
/// 脚本通过 `params` 读取调用参数，最后一行表达式作为返回值。
/// 可用内置函数：http_get(url)、now()、to_json(x)、len(x)
pub fn run(code: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let mut engine = Engine::new();
    engine.set_max_expr_depths(64, 64);
    engine.set_max_operations(500_000);

    // 墙钟预算：on_progress 到期中止 eval；http_get 入口同样检查（阻塞期间 on_progress 不触发）
    let deadline = Instant::now() + SCRIPT_TIME_BUDGET;
    engine.on_progress(move |_| {
        if Instant::now() >= deadline { Some(rhai::Dynamic::UNIT) } else { None }
    });

    engine.register_fn("http_get", move |url: &str| -> String {
        let now = Instant::now();
        if now >= deadline {
            return "ERROR: script time budget exhausted".into();
        }
        // 单次调用上限取「剩余预算」与 30s 的较小值，保证总时限不被网络等待击穿
        let left = (deadline - now).min(Duration::from_secs(30));
        let url = url.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let msg = match reqwest::blocking::Client::new().get(&url).timeout(left).send() {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.text().unwrap_or_default();
                    // safe_trunc 按 UTF-8 边界截断：直接 body[..4000] 遇到多字节字符中缝会 panic
                    let body = crate::registry::safe_trunc(&body, 4000);
                    format!("{{\"status\":{status},\"body\":{}}}", serde_json::Value::String(body))
                }
                Err(e) => format!("ERROR: {e}"),
            };
            let _ = tx.send(msg);
        });
        // 旧实现 join() 会被挂死的连接永久卡住；recv 超时即放弃（网络线程最多再活 left 秒）
        rx.recv_timeout(left + Duration::from_secs(2))
            .unwrap_or_else(|_| "ERROR: http thread timed out".into())
    });

    engine.register_fn("now", || chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());

    engine.register_fn("to_json", |v: rhai::Dynamic| -> String {
        match rhai::serde::from_dynamic::<serde_json::Value>(&v) {
            Ok(val) => val.to_string(),
            Err(e) => format!("ERROR: {e}"),
        }
    });

    let mut scope = rhai::Scope::new();
    let params_dyn = rhai::serde::to_dynamic(&params).map_err(|e| format!("参数转换失败: {e}"))?;
    scope.push("params", params_dyn);

    let result: rhai::Dynamic = engine
        .eval_with_scope(&mut scope, code)
        .map_err(|e| format!("脚本执行失败: {e}"))?;

    match rhai::serde::from_dynamic::<serde_json::Value>(&result) {
        Ok(v) => Ok(v),
        Err(_) => Ok(serde_json::Value::String(result.to_string())),
    }
}
