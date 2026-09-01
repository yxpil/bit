use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 单个模型提供方（可同时配置多家，但每次仅激活一个）
#[derive(Serialize, Deserialize, Clone)]
pub struct Provider {
    pub id: String,
    /// 显示名称
    pub name: String,
    /// 协议：openai | gemini | claude
    pub protocol: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// 是否为当前激活项（全局仅一个为 true）
    pub active: bool,
}

impl Provider {
    /// 某协议的默认 base_url
    pub fn default_base_url(protocol: &str) -> &'static str {
        match protocol {
            "gemini" => "https://generativelanguage.googleapis.com",
            "claude" => "https://api.anthropic.com",
            _ => "https://api.openai.com/v1",
        }
    }
    /// 某协议的默认模型
    pub fn default_model(protocol: &str) -> &'static str {
        match protocol {
            "gemini" => "gemini-1.5-flash",
            "claude" => "claude-3-5-sonnet-latest",
            _ => "gpt-4o-mini",
        }
    }
}

/// AI 配置：多家提供方 + 当前激活项由各条目的 active 标记
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AiConfig {
    #[serde(default)]
    pub providers: Vec<Provider>,
    /// 思考强度：""=默认（不发送参数）/ low / medium / high
    #[serde(default)]
    pub reasoning_effort: String,
    /// 温度：None=默认（不发送参数），范围 0-2
    #[serde(default)]
    pub temperature: Option<f64>,
}

impl AiConfig {
    /// 当前激活的提供方（若无显式激活，退回第一条）
    pub fn active(&self) -> Option<&Provider> {
        self.providers
            .iter()
            .find(|p| p.active)
            .or_else(|| self.providers.first())
    }
    /// 是否已配置可用的激活项（有 key）
    pub fn is_configured(&self) -> bool {
        self.active().map(|p| !p.api_key.is_empty()).unwrap_or(false)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// 该条 assistant 消息触发的工具调用（用于前端可视化，模型请求时会被剥离）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRecord>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage { role: "user".into(), content: content.into(), tool_calls: Vec::new() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage { role: "assistant".into(), content: content.into(), tool_calls: Vec::new() }
    }
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage { role: "system".into(), content: content.into(), tool_calls: Vec::new() }
    }
}

/// 单次工具调用的可视化记录
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ToolCallRecord {
    pub tool: String,
    pub params: serde_json::Value,
    pub ok: bool,
    pub result: serde_json::Value,
}

/// 调用当前激活提供方进行一次对话（按协议分派）
pub async fn chat(ctx: &Arc<crate::state::Ctx>, messages: &[ChatMessage]) -> Result<String, String> {
    chat_with_images(ctx, messages, &[]).await
}

/// 把用户设置的思考强度 / 温度按协议注入请求体。
/// 默认档（空 / None）不发送任何参数，保持提供方自身默认，避免不支持参数的模型报错。
/// - OpenAI：reasoning_effort + temperature
/// - Claude：thinking.budget_tokens（low=2048/medium=8192/high=16384），max_tokens 自动放大；温度范围 0-1，开思考时忽略
/// - Gemini：generationConfig.thinkingBudget（low=1024/medium=8192/high=24576）+ temperature
fn apply_params(protocol: &str, body: &mut serde_json::Value, cfg: &AiConfig) {
    let effort = cfg.reasoning_effort.as_str();
    let temp = cfg.temperature;
    match protocol {
        "openai" => {
            if let Some(t) = temp {
                body["temperature"] = serde_json::json!(t);
            }
            if !effort.is_empty() {
                body["reasoning_effort"] = serde_json::json!(effort);
            }
        }
        "claude" => {
            if !effort.is_empty() {
                let budget = match effort {
                    "low" => 2048,
                    "high" => 16384,
                    _ => 8192,
                };
                let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(4096);
                // Claude 要求 max_tokens > budget_tokens
                body["max_tokens"] = serde_json::json!(max_tokens.max(budget + 4096));
                body["thinking"] =
                    serde_json::json!({ "type": "enabled", "budget_tokens": budget });
            } else if let Some(t) = temp {
                // Claude 温度范围 0-1（OpenAI 风格 0-2 折半映射）
                body["temperature"] = serde_json::json!(t / 2.0);
            }
        }
        "gemini" => {
            let mut gc = serde_json::Map::new();
            if let Some(t) = temp {
                gc.insert("temperature".to_string(), serde_json::json!(t));
            }
            if !effort.is_empty() {
                let budget = match effort {
                    "low" => 1024,
                    "high" => 24576,
                    _ => 8192,
                };
                gc.insert(
                    "thinkingConfig".to_string(),
                    serde_json::json!({ "thinkingBudget": budget }),
                );
            }
            if !gc.is_empty() {
                body["generationConfig"] = serde_json::Value::Object(gc);
            }
        }
        _ => {}
    }
}

/// 带图片（base64 data URL）的一次性对话：图片仅附加到最后一条 user 消息（多模态）
pub async fn chat_with_images(
    ctx: &Arc<crate::state::Ctx>,
    messages: &[ChatMessage],
    images: &[String],
) -> Result<String, String> {
    let provider = {
        let cfg = ctx.ai_config.lock().unwrap();
        cfg.active().cloned()
    };
    let p = provider.ok_or("未配置任何 AI 提供方，请先在「AI 设置」中添加并启用一个")?;
    if p.api_key.is_empty() {
        return Err(format!("提供方「{}」未填写 API Key", p.name));
    }
    let params = {
        let cfg = ctx.ai_config.lock().unwrap();
        cfg.clone()
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    match p.protocol.as_str() {
        "gemini" => chat_gemini(&client, &p, messages, images, &params).await,
        "claude" => chat_claude(&client, &p, messages, images, &params).await,
        _ => chat_openai(&client, &p, messages, images, &params).await,
    }
}

/// 流式对话：图片（base64 data URL）仅附加到最后一条 user 消息（多模态）。
/// 每收到一个文本增量就调用 `on_token`，最终返回完整文本。
/// 若提供方/网络不支持流式，则退回一次性请求并把整段作为单个 token 回调。
pub async fn chat_stream_with_images<F: FnMut(&str)>(
    ctx: &Arc<crate::state::Ctx>,
    messages: &[ChatMessage],
    images: &[String],
    mut on_token: F,
) -> Result<String, String> {
    let provider = {
        let cfg = ctx.ai_config.lock().unwrap();
        cfg.active().cloned()
    };
    let p = provider.ok_or("未配置任何 AI 提供方，请先在「AI 设置」中添加并启用一个")?;
    if p.api_key.is_empty() {
        return Err(format!("提供方「{}」未填写 API Key", p.name));
    }
    let params = {
        let cfg = ctx.ai_config.lock().unwrap();
        cfg.clone()
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;

    let res = match p.protocol.as_str() {
        "gemini" => stream_gemini(&client, &p, messages, images, &params, &mut on_token).await,
        "claude" => stream_claude(&client, &p, messages, images, &params, &mut on_token).await,
        _ => stream_openai(&client, &p, messages, images, &params, &mut on_token).await,
    };
    match res {
        Ok(full) => Ok(full),
        // 流式失败（部分服务不支持 SSE）时退回普通请求
        Err(_) => {
            let full = match p.protocol.as_str() {
                "gemini" => chat_gemini(&client, &p, messages, images, &params).await,
                "claude" => chat_claude(&client, &p, messages, images, &params).await,
                _ => chat_openai(&client, &p, messages, images, &params).await,
            }?;
            on_token(&full);
            Ok(full)
        }
    }
}

/// 逐行处理 SSE 流：对每个 `data:` 行调用 `handle`，返回 true 表示遇到结束标记
async fn read_sse<H: FnMut(&str) -> bool>(
    resp: reqwest::Response,
    mut handle: H,
) -> Result<(), String> {
    use futures_util::StreamExt;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        // 错误体常含中文，必须按字符边界截断
        return Err(format!("HTTP {status}: {}", crate::registry::safe_trunc(&text, 400)));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("流读取失败: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        // 以换行切分，保留最后不完整的一段
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf = buf[pos + 1..].to_string();
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if handle(data) {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

/// 找到最后一条 user 消息的下标（图片挂到它上面），无则返回 None
fn last_user_idx(messages: &[ChatMessage]) -> Option<usize> {
    messages.iter().rposition(|m| m.role == "user")
}

/// 从 base64 data URL 里拆出 (mime, 纯base64)。无前缀时默认 image/png。
fn split_data_url(s: &str) -> (String, String) {
    if let Some(rest) = s.strip_prefix("data:") {
        if let Some(comma) = rest.find(',') {
            let meta = &rest[..comma];
            let b64 = &rest[comma + 1..];
            let mime = meta.split(';').next().unwrap_or("image/png").to_string();
            return (mime, b64.to_string());
        }
    }
    ("image/png".to_string(), s.to_string())
}

/// 构造 OpenAI messages：把图片以 image_url 形式挂到最后一条 user 消息（多模态数组）
fn openai_messages(messages: &[ChatMessage], images: &[String]) -> Vec<serde_json::Value> {
    let target = if images.is_empty() { None } else { last_user_idx(messages) };
    messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if Some(i) == target {
                let mut parts: Vec<serde_json::Value> = Vec::new();
                if !m.content.is_empty() {
                    parts.push(serde_json::json!({ "type": "text", "text": m.content }));
                }
                for img in images {
                    // OpenAI 直接接受 data URL
                    let url = if img.starts_with("data:") { img.clone() } else { format!("data:image/png;base64,{img}") };
                    parts.push(serde_json::json!({ "type": "image_url", "image_url": { "url": url } }));
                }
                serde_json::json!({ "role": m.role, "content": parts })
            } else {
                serde_json::json!({ "role": m.role, "content": m.content })
            }
        })
        .collect()
}

/// 构造 Claude 请求：返回 (system 文本, messages)。图片以 image/base64 block 挂到最后一条 user。
fn claude_messages(messages: &[ChatMessage], images: &[String]) -> (String, Vec<serde_json::Value>) {
    let target = if images.is_empty() { None } else { last_user_idx(messages) };
    let mut system_txt = String::new();
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        match m.role.as_str() {
            "system" => {
                if !system_txt.is_empty() { system_txt.push_str("\n\n"); }
                system_txt.push_str(&m.content);
            }
            "assistant" => msgs.push(serde_json::json!({ "role": "assistant", "content": m.content })),
            _ => {
                if Some(i) == target {
                    let mut blocks: Vec<serde_json::Value> = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(serde_json::json!({ "type": "text", "text": m.content }));
                    }
                    for img in images {
                        let (mime, b64) = split_data_url(img);
                        blocks.push(serde_json::json!({
                            "type": "image",
                            "source": { "type": "base64", "media_type": mime, "data": b64 }
                        }));
                    }
                    msgs.push(serde_json::json!({ "role": "user", "content": blocks }));
                } else {
                    msgs.push(serde_json::json!({ "role": "user", "content": m.content }));
                }
            }
        }
    }
    (system_txt, msgs)
}

/// 构造 Gemini 请求：返回 (system 文本, contents)。图片以 inlineData 挂到最后一条 user。
fn gemini_contents(messages: &[ChatMessage], images: &[String]) -> (String, Vec<serde_json::Value>) {
    let target = if images.is_empty() { None } else { last_user_idx(messages) };
    let mut system_txt = String::new();
    let mut contents: Vec<serde_json::Value> = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        match m.role.as_str() {
            "system" => {
                if !system_txt.is_empty() { system_txt.push_str("\n\n"); }
                system_txt.push_str(&m.content);
            }
            "assistant" => contents.push(serde_json::json!({ "role": "model", "parts": [{ "text": m.content }] })),
            _ => {
                if Some(i) == target {
                    let mut parts: Vec<serde_json::Value> = Vec::new();
                    if !m.content.is_empty() {
                        parts.push(serde_json::json!({ "text": m.content }));
                    }
                    for img in images {
                        let (mime, b64) = split_data_url(img);
                        parts.push(serde_json::json!({ "inlineData": { "mimeType": mime, "data": b64 } }));
                    }
                    contents.push(serde_json::json!({ "role": "user", "parts": parts }));
                } else {
                    contents.push(serde_json::json!({ "role": "user", "parts": [{ "text": m.content }] }));
                }
            }
        }
    }
    (system_txt, contents)
}

/// OpenAI 流式：/chat/completions stream=true
async fn stream_openai<F: FnMut(&str)>(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
    on_token: &mut F,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", p.base_url.trim_end_matches('/'));
    let msgs = openai_messages(messages, images);
    let mut body = serde_json::json!({ "model": p.model, "messages": msgs, "stream": true });
    apply_params("openai", &mut body, params);
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", p.api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let mut full = String::new();
    read_sse(resp, |data| {
        if data == "[DONE]" {
            return true;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            if let Some(delta) = v.pointer("/choices/0/delta/content").and_then(|x| x.as_str()) {
                full.push_str(delta);
                on_token(delta);
            }
        }
        false
    })
    .await?;
    if full.is_empty() {
        return Err("流式无内容".into());
    }
    Ok(full)
}

/// Claude 流式：/v1/messages stream=true（content_block_delta）
async fn stream_claude<F: FnMut(&str)>(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
    on_token: &mut F,
) -> Result<String, String> {
    let (system_txt, msgs) = claude_messages(messages, images);
    let mut body = serde_json::json!({
        "model": p.model, "max_tokens": 4096, "messages": msgs, "stream": true
    });
    apply_params("claude", &mut body, params);
    if !system_txt.is_empty() {
        body["system"] = serde_json::json!(system_txt);
    }
    let url = format!("{}/v1/messages", p.base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("x-api-key", &p.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let mut full = String::new();
    read_sse(resp, |data| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            let t = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
            if t == "content_block_delta" {
                if let Some(delta) = v.pointer("/delta/text").and_then(|x| x.as_str()) {
                    full.push_str(delta);
                    on_token(delta);
                }
            } else if t == "message_stop" {
                return true;
            }
        }
        false
    })
    .await?;
    if full.is_empty() {
        return Err("流式无内容".into());
    }
    Ok(full)
}

/// Gemini 流式：streamGenerateContent?alt=sse
async fn stream_gemini<F: FnMut(&str)>(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
    on_token: &mut F,
) -> Result<String, String> {
    let (system_txt, contents) = gemini_contents(messages, images);
    let mut body = serde_json::json!({ "contents": contents });
    apply_params("gemini", &mut body, params);
    if !system_txt.is_empty() {
        body["systemInstruction"] = serde_json::json!({ "parts": [{ "text": system_txt }] });
    }
    let url = format!(
        "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
        p.base_url.trim_end_matches('/'),
        p.model
    );
    let resp = client
        .post(&url)
        .header("x-goog-api-key", &p.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let mut full = String::new();
    read_sse(resp, |data| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            if let Some(delta) = v.pointer("/candidates/0/content/parts/0/text").and_then(|x| x.as_str()) {
                full.push_str(delta);
                on_token(delta);
            }
        }
        false
    })
    .await?;
    if full.is_empty() {
        return Err("流式无内容".into());
    }
    Ok(full)
}

/// OpenAI 兼容协议：/chat/completions
async fn chat_openai(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", p.base_url.trim_end_matches('/'));
    // 只发送 role/content（图片挂到最后一条 user），剥离本地可视化用的 tool_calls 字段
    let msgs = openai_messages(messages, images);
    let mut body = serde_json::json!({ "model": p.model, "messages": msgs });
    apply_params("openai", &mut body, params);
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", p.api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let value = read_json_resp(resp).await?;
    value
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "响应中缺少 content".to_string())
}

/// Google Gemini 原生协议：generateContent
async fn chat_gemini(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
) -> Result<String, String> {
    // system 合并进 systemInstruction，其余转为 contents（role: user/model），图片挂到最后一条 user
    let (system_txt, contents) = gemini_contents(messages, images);
    let mut body = serde_json::json!({ "contents": contents });
    apply_params("gemini", &mut body, params);
    if !system_txt.is_empty() {
        body["systemInstruction"] = serde_json::json!({ "parts": [{ "text": system_txt }] });
    }
    let url = format!(
        "{}/v1beta/models/{}:generateContent",
        p.base_url.trim_end_matches('/'),
        p.model
    );
    let resp = client
        .post(&url)
        .header("x-goog-api-key", &p.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let value = read_json_resp(resp).await?;
    value
        .pointer("/candidates/0/content/parts/0/text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "响应中缺少 text".to_string())
}

/// Anthropic Claude 原生协议：/v1/messages
async fn chat_claude(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
) -> Result<String, String> {
    // system 单独抽出，其余作为 messages（role: user/assistant），图片挂到最后一条 user
    let (system_txt, msgs) = claude_messages(messages, images);
    let mut body = serde_json::json!({
        "model": p.model,
        "max_tokens": 4096,
        "messages": msgs,
    });
    apply_params("claude", &mut body, params);
    if !system_txt.is_empty() {
        body["system"] = serde_json::json!(system_txt);
    }
    let url = format!("{}/v1/messages", p.base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("x-api-key", &p.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let value = read_json_resp(resp).await?;
    value
        .pointer("/content/0/text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "响应中缺少 text".to_string())
}

/// 统一处理 HTTP 响应：非 2xx 返回错误，否则解析 JSON
async fn read_json_resp(resp: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let short = if text.len() > 400 { text[..400].to_string() } else { text };
        return Err(format!("HTTP {status}: {short}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("响应解析失败: {e}"))
}

/// 生成当前已注册工具的清单（供 AI 了解可用工具）
pub fn tools_manifest(ctx: &Arc<crate::state::Ctx>) -> serde_json::Value {
    let tools = ctx.tools.lock().unwrap().clone();
    serde_json::json!(tools
        .iter()
        .filter(|t| t.enabled) // 暂停的工具不告知 AI
        .map(|t| serde_json::json!({
            "id": t.id,
            "name": t.name,
            "description": t.description,
            "parameters": t.parameters,
        }))
        .collect::<Vec<_>>())
}

/// 组装系统提示词：内置能力 + 动态工具清单 + 记忆 + 技能
pub fn system_prompt(ctx: &Arc<crate::state::Ctx>) -> String {
    let memories = ctx.memories.lock().unwrap();
    let mem_lines: Vec<String> = memories
        .iter()
        .rev()
        .take(12)
        .map(|m| format!("- [{}] {}", m.kind, m.content))
        .collect();
    let skills = ctx.skills.lock().unwrap();
    let skill_lines: Vec<String> = skills
        .iter()
        .rev()
        .take(12)
        .map(|s| format!("- {}: {}", s.name, s.summary))
        .collect();
    drop(skills);

    let goal_lines: Vec<String> = ctx
        .goals
        .lock()
        .unwrap()
        .iter()
        .filter(|g| g.status == "active")
        .map(|g| format!("- [{}] {} {}", g.id, g.title, g.detail))
        .collect();
    let todo_lines: Vec<String> = ctx
        .todos
        .lock()
        .unwrap()
        .iter()
        .filter(|t| t.status != "completed")
        .map(|t| {
            format!(
                "- [{}]{} {} ({})",
                t.id,
                t.goal_id.as_deref().map(|g| format!(" 目标{g}")).unwrap_or_default(),
                t.content,
                t.status
            )
        })
        .collect();

    // 本机可用运行时清单（仅列出「已启用」的，AI 只能用这些真实存在的解释器写代码）
    let runtime_lines: Vec<String> = ctx
        .runtimes
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.enabled)
        .map(|r| {
            let how = match r.mode.as_str() {
                "compile" => "编译型：写完整源码，BIT 自动编译再运行",
                "exec" => "可执行：直接调用该程序",
                _ => "解释型：写一段脚本直接运行",
            };
            format!(
                "- id=\"{}\"（{}，语言 {}，{}，版本 {}）",
                r.id, r.name, r.lang, how, r.version
            )
        })
        .collect();

    format!(
        "你是 BIT，一个可以自我扩展的 AI 助手。你能调用工具、并通过写代码为自己增加新工具。\n\
        \n\
        ## 操作手册：如何调用工具（务必遵守）\n\
        当你需要动手做事（执行命令、读写文件、制定计划、扩展自己、沉淀/查找技能）时，\
        在回答里【单独一行】输出一个 JSON 数组，数组每个元素形如 {{\"tool\":\"工具名\",\"params\":{{...}}}}。\n\
        这一行必须是纯 JSON，前后不要加解释文字、不要用代码块包裹；系统会执行后把结果回给你，你再据此继续。\n\
        严禁自创标记语法（如 <xxx_function_call>）、严禁输出不带方括号的裸对象、严禁把多个调用拆成多行——\
        多个调用必须放在同一个数组里：[{{...}},{{...}}]。不需要动手时正常用自然语言回答即可。\n\
        单个工具调用示例：[{{\"tool\":\"shell\",\"params\":{{\"command\":\"echo hi\"}}}}]\n\
        \n\
        ## 七个出厂内置工具（编号即「工具N」，在「已注册工具」清单中）\n\
        - 工具1 · shell：执行命令行（Windows PowerShell 语法，路径用 C:\\ 形式）。参数 {{\"command\":string,\"cwd\":string(可选)}}\n\
        - 工具2 · write_file：写入/覆盖文件（文档编辑）。参数 {{\"path\":string,\"content\":string}}\n\
        - 工具3 · plan：制定计划，登记目标与分步待办。参数 {{\"goal\":string,\"steps\":[string]}}\n\
        - 工具4 · edit：增量补丁改文件，精确替换。参数 {{\"path\":string,\"old_string\":string,\"new_string\":string,\"replace_all\":bool(可选)}}\n\
        - 工具5 · add_tool：给自己增加工具——用本机某解释器把一段代码沉淀为常驻工具。参数 {{\"name\":string,\"description\":string,\"runtime\":string,\"code\":string}}\n\
        - 工具6 · skill：技能库读写。写入技能 {{\"action\":\"save\",\"name\":string,\"summary\":string}}（同名覆盖）；搜索技能 {{\"action\":\"search\",\"query\":string}}（query 留空返回全部）\n\
        - 工具7 · sub_agent：派生子智能体——新建独立会话执行自包含的大任务（调研/批量整理/写大文件），阻塞等待并返回其最终结论，子会话保留在侧栏可查看全过程。参数 {{\"task\":string,\"title\":string(可选)}}。注意 task 必须自包含：子智能体看不到当前对话历史，请把背景、目标、验收标准写全\n\
        写 SKILL、搜 SKILL 都用【工具6 · skill】：save 是自己写一条技能，search 是搜索已有技能。示例：\n\
        - 写 SKILL：[{{\"tool\":\"skill\",\"params\":{{\"action\":\"save\",\"name\":\"批量重命名\",\"summary\":\"用 shell 遍历目录并 mv 重命名文件的步骤…\"}}}}]\n\
        - 搜 SKILL：[{{\"tool\":\"skill\",\"params\":{{\"action\":\"search\",\"query\":\"重命名\"}}}}]\n\
        \n\
        ## 自我扩展的扩展动作（同样作为 tool 调用）\n\
        - run_script：用本机解释器临时执行一段代码（不落地）。参数 {{\"runtime\":string,\"code\":string,\"params\":object}}\n\
        - add_memory {{\"content\":string,\"kind\":string}}（沉淀记忆）\n\
        - goal_create / goal_update / todo_add / todo_update / todo_write（管理目标与待办）\n\
        \n\
        ## 主动沉淀（没有自动按钮，全靠你自己调用工具）\n\
        对话里出现值得长期记住的事实/偏好 → 主动用 add_memory 记下来；\n\
        摸索出一套可复用的做法 → 主动用【工具6 · skill】的 save 沉淀为技能；\n\
        开始一个类似任务前 → 先用【工具6 · skill】的 search 查有没有现成技能可复用。\n\
        \n\
        ## 如何用代码武装自己（重要）\n\
        你只能使用【本机可用解释器】里真实列出的 id（这些是本机实际探测到、当前已启用的解释器；被暂停的不会出现，别猜别的语言）。\n\
        想临时算一算/查一查用 run_script；想沉淀成以后可复用的工具用 add_tool（工具5）。\n\
        脚本通讯约定：代码从【stdin】读取一段 JSON（即 params），把结果【打印到 stdout】，建议输出一行 JSON。示例：\n\
        - Node.js: `const p=JSON.parse(require('fs').readFileSync(0,'utf8')||'{{}}');console.log(JSON.stringify({{sum:(p.a||0)+(p.b||0)}}))`\n\
        - Python: `import sys,json; p=json.loads(sys.stdin.read() or '{{}}'); print(json.dumps({{'sum':p.get('a',0)+p.get('b',0)}}))`\n\
        编译型（java/rust/go/c/cpp…）同样从 stdin 读、stdout 写，写完整源码，BIT 自动编译再运行。\n\
        ## 本机可用解释器（只能用这里的 id）\n{}\n\
        ## 当前目标\n{}\n\
        ## 当前待办\n{}\n\
        ## 已注册工具\n{}\n\
        ## 记忆\n{}\n\
        ## 技能\n{}\n\
        需要调用能力时输出 JSON 数组，每个元素 {{\"tool\":string,\"params\":object}}，单独一行，不要包裹其他文字之外的内容。",
        if runtime_lines.is_empty() { "（未探测到任何解释器，可在「工具」页点刷新探测）".to_string() } else { runtime_lines.join("\n") },
        if goal_lines.is_empty() { "（暂无）".to_string() } else { goal_lines.join("\n") },
        if todo_lines.is_empty() { "（暂无）".to_string() } else { todo_lines.join("\n") },
        serde_json::to_string_pretty(&tools_manifest(ctx)).unwrap_or_default(),
        if mem_lines.is_empty() { "（空）".to_string() } else { mem_lines.join("\n") },
        if skill_lines.is_empty() { "（空）".to_string() } else { skill_lines.join("\n") },
    )
}
