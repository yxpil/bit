use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// MCP（Model Context Protocol）接入：Streamable HTTP 传输（JSON-RPC 2.0）。
/// 遵循 MCP 规范：initialize 握手 → notifications/initialized → tools/list / tools/call。
/// 同时兼容返回 application/json 或 text/event-stream 的服务器。

/// 已接入的 MCP 服务器（持久化到 mcp_servers.json）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct McpServer {
    pub id: String,
    /// 服务器自报名称（serverInfo.name）
    pub name: String,
    /// Streamable HTTP 端点 URL
    pub url: String,
    /// 服务器版本（serverInfo.version）
    #[serde(default)]
    pub version: String,
    /// 协议版本（initialize 协商结果）
    #[serde(default)]
    pub protocol: String,
    /// initialize 返回的会话 id（Mcp-Session-Id），后续请求必须携带
    #[serde(default)]
    pub session: String,
    /// 暂停/继续：false 时该服务器全部工具拒绝调用
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub connected_at: String,
}

fn default_enabled() -> bool {
    true
}

/// 发现结果：某个端口上识别到 MCP 服务器
#[derive(Serialize, Clone)]
pub struct Discovered {
    pub url: String,
    pub name: String,
    pub version: String,
    pub protocol: String,
    pub session: String,
}

#[derive(Deserialize)]
struct RpcResp {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    result: Option<serde_json::Value>,
    error: Option<RpcErr>,
}

#[derive(Deserialize)]
struct RpcErr {
    message: Option<String>,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

/// 解析响应体：application/json 直接解析；text/event-stream 提取 data: 行后解析
fn parse_body(ct: &str, body: &str) -> Result<RpcResp, String> {
    if ct.contains("text/event-stream") {
        // SSE：取最后一行 data: {...} 作为 JSON-RPC 响应
        let mut last: Option<&str> = None;
        for line in body.lines() {
            if let Some(d) = line.strip_prefix("data:") {
                let d = d.trim();
                if d.contains("\"result\"") || d.contains("\"error\"") {
                    last = Some(d);
                }
            }
        }
        let raw = last.ok_or_else(|| format!("SSE 响应中没有 JSON-RPC 结果: {body}"))?;
        serde_json::from_str(raw).map_err(|e| format!("解析 SSE 响应失败: {e}"))
    } else {
        serde_json::from_str(body).map_err(|e| format!("解析响应失败: {e}"))
    }
}

/// 发送一条 JSON-RPC 请求，返回 (result, Mcp-Session-Id)
async fn rpc(
    http: &reqwest::Client,
    url: &str,
    session: Option<&str>,
    method: &str,
    params: serde_json::Value,
) -> Result<(serde_json::Value, Option<String>), String> {
    let mut req = http
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }));
    if let Some(sid) = session {
        req = req.header("Mcp-Session-Id", sid);
    }
    let resp = req.send().await.map_err(|e| format!("请求 {url} 失败: {e}"))?;
    let sid = resp
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
    if status.is_client_error() || status.is_server_error() {
        return Err(format!("HTTP {status}: {}", truncate(&body, 200)));
    }
    if method == "notifications/initialized" {
        return Ok((serde_json::Value::Null, sid));
    }
    let parsed: RpcResp = parse_body(&ct, &body)?;
    if let Some(err) = parsed.error {
        return Err(format!(
            "MCP 错误 {}: {}",
            method,
            err.message.unwrap_or_default()
        ));
    }
    Ok((parsed.result.unwrap_or(serde_json::Value::Null), sid))
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() > n {
        // 回退到 UTF-8 字符边界，避免多字节字符（如中文错误信息）切片 panic
        let mut end = n;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    } else {
        s.to_string()
    }
}

/// initialize 握手：返回 (serverInfo, protocolVersion, session id)
pub async fn initialize(
    http: &reqwest::Client,
    url: &str,
) -> Result<(String, String, String, String), String> {
    let (result, sid) = rpc(
        http,
        url,
        None,
        "initialize",
        serde_json::json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "BIT", "version": env!("CARGO_PKG_VERSION") }
        }),
    )
    .await?;
    let info = result.get("serverInfo");
    let name = info
        .and_then(|i| i.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("MCP Server")
        .to_string();
    let version = info
        .and_then(|i| i.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let protocol = result
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // 发送 initialized 通知（规范要求），失败不阻塞
    let _ = rpc(
        http,
        url,
        sid.as_deref(),
        "notifications/initialized",
        serde_json::json!({}),
    )
    .await;
    Ok((name, version, protocol, sid.unwrap_or_default()))
}

/// 发现单个端口：TCP 连通则尝试 MCP 握手，识别成功返回 Discovered
pub async fn probe_port(host: &str, port: u16) -> Option<Discovered> {
    let addr = format!("{host}:{port}");
    // 先快速 TCP 探测，未开放直接跳过
    if tokio::time::timeout(
        std::time::Duration::from_millis(600),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .ok()?
    .is_err()
    {
        return None;
    }
    let url = format!("http://{addr}");
    let http = client().ok()?;
    let (name, version, protocol, session) = initialize(&http, &url).await.ok()?;
    Some(Discovered { url, name, version, protocol, session })
}

/// 扫描端口范围（并发，最多 2048 个端口），返回识别为 MCP 服务器的列表
pub async fn discover(host: &str, start: u16, end: u16) -> Result<Vec<Discovered>, String> {
    let host = if host.trim().is_empty() { "127.0.0.1" } else { host.trim() };
    if end < start {
        return Err("端口范围无效（结束 < 开始）".into());
    }
    if (end as u32 - start as u32) >= 2048 {
        return Err("一次最多扫描 2048 个端口".into());
    }
    let ports: Vec<u16> = (start..=end).collect();
    let tasks: Vec<_> = ports
        .into_iter()
        .map(|p| {
            let host = host.to_string();
            tauri::async_runtime::spawn(async move { probe_port(&host, p).await })
        })
        .collect();
    let mut found = Vec::new();
    for t in tasks {
        if let Ok(Some(d)) = t.await {
            found.push(d);
        }
    }
    Ok(found)
}

/// MCP 工具定义（tools/list 条目）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: serde_json::Value,
}

/// 拉取服务器的工具清单（自动跟随 nextCursor 分页，避免工具多时只拿到第一页）
pub async fn list_tools(server: &McpServer) -> Result<Vec<McpTool>, String> {
    let http = client()?;
    let mut tools: Vec<McpTool> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..100 {
        // 分页上限保护：100 页足以覆盖任何真实服务器
        let mut params = serde_json::json!({});
        if let Some(c) = &cursor {
            params["cursor"] = serde_json::json!(c);
        }
        let (result, _) =
            rpc(&http, &server.url, Some(&server.session), "tools/list", params).await?;
        let arr = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for t in arr {
            if let Some(name) = t.get("name").and_then(|v| v.as_str()) {
                tools.push(McpTool {
                    name: name.to_string(),
                    description: t
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or(serde_json::json!({"type":"object","properties":{}})),
                });
            }
        }
        cursor = result
            .get("nextCursor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        if cursor.is_none() {
            break;
        }
    }
    Ok(tools)
}

/// 调用服务器上的工具，返回拼接后的文本结果
pub async fn call_tool(
    server: &McpServer,
    tool: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let http = client()?;
    let (result, _) = rpc(
        &http,
        &server.url,
        Some(&server.session),
        "tools/call",
        serde_json::json!({ "name": tool, "arguments": args }),
    )
    .await?;
    // result.content: [{type:"text", text:"..."}]，拼接全部文本
    let mut text = String::new();
    if let Some(items) = result.get("content").and_then(|v| v.as_array()) {
        for item in items {
            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
        }
    }
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if is_error {
        return Err(if text.is_empty() { "MCP 工具返回错误".to_string() } else { text });
    }
    if text.is_empty() {
        return Ok(result);
    }
    Ok(serde_json::json!({ "text": text, "raw": result }))
}

/// 全局：查找已接入服务器
pub fn find<'a>(ctx: &Arc<crate::state::Ctx>, id: &str) -> Option<McpServer> {
    ctx.mcp.lock().unwrap().iter().find(|s| s.id == id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    // ── 纯函数 ──

    #[test]
    fn test_truncate_utf8_boundary() {
        // 中文（3 字节/字）在第 200 字节附近切断不得 panic
        let s = "错误信息".repeat(100);
        let t = truncate(&s, 200);
        assert!(t.ends_with('…'));
        assert!(t.len() <= 201);
    }

    #[test]
    fn test_safe_trunc_shared() {
        // registry::safe_trunc：ASCII 不受影响、中文不 panic、短串原样返回
        assert_eq!(crate::registry::safe_trunc("hello", 10), "hello");
        assert_eq!(crate::registry::safe_trunc("hello", 5), "hello");
        let cn = crate::registry::safe_trunc("中文输出测试", 7);
        assert!(cn.ends_with('…'));
        assert_eq!(cn.chars().count(), 3); // 2 个汉字 + 省略号
    }

    #[test]
    fn test_parse_body_json() {
        let r = parse_body("application/json", r#"{"jsonrpc":"2.0","id":1,"result":{"ok":1}}"#).unwrap();
        assert_eq!(r.result.unwrap()["ok"], 1);
    }

    #[test]
    fn test_parse_body_sse() {
        let body = "event: message\r\ndata: {\"journal\":1}\r\n\r\nevent: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\r\n\r\n";
        let r = parse_body("text/event-stream", body).unwrap();
        assert_eq!(r.result.unwrap()["tools"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_parse_body_rpc_error() {
        let r = parse_body("application/json", r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method 404"}}"#).unwrap();
        assert_eq!(r.error.unwrap().message.unwrap(), "method 404");
    }

    // ── 集成：本地 mock MCP 服务器（Streamable HTTP，JSON 与 SSE 混合响应） ──

    /// 极简异步 HTTP 服务器：按 JSON-RPC method 路由，逐请求读取、写完即关
    async fn spawn_mock_mcp() -> (String, Arc<StdMutex<HashMap<String, usize>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits: Arc<StdMutex<HashMap<String, usize>>> = Arc::new(StdMutex::new(HashMap::new()));
        let hits_cloned = hits.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let hits = hits_cloned.clone();
                tauri::async_runtime::spawn(async move {
                    // 读请求头
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let head_end = loop {
                        let Ok(n) = tokio::io::AsyncReadExt::read(&mut sock, &mut chunk).await else { return };
                        if n == 0 { return; }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break p + 4;
                        }
                        if buf.len() > 65536 { return; }
                    };
                    // 解析 Content-Length 并读 body
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let clen: usize = head
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    while buf.len() < head_end + clen {
                        let Ok(n) = tokio::io::AsyncReadExt::read(&mut sock, &mut chunk).await else { return };
                        if n == 0 { break; }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
                    let req: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let params = req.get("params").cloned().unwrap_or(serde_json::Value::Null);
                    *hits.lock().unwrap().entry(method.clone()).or_insert(0) += 1;

                    let (ct, payload, session) = match method.as_str() {
                        "initialize" => (
                            "application/json",
                            json!({
                                "jsonrpc": "2.0", "id": 1,
                                "result": {
                                    "protocolVersion": "2025-03-26",
                                    "serverInfo": { "name": "Mock MCP", "version": "1.2.3" }
                                }
                            })
                            .to_string(),
                            Some("sess-mock-1".to_string()),
                        ),
                        "notifications/initialized" => ("text/plain", String::new(), None),
                        "tools/list" => {
                            // 带分页：第一次给 cursor，第二次返回剩余工具
                            let page2 = params.get("cursor").is_some();
                            let tools = if page2 {
                                json!([{ "name": "tool_b", "description": "第二个工具",
                                         "inputSchema": { "type": "object", "properties": {} } }])
                            } else {
                                json!([{ "name": "tool_a", "description": "第一个工具",
                                         "inputSchema": { "type": "object", "properties": { "x": { "type": "number" } } } }])
                            };
                            let mut result = json!({ "tools": tools });
                            if !page2 {
                                result["nextCursor"] = json!("cursor-page-2");
                            }
                            (
                                "text/event-stream",
                                format!(
                                    "event: message\r\ndata: {}\r\n\r\n",
                                    json!({ "jsonrpc": "2.0", "id": 1, "result": result })
                                ),
                                None,
                            )
                        }
                        "tools/call" => (
                            "application/json",
                            json!({
                                "jsonrpc": "2.0", "id": 1,
                                "result": { "content": [
                                    { "type": "text", "text": "第一行结果" },
                                    { "type": "text", "text": "second line" }
                                ], "isError": false }
                            })
                            .to_string(),
                            None,
                        ),
                        _ => (
                            "application/json",
                            json!({ "jsonrpc": "2.0", "id": 1,
                                    "error": { "code": -32601, "message": format!("未知方法 {method}") } })
                                .to_string(),
                            None,
                        ),
                    };
                    let mut resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n", payload.len());
                    if let Some(sid) = &session {
                        resp.push_str(&format!("Mcp-Session-Id: {sid}\r\n"));
                    }
                    resp.push_str("\r\n");
                    resp.push_str(&payload);
                    let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
                    let _ = tokio::io::AsyncWriteExt::shutdown(&mut sock).await;
                });
            }
        });
        (format!("http://{addr}"), hits)
    }

    #[tokio::test]
    async fn test_initialize_and_full_flow() {
        let (url, hits) = spawn_mock_mcp().await;
        let http = client().unwrap();

        // 1. 握手：解析 serverInfo + 会话 id
        let (name, version, protocol, session) = initialize(&http, &url).await.unwrap();
        assert_eq!(name, "Mock MCP");
        assert_eq!(version, "1.2.3");
        assert_eq!(protocol, "2025-03-26");
        assert_eq!(session, "sess-mock-1");

        // 2. 拉取工具清单：自动跟随 nextCursor 分页，两页共 2 个工具
        let server = McpServer {
            id: "t1".into(),
            name: name.clone(),
            url: url.clone(),
            version,
            protocol,
            session,
            enabled: true,
            connected_at: "now".into(),
        };
        let tools = list_tools(&server).await.unwrap();
        assert_eq!(tools.len(), 2, "分页工具应全部拉取: {tools:?}");
        assert_eq!(tools[0].name, "tool_a");
        assert_eq!(tools[0].input_schema["properties"]["x"]["type"], "number");
        assert_eq!(tools[1].name, "tool_b");

        // 3. 调用工具：SSE 响应、多段 text 内容拼接
        let out = call_tool(&server, "tool_a", json!({ "x": 42 })).await.unwrap();
        assert_eq!(out["text"], "第一行结果\nsecond line");
        assert!(out.get("raw").is_some());

        // 4. 规范流程核对：initialized 通知已发出
        let h = hits.lock().unwrap();
        assert!(h.get("initialize").copied().unwrap_or(0) >= 1);
        assert!(h.get("notifications/initialized").copied().unwrap_or(0) >= 1);
        assert!(h.get("tools/list").copied().unwrap_or(0) >= 2, "分页应请求两次");
        assert!(h.get("tools/call").copied().unwrap_or(0) >= 1);
    }

    #[tokio::test]
    async fn test_initialize_rejects_non_mcp_server() {
        // 普通 HTTP 服务（无 JSON-RPC）不应被识别为 MCP 服务器
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                tauri::async_runtime::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
                    let body = "<html>hello</html>";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
                });
            }
        });
        let url = format!("http://{addr}");
        let http = client().unwrap();
        assert!(initialize(&http, &url).await.is_err(), "HTML 服务不应通过 MCP 握手");
    }
}
