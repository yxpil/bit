use rhai::Engine;

/// 在 Rhai 沙盒中执行 AI 编写的插件脚本。
/// 脚本通过 `params` 读取调用参数，最后一行表达式作为返回值。
/// 可用内置函数：http_get(url)、now()、to_json(x)、len(x)
pub fn run(code: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let mut engine = Engine::new();
    engine.set_max_expr_depths(64, 64);
    engine.set_max_operations(500_000);

    engine.register_fn("http_get", |url: &str| -> String {
        // 在独立线程中执行阻塞 HTTP，避免卡住异步运行时
        let url = url.to_string();
        std::thread::spawn(move || match reqwest::blocking::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().unwrap_or_default();
                // safe_trunc 按 UTF-8 边界截断：直接 body[..4000] 遇到多字节字符中缝会 panic
                let body = crate::registry::safe_trunc(&body, 4000);
                format!("{{\"status\":{status},\"body\":{}}}", serde_json::Value::String(body))
            }
            Err(e) => format!("ERROR: {e}"),
        })
        .join()
        .unwrap_or_else(|_| "ERROR: http thread panicked".into())
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
