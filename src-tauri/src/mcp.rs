use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// MCP（Model Context Protocol）接入：Streamable HTTP 传输（JSON-RPC 2.0）。
/// 遵循 MCP 规范：initialize 握手 → notifications/initialized → tools/list / tools/call。
/// 同时兼容返回 application/json 或 text/event-stream 的服务器。

/// 已接入的 MCP 服务器（持久化到 mcp_servers.json）
#[derive(Serialize, Deserialize, Clone)]
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
    if s.len() > n { format!("{}…", &s[..n]) } else { s.to_string() }
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
#[derive(Serialize, Deserialize, Clone)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: serde_json::Value,
}

/// 拉取服务器的工具清单
pub async fn list_tools(server: &McpServer) -> Result<Vec<McpTool>, String> {
    let http = client()?;
    let (result, _) = rpc(&http, &server.url, Some(&server.session), "tools/list", serde_json::json!({})).await?;
    let arr = result
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let tools: Vec<McpTool> = arr
        .into_iter()
        .filter_map(|t| {
            Some(McpTool {
                name: t.get("name")?.as_str()?.to_string(),
                description: t
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                input_schema: t.get("inputSchema").cloned().unwrap_or(serde_json::json!({"type":"object","properties":{}})),
            })
        })
        .collect();
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
