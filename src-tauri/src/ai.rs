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
    /// 该条 assistant 消息的思考过程（前端折叠展示，模型请求时会被剥离）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage { role: "user".into(), content: content.into(), tool_calls: Vec::new(), thinking: None }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage { role: "assistant".into(), content: content.into(), tool_calls: Vec::new(), thinking: None }
    }
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage { role: "system".into(), content: content.into(), tool_calls: Vec::new(), thinking: None }
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
    chat_with_images(ctx, messages, &[]).await.map(|(s, _)| s)
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

/// 带图片（base64 data URL）的一次性对话：图片仅附加到最后一条 user 消息（多模态）。
/// 返回 (完整文本, token 用量)
pub async fn chat_with_images(
    ctx: &Arc<crate::state::Ctx>,
    messages: &[ChatMessage],
    images: &[String],
) -> Result<(String, TokenUsage), String> {
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

/// 流式被调用方中止的哨兵错误（中断会话时立即断开 SSE 读取，不回退非流式）
pub const STREAM_STOP: &str = "__BIT_STREAM_STOP__";

/// 流式 token 种类：Text = 正文增量，Think = 思考过程增量（reasoning/thinking）
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TokenKind {
    Text,
    Think,
}

/// 流式对话：on_token 回调返回 false 表示调用方要求立即停止（如会话中断）。
/// 返回 (完整文本, token 用量)
pub async fn chat_stream_with_images<F: FnMut(TokenKind, &str) -> bool>(
    ctx: &Arc<crate::state::Ctx>,
    messages: &[ChatMessage],
    images: &[String],
    mut on_token: F,
) -> Result<(String, TokenUsage), String> {
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
        Ok((full, usage)) => Ok((full, usage)),
        // 调用方主动停止（中断）：不回退非流式，直接上抛哨兵
        Err(e) if e == STREAM_STOP => Err(e),
        // 流式失败（部分服务不支持 SSE）时退回一次性请求并把整段作为单个 token 回调
        Err(_) => {
            let (full, usage) = match p.protocol.as_str() {
                "gemini" => chat_gemini(&client, &p, messages, images, &params).await,
                "claude" => chat_claude(&client, &p, messages, images, &params).await,
                _ => chat_openai(&client, &p, messages, images, &params).await,
            }?;
            on_token(TokenKind::Text, &full);
            Ok((full, usage))
        }
    }
}

/// 喂入新到达的 SSE 字节，返回其中已完整的行；尾部不完整字节（可能是半个多字节字符）保留在 buf
fn drain_complete_lines(buf: &mut Vec<u8>, chunk: &[u8]) -> Vec<String> {
    buf.extend_from_slice(chunk);
    let mut lines = Vec::new();
    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        let line = String::from_utf8_lossy(&buf[..pos]).trim().to_string();
        buf.drain(..=pos);
        lines.push(line);
    }
    lines
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
    // 字节缓冲：多字节字符可能被 TCP 分块从中间截断，
    // 必须按完整行再解码——逐 chunk 转 String 会把跨块字符打成 U+FFFD
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("流读取失败: {e}"))?;
        for line in drain_complete_lines(&mut buf, &chunk) {
            if let Some(data) = line.strip_prefix("data:") {
                if handle(data.trim()) {
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
async fn stream_openai<F: FnMut(TokenKind, &str) -> bool>(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
    on_token: &mut F,
) -> Result<(String, TokenUsage), String> {
    let url = format!("{}/chat/completions", p.base_url.trim_end_matches('/'));
    let msgs = openai_messages(messages, images);
    // include_usage：最后一个 chunk 携带 usage（含缓存命中统计），不影响正文增量
    let mut body = serde_json::json!({
        "model": p.model, "messages": msgs, "stream": true,
        "stream_options": {"include_usage": true},
    });
    apply_params("openai", &mut body, params);
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", p.api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let mut full = String::new();
    let mut usage = TokenUsage::default();
    let mut stopped = false;
    let mut saw_done = false;
    let mut finish = String::new();
    read_sse(resp, |data| {
        if data == "[DONE]" {
            saw_done = true;
            return true;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            if v.get("usage").is_some() {
                usage = usage_from_openai(&v);
            }
            // 追踪 finish_reason：length = 达到 max_tokens 被截断，需显式告知
            if let Some(f) = v.pointer("/choices/0/finish_reason").and_then(|x| x.as_str()) {
                if !f.is_empty() {
                    finish = f.to_string();
                }
            }
            // 思考过程增量（DeepSeek R1 reasoning_content / OpenRouter reasoning）：先于正文，不混入回复
            if let Some(think) = v
                .pointer("/choices/0/delta/reasoning_content")
                .or_else(|| v.pointer("/choices/0/delta/reasoning"))
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
            {
                if !on_token(TokenKind::Think, &think) {
                    stopped = true;
                    return true;
                }
            }
            if let Some(delta) = v
                .pointer("/choices/0/delta/content")
                .and_then(|x| x.as_str())
                .map(String::from)
                // 模糊回退：delta 缺失改用 message / 旧版 text / output_text 等变体（网关转换常见）
                .or_else(|| fuzzy_text(&v))
            {
                full.push_str(&delta);
                // 回调返回 false = 调用方中止（中断会话）：停止读取 SSE
                if !on_token(TokenKind::Text, &delta) {
                    stopped = true;
                    return true;
                }
            }
        }
        false
    })
    .await?;
    if stopped {
        return Err(STREAM_STOP.into());
    }
    if full.is_empty() {
        return Err("流式无内容".into());
    }
    // 上游未发 [DONE] = 连接异常中断（网络波动/代理断开）：半截内容不能当完整回复
    if !saw_done {
        return Err("连接中断：流式响应未正常结束（网络波动或代理断开），请重试".into());
    }
    // 达到输出上限：在正文尾部显式标注，避免“话说一半”看起来像 bug
    let full = if finish_truncated(&finish) {
        format!("{full}\n\n（回复因达到最大输出长度被截断，可回复“继续”）")
    } else {
        full
    };
    Ok((full, usage))
}

/// Claude 流式：/v1/messages stream=true（content_block_delta）
async fn stream_claude<F: FnMut(TokenKind, &str) -> bool>(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
    on_token: &mut F,
) -> Result<(String, TokenUsage), String> {
    let (system_txt, mut msgs) = claude_messages(messages, images);
    let mut body = serde_json::json!({
        "model": p.model, "max_tokens": 8192, "messages": msgs, "stream": true
    });
    apply_params("claude", &mut body, params);
    claude_apply_cache(&mut body, &system_txt, &mut msgs);
    body["messages"] = serde_json::json!(msgs);
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
    let mut usage = TokenUsage::default();
    let mut stopped = false;
    // input/cache 用量在 message_start，output 用量在 message_delta
    read_sse(resp, |data| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            let t = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
            if t == "message_start" {
                if let Some(u) = v.pointer("/message/usage") {
                    usage.prompt_tokens = u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                    usage.cache_read_tokens = u.get("cache_read_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                    usage.cache_write_tokens = u.get("cache_creation_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                }
            } else if t == "message_delta" {
                if let Some(n) = v.pointer("/usage/output_tokens").and_then(|x| x.as_u64()) {
                    usage.completion_tokens = n;
                }
            } else if t == "content_block_delta" {
                // thinking_delta：思考过程增量（开思考时先于正文块），不混入回复
                if v.pointer("/delta/type").and_then(|x| x.as_str()) == Some("thinking_delta") {
                    if let Some(think) = v
                        .pointer("/delta/thinking")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                    {
                        if !on_token(TokenKind::Think, &think) {
                            stopped = true;
                            return true;
                        }
                    }
                    return false;
                }
                if let Some(delta) = v
                    .pointer("/delta/text")
                    .and_then(|x| x.as_str())
                    .map(String::from)
                    // 模糊回退：delta.content 变体 / 非标准块结构
                    .or_else(|| fuzzy_text(&v))
                {
                    full.push_str(&delta);
                    if !on_token(TokenKind::Text, &delta) {
                        stopped = true;
                        return true;
                    }
                }
            } else if t == "message_stop" {
                return true;
            }
        }
        false
    })
    .await?;
    if stopped {
        return Err(STREAM_STOP.into());
    }
    if full.is_empty() {
        return Err("流式无内容".into());
    }
    Ok((full, usage))
}

/// Gemini 流式：streamGenerateContent?alt=sse
async fn stream_gemini<F: FnMut(TokenKind, &str) -> bool>(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
    on_token: &mut F,
) -> Result<(String, TokenUsage), String> {
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
    let mut usage = TokenUsage::default();
    let mut stopped = false;
    read_sse(resp, |data| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            // 每块都带 usageMetadata（渐进累计，取最后一块即最终值）
            if v.get("usageMetadata").is_some() {
                usage = usage_from_gemini(&v);
            }
            if let Some(parts) = v.pointer("/candidates/0/content/parts").and_then(|x| x.as_array()) {
                // thought=true 的 part 是思考过程（先于正文），其余为正文
                for part in parts {
                    let Some(text) = part.get("text").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) else {
                        continue;
                    };
                    if part.get("thought").and_then(|x| x.as_bool()).unwrap_or(false) {
                        if !on_token(TokenKind::Think, text) {
                            stopped = true;
                            return true;
                        }
                    } else {
                        full.push_str(text);
                        if !on_token(TokenKind::Text, text) {
                            stopped = true;
                            return true;
                        }
                    }
                }
            } else if let Some(delta) = fuzzy_text(&v) {
                // 模糊回退：非标准结构（文本被拆进多 part 等变体）
                full.push_str(&delta);
                if !on_token(TokenKind::Text, &delta) {
                    stopped = true;
                    return true;
                }
            }
        }
        false
    })
    .await?;
    if stopped {
        return Err(STREAM_STOP.into());
    }
    if full.is_empty() {
        return Err("流式无内容".into());
    }
    Ok((full, usage))
}

/// OpenAI 兼容协议：/chat/completions
async fn chat_openai(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
) -> Result<(String, TokenUsage), String> {
    let url = format!("{}/chat/completions", p.base_url.trim_end_matches('/'));
    // 只发送 role/content（图片挂到最后一条 user），剥离本地可视化用的 tool_calls 字段
    let msgs = openai_messages(messages, images);
    let mut body = serde_json::json!({ "model": p.model, "messages": msgs, "max_tokens": 8192 });
    apply_params("openai", &mut body, params);
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", p.api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let value = read_json_resp(resp).await?;
    let mut content = value
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        // 模糊回退：content 为内容块数组 / 旧版 choices[0].text / Responses API output_text 等
        .or_else(|| fuzzy_text(&value))
        .ok_or_else(|| "响应中缺少 content".to_string())?;
    // finish_reason=length：输出被截断，显式标注避免“话说一半”像 bug
    if value
        .pointer("/choices/0/finish_reason")
        .and_then(|v| v.as_str())
        .is_some_and(finish_truncated)
    {
        content.push_str("\n\n（回复因达到最大输出长度被截断，可回复“继续”）");
    }
    Ok((content, usage_from_openai(&value)))
}

/// Google Gemini 原生协议：generateContent
async fn chat_gemini(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
) -> Result<(String, TokenUsage), String> {
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
    let text = value
        .pointer("/candidates/0/content/parts/0/text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        // 模糊回退：多 part 拼接 / 变体结构
        .or_else(|| fuzzy_text(&value))
        .ok_or_else(|| "响应中缺少 text".to_string())?;
    Ok((text, usage_from_gemini(&value)))
}

/// Claude 提示词缓存：system 与最后一条消息打 cache_control 断点。
/// 请求前缀稳定时命中 prompt cache（省 token、显著降首字延迟）；
/// 因此系统提示词内的动态段落（记忆/目标/待办）只在变化时才使缓存失效。
fn claude_apply_cache(
    body: &mut serde_json::Value,
    system_txt: &str,
    msgs: &mut [serde_json::Value],
) {
    if !system_txt.is_empty() {
        body["system"] = serde_json::json!([
            {"type": "text", "text": system_txt, "cache_control": {"type": "ephemeral"}}
        ]);
    }
    if let Some(last) = msgs.last_mut() {
        let content = last.get("content").cloned().unwrap_or(serde_json::json!(""));
        last["content"] = match content {
            serde_json::Value::String(s) => serde_json::json!([
                {"type": "text", "text": s, "cache_control": {"type": "ephemeral"}}
            ]),
            serde_json::Value::Array(mut arr) => {
                if let Some(b) = arr.last_mut() {
                    b["cache_control"] = serde_json::json!({"type": "ephemeral"});
                }
                serde_json::Value::Array(arr)
            }
            other => other,
        };
    }
}

async fn chat_claude(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
) -> Result<(String, TokenUsage), String> {
    // system 单独抽出，其余作为 messages（role: user/assistant），图片挂到最后一条 user
    let (system_txt, mut msgs) = claude_messages(messages, images);
    let mut body = serde_json::json!({
        "model": p.model,
        "max_tokens": 8192,
        "messages": msgs,
    });
    apply_params("claude", &mut body, params);
    claude_apply_cache(&mut body, &system_txt, &mut msgs);
    body["messages"] = serde_json::json!(msgs);
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
    let text = value
        .pointer("/content/0/text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        // 模糊回退：content 为纯字符串 / 多 text 块拼接 / 变体结构
        .or_else(|| fuzzy_text(&value))
        .ok_or_else(|| "响应中缺少 text".to_string())?;
    Ok((text, usage_from_claude(&value)))
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

// ── 模糊格式识别：动态适配不严格遵循协议的响应（第三方网关 / 新模型常出现变体）──

/// 视为文本载体的键（小写精确匹配），命中即取值；对象/数组则继续深入
const FUZZY_TEXT_KEYS: &[&str] = &[
    "text",
    "content",
    "delta",
    "message",
    "output_text",
    "completion",
    "answer",
    "response",
];

/// 明确跳过的键：推理过程不是正文；error / 工具调用 / 元数据结构不能当回复
const FUZZY_SKIP_KEYS: &[&str] = &[
    "error",
    "errors",
    "reasoning_content",
    "reasoning",
    "thinking",
    "thought",
    "signature",
    "type",
    "role",
    "id",
    "model",
    "name",
    "url",
    "data",
    "index",
    "object",
    "created",
    "usage",
    "finish_reason",
    "stop_reason",
    "annotations",
    "citations",
    "refusal",
    "logprobs",
    "system_fingerprint",
    "function",
    "function_call",
    "functioncall",
    "tool_calls",
    "tool_use",
    "arguments",
    "input",
    "args",
    "parameters",
];

/// 递归模糊提取正文文本：字符串挂在文本键下才算命中；数组按序拼接（多 part / 多内容块）
fn fuzzy_text(v: &serde_json::Value) -> Option<String> {
    fuzzy_walk(v, 0).filter(|s| !s.trim().is_empty())
}

/// OpenAI 正文取值：字符串直取；content 为内容块数组（网关转换常见，[{type:"text",text},...]）时拼接各块文本
fn content_str(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            let mut out = String::new();
            for b in arr {
                if let Some(t) = b.get("text").and_then(|x| x.as_str()) {
                    out.push_str(t);
                }
            }
            if out.is_empty() { None } else { Some(out) }
        }
        _ => None,
    }
}

fn fuzzy_walk(v: &serde_json::Value, depth: usize) -> Option<String> {
    if depth > 16 {
        return None;
    }
    match v {
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> =
                arr.iter().filter_map(|x| fuzzy_walk(x, depth + 1)).collect();
            if parts.is_empty() { None } else { Some(parts.concat()) }
        }
        serde_json::Value::Object(m) => {
            // 第一轮：只看文本键（保持字段优先级，命中即返回）
            for (k, val) in m {
                let kl = k.to_lowercase();
                if FUZZY_SKIP_KEYS.contains(&kl.as_str()) {
                    continue;
                }
                if FUZZY_TEXT_KEYS.contains(&kl.as_str()) {
                    match val {
                        serde_json::Value::String(s) if !s.is_empty() => return Some(s.clone()),
                        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                            if let Some(t) = fuzzy_walk(val, depth + 1) {
                                return Some(t);
                            }
                        }
                        _ => {}
                    }
                }
            }
            // 第二轮：文本键下没有 → 继续深入容器键（choices / candidates / parts 等结构）
            for (k, val) in m {
                let kl = k.to_lowercase();
                if FUZZY_SKIP_KEYS.contains(&kl.as_str()) {
                    continue;
                }
                if matches!(val, serde_json::Value::Object(_) | serde_json::Value::Array(_)) {
                    if let Some(t) = fuzzy_walk(val, depth + 1) {
                        return Some(t);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// finish_reason 模糊归一化：不同端点大小写 / 别名不一（length / max_tokens / MAX_TOKENS 风格）
fn finish_truncated(f: &str) -> bool {
    matches!(
        f.to_lowercase().as_str(),
        "length"
            | "max_tokens"
            | "max_output_tokens"
            | "max_token_limit"
            | "token_limit"
            | "length_limit"
            | "context_length_exceeded"
    )
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

/// 组装系统提示词（文本约定模式）：内置能力 + 动态工具清单 + 记忆 + 技能
pub fn system_prompt(ctx: &Arc<crate::state::Ctx>, session: Option<&str>) -> String {
    system_prompt_mode(ctx, session, false)
}

/// 组装系统提示词（原生函数调用模式）：不教文本格式，指导模型直接发起 function call
pub fn system_prompt_native(ctx: &Arc<crate::state::Ctx>, session: Option<&str>) -> String {
    system_prompt_mode(ctx, session, true)
}

/// session：当前会话 —— 目标/待办只注入「本会话创建的」或「全局（无会话归属）」的
fn system_prompt_mode(ctx: &Arc<crate::state::Ctx>, session: Option<&str>, native: bool) -> String {
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
        .take(100)
        .map(|s| format!("- {}", s.name))
        .collect();
    drop(skills);

    // 会话隔离：仅注入当前会话创建的或全局（session_id 为空）的目标/待办
    let in_session = |sid: &Option<String>| -> bool {
        match (sid.as_deref(), session) {
            (None, _) => true,
            (Some(s), Some(cur)) => s == cur,
            (Some(_), None) => false,
        }
    };
    let goal_lines: Vec<String> = ctx
        .goals
        .lock()
        .unwrap()
        .iter()
        .filter(|g| g.status == "active" && in_session(&g.session_id))
        .map(|g| format!("- [{}] {} {}", g.id, g.title, g.detail))
        .collect();
    let todo_lines: Vec<String> = ctx
        .todos
        .lock()
        .unwrap()
        .iter()
        .filter(|t| t.status != "completed" && in_session(&t.session_id))
        .map(|t| {
            format!(
                "- [{}]{} {} ({})",
                t.id,
                t.goal_id.as_deref().map(|g| format!(" goal:{g}")).unwrap_or_default(),
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
                "compile" => "compiled: write full source, BIT compiles then runs",
                "exec" => "executable: invoked directly",
                _ => "interpreted: write a script and run it",
            };
            format!(
                "- id=\"{}\" ({}, lang {}, {}, version {})",
                r.id, r.name, r.lang, how, r.version
            )
        })
        .collect();

    // shell 工具说明按平台区分：Windows 用 PowerShell，macOS/Linux 用 POSIX shell
    let shell_syntax = if cfg!(windows) {
        "run command lines (PowerShell syntax on Windows; use C:\\ style paths)"
    } else {
        "run command lines (POSIX shell syntax; use / style paths, e.g. /Users/xxx and /home/xxx)"
    };

    // 操作手册 / skill 示例 / 收尾句：文本约定与原生函数调用两种模式各自一份
    // 提示词默认英文（各模型兼容性最好），但要求模型始终以用户的语言回复
    let (manual, skill_examples, closing) = if native {
        (
            "## How to call tools (native function calling)\n\
            You are in native function-calling mode: when you need to act, issue function calls directly (multiple parallel calls in one turn are allowed). \
            NEVER announce an action and then stop — if your reply says you are going to do something, the same turn MUST contain the actual tool call. \
            The system executes each call and returns its result to you as a tool message; keep reasoning or calling more tools based on the results. \
            When everything is done, output the final answer in natural language (do NOT output any call-format explanation or JSON call arrays in your reply).",
            "The SKILL list in this prompt shows names only. When a skill name looks relevant to the current task, fetch its full content first via Tool 6 · skill with action=search and query=that name, then follow it. action=save writes a skill (same name overwrites), action=search finds existing skills.",
            "When no more tool calls are needed, just output the final answer in natural language.",
        )
    } else {
        (
            "## How to call tools (follow strictly)\n\
            When you need to act (run commands, read/write files, make plans, extend yourself, save/find skills), output a JSON array on its own single line in your reply; each element looks like {{\"tool\":\"tool_name\",\"params\":{{...}}}}.\n\
            That line must be pure JSON — no explanatory text before or after, not wrapped in code fences. The system executes it and returns the results to you, then you continue.\n\
            Never invent marker syntax (like <xxx_function_call>), never output a bare object without the square brackets, never split multiple calls into multiple lines — \
            multiple calls must stay in ONE array: [{{...}},{{...}}]. When no action is needed, just answer in natural language.\n\
            Single call example: [{{\"tool\":\"shell\",\"params\":{{\"command\":\"echo hi\"}}}}]",
            "The SKILL list in this prompt shows names only. When a skill name looks relevant to the current task, fetch its full content first via Tool 6 · skill with action=search and query=that name, then follow it. save writes a skill, search finds existing skills. Examples:\n\
            - save a skill: [{{\"tool\":\"skill\",\"params\":{{\"action\":\"save\",\"name\":\"batch-rename\",\"summary\":\"use shell to walk the directory and mv-rename files…\"}}}}]\n\
            - search a skill: [{{\"tool\":\"skill\",\"params\":{{\"action\":\"search\",\"query\":\"rename\"}}}}]",
            "When you need to call a capability, output a JSON array, each element {{\"tool\":string,\"params\":object}}, on its own line, with nothing else wrapped around it.",
        )
    };

    format!(
        "You are BIT, a self-extending AI assistant. You can call tools, and write code to add new tools for yourself.\n\
        Always reply in the user's language (e.g. reply in Chinese when the user writes Chinese).\n\
        You are a local-first agent: every tool call executes on the user's own machine and all data stays on their device. \
        Your underlying model may be hosted by a remote API provider, but never present yourself as a cloud service — \
        if asked about your nature, answer honestly: a local agent running on this device, with a model served remotely.\n\
        \n\
        {manual}\n\
        \n\
        ## Factory built-in tools (the numbers map to the Registered tools list below)\n\
        - Tool 1 · shell: {shell_syntax}. Params {{\"command\":string,\"cwd\":string(optional)}}\n\
        - Tool 2 · write_file: create/overwrite a file (document editing). Params {{\"path\":string,\"content\":string}}\n\
        - Tool 3 · plan: make a plan; register a goal with step todos. Params {{\"goal\":string,\"steps\":[string]}}\n\
        - Tool 4 · edit: patch a file with exact string replacement. Params {{\"path\":string,\"old_string\":string,\"new_string\":string,\"replace_all\":bool(optional)}}\n\
        - Tool 5 · add_tool: add a tool for yourself — persist a piece of code with a local interpreter as a resident tool. Params {{\"name\":string,\"description\":string,\"runtime\":string,\"code\":string}}. Re-registering the same name overwrites your own interpreter/script tool in place (you may rewrite the same-name tool to fix your earlier mistakes); system/remote tools cannot be overwritten\n\
        - Tool 6 · skill: read/write the skill library. Save a skill {{\"action\":\"save\",\"name\":string,\"summary\":string}} (same name overwrites); search skills {{\"action\":\"search\",\"query\":string}} (empty query returns all)\n\
        - Tool 7 · sub_agent: spawn a sub-agent — it runs a self-contained big task (research / bulk processing / writing large files) in a separate session, blocks until done, and returns its final conclusion verbatim into this conversation (no file-location convention needed; just continue from the returned content). The sub-session stays in the sidebar for full review. Params {{\"task\":string,\"title\":string(optional)}}. The task must be self-contained: the sub-agent cannot see this conversation, so spell out background, goal and acceptance criteria\n\
        - Tool 8 · send_file: deliver an existing file to the user — a clickable file card appears in the chat, like sending a file (reports/HTML/images/data files etc.). Params {{\"path\":string,\"note\":string(optional, one-line note)}}\n\
        - Tool 9 · delete_tool: delete a tool you created via add_tool (interpreter/script tools only; built-in/remote/MCP tools cannot be deleted). Params {{\"name\":string}}\n\
        - Tool 10 · view_image: look at a local image — the image is injected into your next request, so vision models (GPT/Gemini/Claude/deepseek-vision etc.) can actually see it. Params {{\"path\":string,\"note\":string(optional, what to focus on)}}\n\
        - Tool 11 · truncate_history: truncate this session's history, keeping only the most recent `keep` messages (default 12). Use proactively when history grows long and early content is no longer valuable. Params {{\"keep\":integer(optional)}}\n\
        - Tool 12 · compact_history: compact this session — replace all earlier history with a summary you write (the last 2 messages are kept as-is). Put all key conclusions, decisions, unfinished work and next steps into `summary`. Params {{\"summary\":string}}\n\
        {skill_examples}\n\
        \n\
        ## Extension actions (also issued as tool calls)\n\
        - run_script: run a piece of code temporarily with a local interpreter (not persisted). Params {{\"runtime\":string,\"code\":string,\"params\":object}}\n\
        - add_memory {{\"content\":string,\"kind\":string}} (store a memory)\n\
        - goal_create / goal_update / todo_add / todo_update / todo_write (manage goals and todos)\n\
        \n\
        ## Proactive knowledge capture (no automatic buttons — call the tools yourself)\n\
        When the conversation reveals a fact or preference worth remembering long-term → call add_memory proactively;\n\
        When you work out a reusable procedure → save it as a skill with Tool 6 · skill (action=save);\n\
        Before starting a similar task → first search for an existing skill with Tool 6 · skill (action=search).\n\
        \n\
        ## Arming yourself with code (important)\n\
        Only use ids actually listed under Local interpreters (these are the interpreters really detected and currently enabled on this machine; paused ones will not appear — do not guess other languages).\n\
        Use run_script for one-off calculations/lookups; use add_tool (Tool 5) to persist a reusable tool.\n\
        Script I/O contract: your code reads one JSON from stdin (the params) and prints the result to stdout, preferably a single line of JSON. Examples:\n\
        - Node.js: `const p=JSON.parse(require('fs').readFileSync(0,'utf8')||'{{}}');console.log(JSON.stringify({{sum:(p.a||0)+(p.b||0)}}))`\n\
        - Python: `import sys,json; p=json.loads(sys.stdin.read() or '{{}}'); print(json.dumps({{'sum':p.get('a',0)+p.get('b',0)}}))`\n\
        Compiled languages (java/rust/go/c/cpp…) follow the same stdin/stdout contract with full source; BIT compiles then runs.\n\
        ## Local interpreters (only ids listed here are usable)\n{}\n\
        ## Current goals\n{}\n\
        ## Current todos\n{}\n\
        ## Registered tools\n{}\n\
        ## Memories\n{}\n\
        ## Skills\n{}\n\
        {closing}",
        if runtime_lines.is_empty() { "(no interpreter detected — click Refresh on the Tools page)".to_string() } else { runtime_lines.join("\n") },
        if goal_lines.is_empty() { "(none)".to_string() } else { goal_lines.join("\n") },
        if todo_lines.is_empty() { "(none)".to_string() } else { todo_lines.join("\n") },
        serde_json::to_string_pretty(&tools_manifest(ctx)).unwrap_or_default(),
        if mem_lines.is_empty() { "(empty)".to_string() } else { mem_lines.join("\n") },
        if skill_lines.is_empty() { "(empty)".to_string() } else { skill_lines.join("\n") },
        manual = manual,
        skill_examples = skill_examples,
        closing = closing,
    )
}

// ============ 原生工具调用（function calling）：OpenAI / Claude / Gemini ============

/// 原生协议返回的单个工具调用
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// 单个工具的执行结果
#[derive(Clone, Debug)]
pub struct ToolResult {
    pub ok: bool,
    pub value: serde_json::Value,
}

/// 一轮工具交换（模型发起的调用 + 系统执行结果），原生模式下按协议转成回传历史
#[derive(Clone, Debug)]
pub struct ToolExchange {
    pub calls: Vec<NativeToolCall>,
    pub results: Vec<ToolResult>,
}

/// 单次请求的 token 用量（含缓存命中信息，用于统计提示词缓存命中率）
#[derive(Default, Clone, Debug, Serialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub completion_tokens: u64,
}

impl TokenUsage {
    fn is_empty(&self) -> bool {
        self.prompt_tokens == 0 && self.completion_tokens == 0
    }
}

/// openai 协议 usage（兼容 deepseek 的 prompt_cache_hit_tokens 字段）
fn usage_from_openai(v: &serde_json::Value) -> TokenUsage {
    let u = v.get("usage");
    TokenUsage {
        prompt_tokens: u.and_then(|x| x.get("prompt_tokens")).and_then(|x| x.as_u64()).unwrap_or(0),
        cache_read_tokens: u
            .and_then(|x| x.pointer("/prompt_tokens_details/cached_tokens"))
            .and_then(|x| x.as_u64())
            .or_else(|| u.and_then(|x| x.get("prompt_cache_hit_tokens")).and_then(|x| x.as_u64()))
            .unwrap_or(0),
        cache_write_tokens: 0,
        completion_tokens: u.and_then(|x| x.get("completion_tokens")).and_then(|x| x.as_u64()).unwrap_or(0),
    }
}

/// claude 协议 usage（cache_read/cache_creation 为缓存命中/写入 token）
fn usage_from_claude(v: &serde_json::Value) -> TokenUsage {
    let u = v.get("usage");
    TokenUsage {
        prompt_tokens: u.and_then(|x| x.get("input_tokens")).and_then(|x| x.as_u64()).unwrap_or(0),
        cache_read_tokens: u.and_then(|x| x.get("cache_read_input_tokens")).and_then(|x| x.as_u64()).unwrap_or(0),
        cache_write_tokens: u.and_then(|x| x.get("cache_creation_input_tokens")).and_then(|x| x.as_u64()).unwrap_or(0),
        completion_tokens: u.and_then(|x| x.get("output_tokens")).and_then(|x| x.as_u64()).unwrap_or(0),
    }
}

/// gemini 协议 usageMetadata
fn usage_from_gemini(v: &serde_json::Value) -> TokenUsage {
    let u = v.get("usageMetadata");
    TokenUsage {
        prompt_tokens: u.and_then(|x| x.get("promptTokenCount")).and_then(|x| x.as_u64()).unwrap_or(0),
        cache_read_tokens: u.and_then(|x| x.get("cachedContentTokenCount")).and_then(|x| x.as_u64()).unwrap_or(0),
        cache_write_tokens: 0,
        completion_tokens: u
            .and_then(|x| x.get("candidatesTokenCount"))
            .and_then(|x| x.as_u64())
            .or_else(|| u.and_then(|x| x.get("totalTokenCount")).and_then(|x| x.as_u64()))
            .unwrap_or(0),
    }
}

/// 原生一轮的结果：文本 + 工具调用（可同时存在）
#[derive(Debug)]
pub struct NativeRound {
    pub content: String,
    /// 思考过程（reasoning/thinking），可能为空
    pub thinking: String,
    pub calls: Vec<NativeToolCall>,
    pub usage: TokenUsage,
}

/// 原生调用错误：Unsupported 表示端点不支持 tools 参数（应降级文本约定）
#[derive(Debug)]
#[allow(dead_code)] // Unsupported 携带的原始报错仅用于排查，降级是静默自动的
pub enum NativeErr {
    Unsupported(String),
    Other(String),
}

/// 组装原生工具清单：已注册且启用的工具 + execute_tool_call 直接处理的扩展动作。
/// 中立格式（name/description/parameters），发送前按协议转换。
pub fn native_tool_defs(ctx: &Arc<crate::state::Ctx>) -> Vec<serde_json::Value> {
    let mut defs: Vec<serde_json::Value> = {
        let tools = ctx.tools.lock().unwrap();
        tools
            .iter()
            .filter(|t| t.enabled)
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect()
    };
    let extra: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "run_script",
            "用本机解释器临时执行一段代码（不落地成常驻工具）。代码从 stdin 读 params JSON，结果打印到 stdout",
            serde_json::json!({"type":"object","properties":{"runtime":{"type":"string","description":"本机可用解释器 id"},"code":{"type":"string","description":"完整代码"},"params":{"type":"object","description":"传给代码的参数"}},"required":["runtime","code"]}),
        ),
        (
            "write_tool",
            "用本机解释器把一段代码沉淀为常驻工具；同名工具若为你自建则原位覆盖更新（可借此修正有误的实现）",
            serde_json::json!({"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"runtime":{"type":"string"},"code":{"type":"string"}},"required":["name","description","runtime","code"]}),
        ),
        (
            "add_memory",
            "沉淀一条长期记忆",
            serde_json::json!({"type":"object","properties":{"content":{"type":"string"},"kind":{"type":"string","description":"如 preference/fact"}},"required":["content"]}),
        ),
        (
            "add_skill",
            "沉淀一条可复用技能",
            serde_json::json!({"type":"object","properties":{"name":{"type":"string"},"summary":{"type":"string"}},"required":["name","summary"]}),
        ),
        (
            "goal_create",
            "创建目标",
            serde_json::json!({"type":"object","properties":{"title":{"type":"string"},"detail":{"type":"string"}},"required":["title"]}),
        ),
        (
            "goal_update",
            "更新目标状态（active/done/archived 等）",
            serde_json::json!({"type":"object","properties":{"id":{"type":"string"},"status":{"type":"string"}},"required":["id","status"]}),
        ),
        (
            "todo_add",
            "添加待办",
            serde_json::json!({"type":"object","properties":{"content":{"type":"string"},"goal_id":{"type":"string"}},"required":["content"]}),
        ),
        (
            "todo_update",
            "更新待办状态（pending/doing/completed）",
            serde_json::json!({"type":"object","properties":{"id":{"type":"string"},"status":{"type":"string"}},"required":["id","status"]}),
        ),
        (
            "todo_write",
            "整体重写待办清单",
            serde_json::json!({"type":"object","properties":{"items":{"type":"array","items":{"type":"string"}},"goal_id":{"type":"string"}},"required":["items"]}),
        ),
    ];
    for (name, desc, params) in extra {
        // 注册表里已有同名工具则跳过，避免重复
        if defs.iter().any(|d| d["name"] == name) {
            continue;
        }
        defs.push(serde_json::json!({
            "name": name, "description": desc, "parameters": params,
        }));
    }
    defs
}

/// 原生工具一轮对话：自动按提供方协议分发（OpenAI / Claude / Gemini）
pub async fn chat_native_round(
    ctx: &Arc<crate::state::Ctx>,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
) -> Result<NativeRound, NativeErr> {
    let (p, params) = {
        let cfg = ctx.ai_config.lock().unwrap();
        match cfg.active() {
            Some(p) => (p.clone(), cfg.clone()),
            None => return Err(NativeErr::Other("未配置任何 AI 提供方".into())),
        }
    };
    if p.api_key.is_empty() {
        return Err(NativeErr::Other(format!(
            "提供方「{}」未填写 API Key",
            p.name
        )));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| NativeErr::Other(e.to_string()))?;
    let defs = native_tool_defs(ctx);
    match p.protocol.as_str() {
        "claude" => native_round_claude(&client, &p, convo, images, exchanges, &defs, &params).await,
        "gemini" => native_round_gemini(&client, &p, convo, images, exchanges, &defs, &params).await,
        _ => native_round_openai(&client, &p, convo, images, exchanges, &defs, &params).await,
    }
}

/// 读响应体：非 2xx → 分类为 Unsupported / Other
async fn read_json_native(resp: reqwest::Response) -> Result<serde_json::Value, NativeErr> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let short = crate::registry::safe_trunc(&text, 400);
        if status.is_client_error() {
            let low = text.to_lowercase();
            if low.contains("tool") || low.contains("function") || low.contains("schema") {
                return Err(NativeErr::Unsupported(format!("HTTP {status}: {short}")));
            }
        }
        return Err(NativeErr::Other(format!("HTTP {status}: {short}")));
    }
    serde_json::from_str(&text).map_err(|e| NativeErr::Other(format!("响应解析失败: {e}")))
}

/// 非 2xx 响应分类（流式与一次性共用）：错误体含 tool/function/schema → Unsupported
/// （探测失败，调用方降级文本约定）；其余 → Other（流式路径可退回一次性请求）。
/// 仅在非 2xx 时调用（会消耗响应体读取错误详情）
async fn native_err_from_resp(status: reqwest::StatusCode, resp: reqwest::Response) -> NativeErr {
    let text = resp.text().await.unwrap_or_default();
    let short = crate::registry::safe_trunc(&text, 400);
    if status.is_client_error() {
        let low = text.to_lowercase();
        if low.contains("tool") || low.contains("function") || low.contains("schema") {
            return NativeErr::Unsupported(format!("HTTP {status}: {short}"));
        }
    }
    NativeErr::Other(format!("HTTP {status}: {short}"))
}

/// 构造 OpenAI 原生工具调用请求体（流式/一次性共用）
fn openai_native_body(
    p: &Provider,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    defs: &[serde_json::Value],
    params: &AiConfig,
) -> serde_json::Value {
    let mut msgs = openai_messages(convo, images);
    for ex in exchanges {
        let tcs: Vec<serde_json::Value> = ex
            .calls
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id, "type": "function",
                    "function": {
                        "name": c.name,
                        "arguments": serde_json::to_string(&c.args).unwrap_or_else(|_| "{}".into()),
                    },
                })
            })
            .collect();
        msgs.push(serde_json::json!({"role": "assistant", "content": null, "tool_calls": tcs}));
        for (c, r) in ex.calls.iter().zip(ex.results.iter()) {
            let content = if r.ok {
                serde_json::to_string(&r.value).unwrap_or_default()
            } else {
                format!(
                    "工具执行失败: {}",
                    serde_json::to_string(&r.value).unwrap_or_default()
                )
            };
            msgs.push(serde_json::json!({"role": "tool", "tool_call_id": c.id, "content": content}));
        }
    }
    let tools: Vec<serde_json::Value> = defs
        .iter()
        .map(|d| serde_json::json!({"type": "function", "function": d}))
        .collect();
    let mut body = serde_json::json!({
        "model": p.model,
        // DeepSeek 等默认 max_tokens=4096：长预告+工具调用易撞上限导致“话说一半没调用”，放宽到 8192
        "max_tokens": 8192,
        "messages": msgs,
        "tools": tools,
        "tool_choice": "auto",
    });
    apply_params("openai", &mut body, params);
    body
}

/// 构造 Claude 原生工具调用请求体（流式/一次性共用）
fn claude_native_body(
    p: &Provider,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    defs: &[serde_json::Value],
    params: &AiConfig,
) -> serde_json::Value {
    let (system_txt, mut msgs) = claude_messages(convo, images);
    for ex in exchanges {
        let blocks: Vec<serde_json::Value> = ex
            .calls
            .iter()
            .map(|c| {
                serde_json::json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.args})
            })
            .collect();
        if !blocks.is_empty() {
            msgs.push(serde_json::json!({"role": "assistant", "content": blocks}));
        }
        // user：tool_result 块
        let results: Vec<serde_json::Value> = ex
            .calls
            .iter()
            .zip(ex.results.iter())
            .map(|(c, r)| {
                serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": c.id,
                    "content": serde_json::to_string(&r.value).unwrap_or_default(),
                    "is_error": !r.ok,
                })
            })
            .collect();
        if !results.is_empty() {
            msgs.push(serde_json::json!({"role": "user", "content": results}));
        }
    }
    let tools: Vec<serde_json::Value> = defs
        .iter()
        .map(|d| {
            serde_json::json!({
                "name": d["name"], "description": d["description"], "input_schema": d["parameters"],
            })
        })
        .collect();
    let mut body = serde_json::json!({
        "model": p.model,
        "max_tokens": 8192,
        "messages": msgs,
        "tools": tools,
        "tool_choice": {"type": "auto"},
    });
    apply_params("claude", &mut body, params);
    claude_apply_cache(&mut body, &system_txt, &mut msgs);
    body["messages"] = serde_json::json!(msgs);
    body
}

/// 构造 Gemini 原生工具调用请求体（流式/一次性共用）
fn gemini_native_body(
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    defs: &[serde_json::Value],
    params: &AiConfig,
) -> serde_json::Value {
    let (system_txt, mut contents) = gemini_contents(convo, images);
    for ex in exchanges {
        // model：functionCall 部分
        let model_parts: Vec<serde_json::Value> = ex
            .calls
            .iter()
            .map(|c| {
                serde_json::json!({"functionCall": {"name": c.name, "args": c.args}})
            })
            .collect();
        if !model_parts.is_empty() {
            contents.push(serde_json::json!({"role": "model", "parts": model_parts}));
        }
        // user：functionResponse 部分（Gemini v1beta 按名字对应，无 id）
        let user_parts: Vec<serde_json::Value> = ex
            .calls
            .iter()
            .zip(ex.results.iter())
            .map(|(c, r)| {
                serde_json::json!({
                    "functionResponse": {
                        "name": c.name,
                        "response": {"ok": r.ok, "result": r.value},
                    },
                })
            })
            .collect();
        if !user_parts.is_empty() {
            contents.push(serde_json::json!({"role": "user", "parts": user_parts}));
        }
    }
    let decls: Vec<serde_json::Value> = defs
        .iter()
        .map(|d| {
            serde_json::json!({
                "name": d["name"], "description": d["description"], "parameters": d["parameters"],
            })
        })
        .collect();
    let mut body = serde_json::json!({
        "contents": contents,
        "tools": [{"functionDeclarations": decls}],
    });
    apply_params("gemini", &mut body, params);
    if !system_txt.is_empty() {
        body["systemInstruction"] =
            serde_json::json!({"parts": [{"text": system_txt}]});
    }
    body
}

/// 原生工具调用流式增量事件：Text = 正文增量，Think = 思考过程增量
#[derive(Clone, Copy, Debug)]
pub enum NativeEvent<'a> {
    Text(&'a str),
    Think(&'a str),
}

/// 原生一轮（流式）：文本/思考增量实时回调，工具调用增量在本地聚合；
/// 回调返回 false 立即停止（会话中断，返回 STREAM_STOP 哨兵）。
/// 端点拒绝流式/工具参数（Unsupported）原样上抛（调用方降级文本约定）；
/// 其他流式失败在尚未输出任何增量时自动退回一次性请求（整段以单事件补发）。
pub async fn chat_native_round_stream<F: FnMut(NativeEvent) -> bool + Send>(
    ctx: &Arc<crate::state::Ctx>,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    mut on_event: F,
) -> Result<NativeRound, NativeErr> {
    let (p, params) = {
        let cfg = ctx.ai_config.lock().unwrap();
        match cfg.active() {
            Some(p) => (p.clone(), cfg.clone()),
            None => return Err(NativeErr::Other("未配置任何 AI 提供方".into())),
        }
    };
    if p.api_key.is_empty() {
        return Err(NativeErr::Other(format!(
            "提供方「{}」未填写 API Key",
            p.name
        )));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| NativeErr::Other(e.to_string()))?;
    let defs = native_tool_defs(ctx);

    // 记录是否已向调用方发出过增量：发出过就不能再退回一次性重发（会造成文本重复）
    let mut emitted = false;
    let result = {
        let emitted_ref = &mut emitted;
        let inner = &mut on_event;
        let mut wrapped = |ev: NativeEvent| {
            *emitted_ref = true;
            inner(ev)
        };
        match p.protocol.as_str() {
            "claude" => {
                native_round_claude_stream(&client, &p, convo, images, exchanges, &defs, &params, &mut wrapped).await
            }
            "gemini" => {
                native_round_gemini_stream(&client, &p, convo, images, exchanges, &defs, &params, &mut wrapped).await
            }
            _ => {
                native_round_openai_stream(&client, &p, convo, images, exchanges, &defs, &params, &mut wrapped).await
            }
        }
    };
    match result {
        Ok(round) => Ok(round),
        Err(NativeErr::Unsupported(e)) => Err(NativeErr::Unsupported(e)),
        Err(NativeErr::Other(e)) if e == STREAM_STOP => Err(NativeErr::Other(e)),
        Err(_) if !emitted => {
            // 流式不可用（端点不支持 stream/参数被拒等）：退回一次性请求，整段以单事件补发
            let round = chat_native_round(ctx, convo, images, exchanges).await?;
            let t = round.thinking.clone();
            if !t.is_empty() {
                on_event(NativeEvent::Think(&t));
            }
            let c = round.content.clone();
            if !c.is_empty() {
                on_event(NativeEvent::Text(&c));
            }
            Ok(round)
        }
        Err(e) => Err(e),
    }
}

async fn native_round_openai_stream(
    client: &reqwest::Client,
    p: &Provider,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    defs: &[serde_json::Value],
    params: &AiConfig,
    on_event: &mut (dyn FnMut(NativeEvent) -> bool + Send),
) -> Result<NativeRound, NativeErr> {
    let url = format!("{}/chat/completions", p.base_url.trim_end_matches('/'));
    let mut body = openai_native_body(p, convo, images, exchanges, defs, params);
    body["stream"] = serde_json::json!(true);
    // 末 chunk 携带 usage（主流端点支持；不支持的端点报错则由入口退回一次性请求）
    body["stream_options"] = serde_json::json!({"include_usage": true});
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", p.api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| NativeErr::Other(format!("请求失败: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(native_err_from_resp(status, resp).await);
    }

    let mut content = String::new();
    let mut thinking = String::new();
    let mut stopped = false;
    // index → (id, name, arguments 字符串增量拼接)
    let mut tcs: std::collections::BTreeMap<usize, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut finish = String::new();
    let mut usage = TokenUsage::default();
    let mut got_any = false;
    let mut bad_data = 0usize; // 非 JSON 的 data 行数（垃圾响应容错判断用）
    read_sse(resp, |data| {
        got_any = true;
        if data == "[DONE]" {
            return true;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
            bad_data += 1;
            return false;
        };
        if v.get("usage").is_some_and(|u| u.is_object()) {
            usage = usage_from_openai(&v);
        }
        if let Some(f) = v.pointer("/choices/0/finish_reason").and_then(|x| x.as_str()) {
            if !f.is_empty() {
                finish = f.to_string();
            }
        }
        let mut usable = false;
        if let Some(delta) = v.pointer("/choices/0/delta") {
            // 正文：字符串直取；内容块数组（网关转换常见）拼接各块文本
            if let Some(t) = content_str(delta.get("content")).filter(|s| !s.is_empty()) {
                usable = true;
                content.push_str(&t);
                if !on_event(NativeEvent::Text(&t)) {
                    stopped = true;
                    return true;
                }
            }
            if let Some(t) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
            {
                usable = true;
                thinking.push_str(t);
                if !on_event(NativeEvent::Think(t)) {
                    stopped = true;
                    return true;
                }
            }
            if let Some(arr) = delta.get("tool_calls").and_then(|x| x.as_array()) {
                if !arr.is_empty() {
                    usable = true;
                }
                for tc in arr {
                    let i = tc.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                    let e = tcs.entry(i).or_default();
                    if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                        if !id.is_empty() {
                            e.0 = id.to_string();
                        }
                    }
                    if let Some(n) = tc.pointer("/function/name").and_then(|x| x.as_str()) {
                        if !n.is_empty() {
                            e.1.push_str(n);
                        }
                    }
                    if let Some(a) = tc.pointer("/function/arguments").and_then(|x| x.as_str()) {
                        e.2.push_str(a);
                    }
                }
            }
        }
        // 模糊回退：本 chunk 无任何可识别内容时（旧版 choices[0].text / Responses API output_text 等变体）
        if !usable {
            if let Some(t) = fuzzy_text(&v) {
                content.push_str(&t);
                if !on_event(NativeEvent::Text(&t)) {
                    stopped = true;
                    return true;
                }
            }
        }
        stopped
    })
    .await
    .map_err(NativeErr::Other)?;
    if stopped {
        return Err(NativeErr::Other(STREAM_STOP.into()));
    }
    // 端点忽略 stream 参数返回整段 JSON：SSE 解析无任何事件 → 交由入口退回一次性请求
    if !got_any {
        return Err(NativeErr::Other("端点未返回 SSE 流".into()));
    }
    // 全程无可识别内容且存在非 JSON 数据行 → 响应体是垃圾：
    // 交由入口退回一次性请求，由一次性路径报出明确的解析失败
    if content.is_empty() && thinking.is_empty() && tcs.is_empty() && bad_data > 0 {
        return Err(NativeErr::Other("流式响应体不可解析".into()));
    }

    let calls: Vec<NativeToolCall> = tcs
        .into_iter()
        .filter(|(_, (_, name, _))| !name.is_empty())
        .map(|(i, (id, name, args))| NativeToolCall {
            id: if id.is_empty() { format!("stream-{i}") } else { id },
            name,
            args: serde_json::from_str(&args).unwrap_or(serde_json::json!({})),
        })
        .collect();
    let mut content = content;
    if finish_truncated(&finish) {
        content.push_str("\n\n（回复因达到最大输出长度被截断，可回复“继续”）");
    }
    Ok(NativeRound { content, thinking, calls, usage })
}

async fn native_round_claude_stream(
    client: &reqwest::Client,
    p: &Provider,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    defs: &[serde_json::Value],
    params: &AiConfig,
    on_event: &mut (dyn FnMut(NativeEvent) -> bool + Send),
) -> Result<NativeRound, NativeErr> {
    let url = format!("{}/v1/messages", p.base_url.trim_end_matches('/'));
    let mut body = claude_native_body(p, convo, images, exchanges, defs, params);
    body["stream"] = serde_json::json!(true);
    let resp = client
        .post(&url)
        .header("x-api-key", &p.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| NativeErr::Other(format!("请求失败: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(native_err_from_resp(status, resp).await);
    }

    // index → 内容块聚合（text/thinking 累积文本；tool_use 拼接 partial_json）
    struct Blk {
        btype: String,
        text: String,
        tool_id: String,
        tool_name: String,
        tool_json: String,
    }
    let mut blocks: std::collections::BTreeMap<u64, Blk> = std::collections::BTreeMap::new();
    let mut stopped = false;
    let mut usage = TokenUsage::default();
    let mut stop_reason = String::new();
    let mut got_any = false;
    read_sse(resp, |data| {
        got_any = true;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { return false };
        match v.get("type").and_then(|x| x.as_str()) {
            Some("message_start") => {
                usage.prompt_tokens = v.pointer("/message/usage/input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                usage.cache_read_tokens = v.pointer("/message/usage/cache_read_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                usage.cache_write_tokens = v.pointer("/message/usage/cache_creation_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            }
            Some("content_block_start") => {
                let i = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0);
                let cb = v.get("content_block").cloned().unwrap_or(serde_json::json!({}));
                let e = blocks.entry(i).or_insert(Blk {
                    btype: String::new(), text: String::new(), tool_id: String::new(),
                    tool_name: String::new(), tool_json: String::new(),
                });
                e.btype = cb.get("type").and_then(|x| x.as_str()).unwrap_or_default().to_string();
                e.tool_id = cb.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string();
                e.tool_name = cb.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string();
            }
            Some("content_block_delta") => {
                let i = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0);
                let Some(d) = v.get("delta") else { return false };
                let dt = d.get("type").and_then(|x| x.as_str()).unwrap_or_default();
                let e = blocks.entry(i).or_insert(Blk {
                    btype: String::new(), text: String::new(), tool_id: String::new(),
                    tool_name: String::new(), tool_json: String::new(),
                });
                match dt {
                    "text_delta" => {
                        if let Some(t) = d.get("text").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
                            e.text.push_str(t);
                            if !on_event(NativeEvent::Text(t)) {
                                stopped = true;
                                return true;
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = d.get("thinking").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
                            e.text.push_str(t);
                            if !on_event(NativeEvent::Think(t)) {
                                stopped = true;
                                return true;
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(t) = d.get("partial_json").and_then(|x| x.as_str()) {
                            e.tool_json.push_str(t);
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                stop_reason = v.pointer("/delta/stop_reason").and_then(|x| x.as_str()).unwrap_or_default().to_string();
                usage.completion_tokens = v.pointer("/usage/output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            }
            Some("message_stop") => return true,
            _ => {}
        }
        stopped
    })
    .await
    .map_err(NativeErr::Other)?;
    if stopped {
        return Err(NativeErr::Other(STREAM_STOP.into()));
    }
    // 端点忽略 stream 参数返回整段 JSON：SSE 解析无任何事件 → 交由入口退回一次性请求
    if !got_any {
        return Err(NativeErr::Other("端点未返回 SSE 流".into()));
    }

    let mut content = String::new();
    let mut thinking = String::new();
    let mut calls = Vec::new();
    for (_, b) in blocks {
        match b.btype.as_str() {
            "text" => content.push_str(&b.text),
            "thinking" => thinking.push_str(&b.text),
            "tool_use" => {
                if !b.tool_name.is_empty() {
                    calls.push(NativeToolCall {
                        id: b.tool_id,
                        name: b.tool_name,
                        args: serde_json::from_str(&b.tool_json).unwrap_or(serde_json::json!({})),
                    });
                }
            }
            _ => {}
        }
    }
    if finish_truncated(&stop_reason) {
        content.push_str("\n\n（回复因达到最大输出长度被截断，可回复“继续”）");
    }
    Ok(NativeRound { content, thinking, calls, usage })
}

async fn native_round_gemini_stream(
    client: &reqwest::Client,
    p: &Provider,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    defs: &[serde_json::Value],
    params: &AiConfig,
    on_event: &mut (dyn FnMut(NativeEvent) -> bool + Send),
) -> Result<NativeRound, NativeErr> {
    let url = format!(
        "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
        p.base_url.trim_end_matches('/'),
        p.model
    );
    let body = gemini_native_body(convo, images, exchanges, defs, params);
    let resp = client
        .post(&url)
        .header("x-goog-api-key", &p.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| NativeErr::Other(format!("请求失败: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(native_err_from_resp(status, resp).await);
    }

    let mut content = String::new();
    let mut thinking = String::new();
    let mut calls = Vec::new();
    let mut stopped = false;
    let mut usage = TokenUsage::default();
    let mut finish = String::new();
    let mut got_any = false;
    read_sse(resp, |data| {
        got_any = true;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { return false };
        if let Some(parts) = v.pointer("/candidates/0/content/parts").and_then(|x| x.as_array()) {
            for part in parts {
                if let Some(t) = part.get("text").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
                    // thought=true 的 part 是思考过程，不混入正文
                    if part.get("thought").and_then(|x| x.as_bool()).unwrap_or(false) {
                        thinking.push_str(t);
                        if !on_event(NativeEvent::Think(t)) {
                            stopped = true;
                            return true;
                        }
                    } else {
                        content.push_str(t);
                        if !on_event(NativeEvent::Text(t)) {
                            stopped = true;
                            return true;
                        }
                    }
                }
                if let Some(fc) = part.get("functionCall") {
                    let name = fc.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string();
                    if !name.is_empty() {
                        calls.push(NativeToolCall {
                            id: format!("gemini-{}", calls.len()),
                            name,
                            args: fc.get("args").cloned().unwrap_or(serde_json::json!({})),
                        });
                    }
                }
            }
        }
        if v.get("usageMetadata").is_some_and(|u| u.is_object()) {
            usage = usage_from_gemini(&v);
        }
        if let Some(f) = v.pointer("/candidates/0/finishReason").and_then(|x| x.as_str()) {
            finish = f.to_string();
        }
        stopped
    })
    .await
    .map_err(NativeErr::Other)?;
    if stopped {
        return Err(NativeErr::Other(STREAM_STOP.into()));
    }
    // 端点忽略 alt=sse 参数返回 JSON 数组：SSE 解析无任何事件 → 交由入口退回一次性请求
    if !got_any {
        return Err(NativeErr::Other("端点未返回 SSE 流".into()));
    }

    let mut content = content;
    if finish_truncated(&finish) {
        content.push_str("\n\n（回复因达到最大输出长度被截断，可回复“继续”）");
    }
    Ok(NativeRound { content, thinking, calls, usage })
}

async fn native_round_openai(
    client: &reqwest::Client,
    p: &Provider,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    defs: &[serde_json::Value],
    params: &AiConfig,
) -> Result<NativeRound, NativeErr> {
    let url = format!("{}/chat/completions", p.base_url.trim_end_matches('/'));
    let body = openai_native_body(p, convo, images, exchanges, defs, params);
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", p.api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| NativeErr::Other(format!("请求失败: {e}")))?;
    let value = read_json_native(resp).await?;
    let msg = &value["choices"][0]["message"];
    let mut content = msg
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // finish_reason 模糊归一化：达到输出上限（可能连工具调用都没来得及发出），显式标注
    if value
        .pointer("/choices/0/finish_reason")
        .and_then(|v| v.as_str())
        .is_some_and(finish_truncated)
    {
        content.push_str("\n\n（回复因达到最大输出长度被截断，可回复“继续”）");
    }
    let mut calls = Vec::new();
    if let Some(arr) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in arr {
            let name = tc
                .pointer("/function/name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            // arguments 兼容两种形态：标准字符串 JSON / 部分端点直接给对象
            let args = match tc.pointer("/function/arguments") {
                Some(serde_json::Value::String(s)) => {
                    serde_json::from_str(s).unwrap_or(serde_json::json!({}))
                }
                Some(v @ serde_json::Value::Object(_)) => v.clone(),
                _ => serde_json::json!({}),
            };
            if !name.is_empty() {
                calls.push(NativeToolCall {
                    id: tc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    name,
                    args,
                });
            }
        }
    }
    // 模糊回退：旧版 function_call 单调用格式（无 tool_calls 数组）
    if calls.is_empty() {
        if let Some(fc) = msg.get("function_call") {
            let name = fc
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if !name.is_empty() {
                let args = match fc.get("arguments") {
                    Some(serde_json::Value::String(s)) => {
                        serde_json::from_str(s).unwrap_or(serde_json::json!({}))
                    }
                    Some(v @ serde_json::Value::Object(_)) => v.clone(),
                    _ => serde_json::json!({}),
                };
                calls.push(NativeToolCall { id: "legacy-fc".into(), name, args });
            }
        }
    }
    // 模糊回退：content 为内容块数组等变体（仅在无工具调用时启用，避免误读参数为正文）
    if content.is_empty() && calls.is_empty() {
        if let Some(t) = fuzzy_text(&value) {
            content = t;
        }
    }
    // 思考过程：reasoning_content（DeepSeek R1）/ reasoning（OpenRouter 变体）
    let thinking = value
        .pointer("/choices/0/message/reasoning_content")
        .or_else(|| value.pointer("/choices/0/message/reasoning"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Ok(NativeRound { content, thinking, calls, usage: usage_from_openai(&value) })
}

async fn native_round_claude(
    client: &reqwest::Client,
    p: &Provider,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    defs: &[serde_json::Value],
    params: &AiConfig,
) -> Result<NativeRound, NativeErr> {
    let body = claude_native_body(p, convo, images, exchanges, defs, params);
    let url = format!("{}/v1/messages", p.base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("x-api-key", &p.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| NativeErr::Other(format!("请求失败: {e}")))?;
    let value = read_json_native(resp).await?;
    let mut content = String::new();
    let mut thinking = String::new();
    let mut calls = Vec::new();
    if let Some(arr) = value.get("content").and_then(|v| v.as_array()) {
        for b in arr {
            match b.get("type").and_then(|v| v.as_str()) {
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                        content.push_str(t);
                    }
                }
                // thinking 块：思考过程，不混入正文
                Some("thinking") => {
                    if let Some(t) = b.get("thinking").and_then(|v| v.as_str()) {
                        thinking.push_str(t);
                    }
                }
                Some("tool_use") => {
                    let name = b
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if !name.is_empty() {
                        calls.push(NativeToolCall {
                            id: b
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            name,
                            args: b.get("input").cloned().unwrap_or(serde_json::json!({})),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    // 模糊回退：content 为纯字符串等变体（仅在无工具调用时启用，避免误读 input 参数为正文）
    if content.is_empty() && calls.is_empty() {
        if let Some(t) = fuzzy_text(&value) {
            content = t;
        }
    }
    Ok(NativeRound { content, thinking, calls, usage: usage_from_claude(&value) })
}

async fn native_round_gemini(
    client: &reqwest::Client,
    p: &Provider,
    convo: &[ChatMessage],
    images: &[String],
    exchanges: &[ToolExchange],
    defs: &[serde_json::Value],
    params: &AiConfig,
) -> Result<NativeRound, NativeErr> {
    let body = gemini_native_body(convo, images, exchanges, defs, params);
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
        .map_err(|e| NativeErr::Other(format!("请求失败: {e}")))?;
    let value = read_json_native(resp).await?;
    let mut content = String::new();
    let mut thinking = String::new();
    let mut calls = Vec::new();
    if let Some(parts) = value
        .pointer("/candidates/0/content/parts")
        .and_then(|v| v.as_array())
    {
        for part in parts {
            if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                // thought=true 的 part 是思考过程，不混入正文
                if part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false) {
                    thinking.push_str(t);
                } else {
                    content.push_str(t);
                }
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                if !name.is_empty() {
                    calls.push(NativeToolCall {
                        id: format!("gemini-{}", calls.len()),
                        name,
                        args: fc.get("args").cloned().unwrap_or(serde_json::json!({})),
                    });
                }
            }
        }
    }
    // 模糊回退：非标准结构（仅在无工具调用时启用，避免误读 functionCall 参数为正文）
    if content.is_empty() && calls.is_empty() {
        if let Some(t) = fuzzy_text(&value) {
            content = t;
        }
    }
    Ok(NativeRound { content, thinking, calls, usage: usage_from_gemini(&value) })
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;

    /// 极简 mock AI 服务器：按入队顺序逐请求回 (状态码, JSON体)，并把每次收到的请求体记录下来
    async fn spawn_mock_ai(responses: Vec<(u16, String)>) -> (String, Arc<StdMutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let queue = Arc::new(StdMutex::new(responses));
        let bodies: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let bodies_cloned = bodies.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let queue = queue.clone();
                let bodies = bodies_cloned.clone();
                tauri::async_runtime::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let head_end = loop {
                        let Ok(n) = sock.read(&mut chunk).await else { return };
                        if n == 0 { return; }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break p + 4;
                        }
                        if buf.len() > 1 << 20 { return; }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let clen: usize = head
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    while buf.len() < head_end + clen {
                        let Ok(n) = sock.read(&mut chunk).await else { break };
                        if n == 0 { break; }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    bodies.lock().unwrap().push(String::from_utf8_lossy(&buf[head_end..]).to_string());
                    let (status, resp_body) = {
                        let mut q = queue.lock().unwrap();
                        if q.is_empty() { (500, "{}".into()) } else { q.remove(0) }
                    };
                    let reason = match status { 200 => "OK", 400 => "Bad Request", _ => "Error" };
                    let resp = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                        resp_body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}"), bodies)
    }

    fn test_provider(protocol: &str, base_url: &str) -> Provider {
        Provider {
            id: "t".into(),
            name: "测试".into(),
            protocol: protocol.into(),
            base_url: base_url.into(),
            api_key: "test-key".into(),
            model: "test-model".into(),
            active: true,
        }
    }

    #[tokio::test]
    async fn test_native_openai_stream() {
        // 原生流式：tool_calls 增量跨 chunk 聚合、reasoning_content → Think、content → Text、末 chunk usage
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let got_body = Arc::new(StdMutex::new(String::new()));
        let gb = got_body.clone();
        tauri::async_runtime::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let mut buf = vec![0u8; 65536];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            *gb.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await;
            let frames = [
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"add","arguments":"{\"a\":"}}]}}]}).to_string(),
                json!({"choices":[{"delta":{"reasoning_content":"想"}}]}).to_string(),
                json!({"choices":[{"delta":{"content":"答"}}]}).to_string(),
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}).to_string(),
                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}).to_string(),
                json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":8}}}).to_string(),
            ];
            for f in &frames {
                let ev = format!("data: {f}\n\n");
                let _ = sock.write_all(format!("{:x}\r\n{}\r\n", ev.len(), ev).as_bytes()).await;
                let _ = sock.flush().await;
            }
            let done = "data: [DONE]\n\n";
            let _ = sock.write_all(format!("{:x}\r\n{}\r\n", done.len(), done).as_bytes()).await;
            let _ = sock.write_all(b"0\r\n\r\n").await;
        });
        let p = test_provider("openai", &format!("http://{addr}"));
        let client = reqwest::Client::new();
        let convo = vec![ChatMessage::user("1+2".to_string())];
        let mut events: Vec<(&'static str, String)> = Vec::new();
        let r = native_round_openai_stream(
            &client, &p, &convo, &[], &[], &[],
            &AiConfig::default(),
            &mut |ev| {
                match ev {
                    NativeEvent::Text(t) => events.push(("text", t.to_string())),
                    NativeEvent::Think(t) => events.push(("think", t.to_string())),
                }
                true
            },
        )
        .await
        .unwrap();
        // 事件顺序：思考先于正文，增量实时回调
        assert_eq!(
            events,
            vec![("think", "想".to_string()), ("text", "答".to_string())],
            "流式增量应按到达顺序回调"
        );
        // tool_calls 增量跨 chunk 聚合 + usage 来自末 chunk
        assert_eq!(r.content, "答");
        assert_eq!(r.thinking, "想");
        assert_eq!(r.calls.len(), 1);
        assert_eq!(r.calls[0].id, "call-1");
        assert_eq!(r.calls[0].name, "add");
        assert_eq!(r.calls[0].args["a"], 1);
        assert_eq!(r.usage.prompt_tokens, 10);
        assert_eq!(r.usage.cache_read_tokens, 8);
        assert_eq!(r.usage.completion_tokens, 5);
        // 请求体应带 stream + include_usage
        let sent = got_body.lock().unwrap();
        let body_start = sent.find('{').unwrap();
        let req: serde_json::Value = serde_json::from_str(&sent[body_start..]).unwrap_or(json!({}));
        assert_eq!(req["stream"], true);
        assert_eq!(req["stream_options"]["include_usage"], true);
    }

    #[tokio::test]
    async fn test_native_openai_round() {
        // 第一轮：返回 tool_calls；第二轮：返回最终文本
        let (url, bodies) = spawn_mock_ai(vec![
            (
                200,
                json!({"choices":[{"message":{"content":"我来算一下","tool_calls":[
                    {"id":"call-1","type":"function","function":{"name":"shell","arguments":"{\"command\":\"echo hi\"}"}},
                    {"id":"call-2","type":"function","function":{"name":"add","arguments":"{\"a\":1,\"b\":2}"}}
                ]}}]}).to_string(),
            ),
            (200, json!({"choices":[{"message":{"content":"答案是 3"}}]}).to_string()),
        ])
        .await;
        let p = test_provider("openai", &url);
        let client = reqwest::Client::new();
        let convo = vec![ChatMessage::system("sys".to_string()), ChatMessage::user("算 1+2".to_string())];

        let r1 = native_round_openai(&client, &p, &convo, &[], &[], &[], &AiConfig::default()).await.unwrap();
        assert_eq!(r1.calls.len(), 2);
        assert_eq!(r1.calls[0].name, "shell");
        assert_eq!(r1.calls[0].args["command"], "echo hi");
        assert_eq!(r1.calls[1].args["a"], 1);

        // 第二轮：带上工具交换 → 请求里应出现 assistant(tool_calls) 与 role:"tool" 消息
        let ex = ToolExchange {
            calls: r1.calls.clone(),
            results: vec![
                ToolResult { ok: true, value: json!("hi") },
                ToolResult { ok: true, value: json!({"sum": 3}) },
            ],
        };
        let r2 = native_round_openai(&client, &p, &convo, &[], &[ex], &[], &AiConfig::default()).await.unwrap();
        assert_eq!(r2.content, "答案是 3");

        let sent = bodies.lock().unwrap();
        let req1: serde_json::Value = serde_json::from_str(&sent[0]).unwrap();
        assert_eq!(req1["tool_choice"], "auto");
        let req2: serde_json::Value = serde_json::from_str(&sent[1]).unwrap();
        let msgs = req2["messages"].as_array().unwrap();
        let tool1 = msgs.iter().find(|m| m["role"] == "tool" && m["tool_call_id"] == "call-1").expect("缺少 call-1 的 tool 消息");
        assert_eq!(tool1["content"], "\"hi\"");
        let tool2 = msgs.iter().find(|m| m["role"] == "tool" && m["tool_call_id"] == "call-2").expect("缺少 call-2 的 tool 消息");
        assert_eq!(tool2["content"], "{\"sum\":3}");
        let asst = msgs.iter().find(|m| m["role"] == "assistant" && m.get("tool_calls").is_some()).expect("缺少 assistant tool_calls");
        assert_eq!(asst["tool_calls"][0]["id"], "call-1");
    }

    #[tokio::test]
    async fn test_native_claude_round() {
        let (url, bodies) = spawn_mock_ai(vec![(
            200,
            json!({"content":[
                {"type":"text","text":"我用工具查一下"},
                {"type":"tool_use","id":"tu_1","name":"shell","input":{"command":"ls"}}
            ]}).to_string(),
        )])
        .await;
        let p = test_provider("claude", &url);
        let client = reqwest::Client::new();
        let defs = vec![json!({"name":"shell","description":"执行命令","parameters":{"type":"object","properties":{}}})];
        let r = native_round_claude(&client, &p, &[ChatMessage::user("列目录".to_string())], &[], &[], &defs, &AiConfig::default()).await.unwrap();
        assert_eq!(r.content, "我用工具查一下");
        assert_eq!(r.calls.len(), 1);
        assert_eq!(r.calls[0].id, "tu_1");
        assert_eq!(r.calls[0].args["command"], "ls");

        let req: serde_json::Value = serde_json::from_str(&bodies.lock().unwrap()[0]).unwrap();
        assert_eq!(req["tools"][0]["name"], "shell");
        assert!(req["tools"][0].get("input_schema").is_some(), "claude 工具应为 input_schema");
        assert_eq!(req["tool_choice"]["type"], "auto");
    }

    #[tokio::test]
    async fn test_native_gemini_round() {
        let (url, bodies) = spawn_mock_ai(vec![(
            200,
            json!({"candidates":[{"content":{"parts":[
                {"text":"好的"},
                {"functionCall":{"name":"add","args":{"a":20,"b":22}}}
            ]}}]}).to_string(),
        )])
        .await;
        let p = test_provider("gemini", &url);
        let client = reqwest::Client::new();
        let defs = vec![json!({"name":"add","description":"加法","parameters":{"type":"object","properties":{}}})];
        let r = native_round_gemini(&client, &p, &[ChatMessage::user("算 20+22".to_string())], &[], &[], &defs, &AiConfig::default()).await.unwrap();
        assert_eq!(r.calls.len(), 1);
        assert_eq!(r.calls[0].name, "add");
        assert_eq!(r.calls[0].args["b"], 22);

        let req: serde_json::Value = serde_json::from_str(&bodies.lock().unwrap()[0]).unwrap();
        assert!(req["tools"][0].get("functionDeclarations").is_some(), "gemini 工具应为 functionDeclarations");
    }

    #[tokio::test]
    async fn test_native_error_classification() {
        // 4xx 且报错提到 tools/function → Unsupported（触发降级）
        let (url, _) = spawn_mock_ai(vec![(
            400,
            json!({"error":{"message":"'tools' is not supported by this model"}}).to_string(),
        )])
        .await;
        let p = test_provider("openai", &url);
        let client = reqwest::Client::new();
        let defs = vec![json!({"name":"shell","description":"","parameters":{}})];
        let r = native_round_openai(&client, &p, &[ChatMessage::user("hi".to_string())], &[], &[], &defs, &AiConfig::default()).await;
        assert!(matches!(r, Err(NativeErr::Unsupported(_))), "应识别为不支持原生工具: {r:?}");

        // 401 鉴权错误 → Other（不应误降级）
        let (url2, _) = spawn_mock_ai(vec![(401, json!({"error":{"message":"invalid api key"}}).to_string())]).await;
        let p2 = test_provider("openai", &url2);
        let r2 = native_round_openai(&client, &p2, &[ChatMessage::user("hi".to_string())], &[], &[], &defs, &AiConfig::default()).await;
        assert!(matches!(r2, Err(NativeErr::Other(_))), "鉴权错误不应判定为不支持: {r2:?}");
    }

    /// 模拟 OpenAI SSE 流式端点：把 chunks 按固定间隔分块下发（真实 chunked 传输）
    async fn spawn_sse_server(chunks: Vec<&'static str>, delay_ms: u64) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let mut buf = vec![0u8; 16384];
            let _ = sock.read(&mut buf).await; // 丢弃请求
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await;
            for c in chunks {
                let ev = format!("data: {}\n\n", json!({"choices":[{"delta":{"content":c}}]}));
                let _ = sock
                    .write_all(format!("{:x}\r\n{}\r\n", ev.len(), ev).as_bytes())
                    .await;
                let _ = sock.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            let done = "data: [DONE]\n\n";
            let _ = sock
                .write_all(format!("{:x}\r\n{}\r\n", done.len(), done).as_bytes())
                .await;
            let _ = sock.write_all(b"0\r\n\r\n").await;
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn test_stream_openai_mock() {
        // 模拟流式 API：分 4 块逐块下发 → 应按序收到 4 个增量并拼出全文
        let url = spawn_sse_server(vec!["你", "好", "，", "世界"], 40).await;
        let p = test_provider("openai", &url);
        let client = reqwest::Client::new();
        let mut tokens: Vec<String> = Vec::new();
        let (full, usage) = stream_openai(
            &client, &p,
            &[ChatMessage::user("hi".to_string())], &[], &AiConfig::default(),
            &mut |_kind, t| { tokens.push(t.to_string()); true },
        )
        .await
        .unwrap();
        assert_eq!(full, "你好，世界");
        assert_eq!(tokens, vec!["你", "好", "，", "世界"]);
        // 模拟端点未携带 usage 字段：用量应保持为 0（不计入命中率统计）
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[tokio::test]
    async fn test_stream_openai_thinking() {
        // 思考过程提取：reasoning_content 增量 → TokenKind::Think（不混入正文），content 增量 → TokenKind::Text
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tauri::async_runtime::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let mut buf = vec![0u8; 16384];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await;
            let frames = [
                json!({"choices":[{"delta":{"reasoning_content":"思"}}]}).to_string(),
                json!({"choices":[{"delta":{"reasoning_content":"考"}}]}).to_string(),
                json!({"choices":[{"delta":{"content":"答"}}]}).to_string(),
            ];
            for f in &frames {
                let ev = format!("data: {f}\n\n");
                let _ = sock.write_all(format!("{:x}\r\n{}\r\n", ev.len(), ev).as_bytes()).await;
                let _ = sock.flush().await;
            }
            let done = "data: [DONE]\n\n";
            let _ = sock.write_all(format!("{:x}\r\n{}\r\n", done.len(), done).as_bytes()).await;
            let _ = sock.write_all(b"0\r\n\r\n").await;
        });
        let p = test_provider("openai", &format!("http://{addr}"));
        let client = reqwest::Client::new();
        let mut got: Vec<(TokenKind, String)> = Vec::new();
        let (full, _) = stream_openai(
            &client, &p,
            &[ChatMessage::user("hi".to_string())], &[], &AiConfig::default(),
            &mut |kind, t| { got.push((kind, t.to_string())); true },
        )
        .await
        .unwrap();
        assert_eq!(
            got,
            vec![
                (TokenKind::Think, "思".to_string()),
                (TokenKind::Think, "考".to_string()),
                (TokenKind::Text, "答".to_string()),
            ],
            "思考与正文应按 TokenKind 分流"
        );
        assert_eq!(full, "答", "完整正文不应混入思考内容");
    }

    #[tokio::test]
    async fn test_stream_openai_abort() {
        // 中断：回调在第 2 块返回 false → 立即停止读取并返回 STREAM_STOP（不等后 2 块）
        let url = spawn_sse_server(vec!["a", "b", "c", "d"], 400).await;
        let p = test_provider("openai", &url);
        let client = reqwest::Client::new();
        let start = std::time::Instant::now();
        let mut n = 0;
        let r = stream_openai(
            &client, &p,
            &[ChatMessage::user("hi".to_string())], &[], &AiConfig::default(),
            &mut |_kind, _t| { n += 1; n < 2 },
        )
        .await;
        assert!(matches!(r, Err(ref e) if e == STREAM_STOP), "应为 STREAM_STOP: {r:?}");
        assert_eq!(n, 2);
        assert!(start.elapsed() < std::time::Duration::from_millis(1000), "中止应及时返回");
    }

    #[tokio::test]
    async fn test_stream_large_payload_perf() {
        // 性能压测：500 块流式负载（约 4KB 文本）全部有序到达且完整拼接
        let chunks: Vec<&'static str> = (0..500)
            .map(|i| -> &'static str { Box::leak(format!("t{i};").into_boxed_str()) })
            .collect();
        let expect: String = chunks.concat();
        let url = spawn_sse_server(chunks, 0).await;
        let p = test_provider("openai", &url);
        let client = reqwest::Client::new();
        let (full, _) = stream_openai(
            &client, &p,
            &[ChatMessage::user("hi".to_string())], &[], &AiConfig::default(),
            &mut |_kind, _t| true,
        )
        .await
        .unwrap();
        assert_eq!(full, expect, "500 块应全部有序到达且无损拼接");
        assert!(full.starts_with("t0;t1;") && full.ends_with("t498;t499;"));
    }
}

/// 模糊格式识别测试：覆盖各协议常见变体 + 不得误读的结构
#[cfg(test)]
mod fuzzy_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_content_array_blocks() {
        // 部分网关把 content 返回成内容块数组
        let v = json!({"choices":[{"message":{"content":[
            {"type":"text","text":"你好"},{"type":"text","text":"世界"}
        ]}}]});
        assert_eq!(fuzzy_text(&v).as_deref(), Some("你好世界"));
    }

    #[test]
    fn legacy_completions_text() {
        // 旧版 completions：choices[0].text
        let v = json!({"choices":[{"text":"hello"}]});
        assert_eq!(fuzzy_text(&v).as_deref(), Some("hello"));
    }

    #[test]
    fn responses_api_output_text() {
        let v = json!({"object":"response","output_text":"hi there"});
        assert_eq!(fuzzy_text(&v).as_deref(), Some("hi there"));
    }

    #[test]
    fn stream_message_instead_of_delta() {
        // 流式网关直接回 message 而非 delta
        let v = json!({"choices":[{"message":{"role":"assistant","content":"chunk1"}}]});
        assert_eq!(fuzzy_text(&v).as_deref(), Some("chunk1"));
    }

    #[test]
    fn claude_content_plain_string() {
        let v = json!({"id":"msg_1","type":"message","content":"plain"});
        assert_eq!(fuzzy_text(&v).as_deref(), Some("plain"));
    }

    #[test]
    fn claude_multi_text_blocks() {
        let v = json!({"content":[
            {"type":"text","text":"A"},{"type":"text","text":"B"}
        ]});
        assert_eq!(fuzzy_text(&v).as_deref(), Some("AB"));
    }

    #[test]
    fn gemini_multi_part_concat() {
        // 严格路径只取 parts/0，模糊路径应拼接全部 parts
        let v = json!({"candidates":[{"content":{"parts":[{"text":"A"},{"text":"B"}]}}]});
        assert_eq!(fuzzy_text(&v).as_deref(), Some("AB"));
    }

    #[test]
    fn error_body_never_matched() {
        // 200 但带 error 结构：message 不能被当正文
        let v = json!({"error":{"message":"boom","type":"invalid_request"}});
        assert_eq!(fuzzy_text(&v), None);
    }

    #[test]
    fn reasoning_not_matched() {
        // 推理内容不是正文
        let v = json!({"choices":[{"delta":{"reasoning_content":"thinking..."}}]});
        assert_eq!(fuzzy_text(&v), None);
    }

    #[test]
    fn tool_call_payload_not_leaked() {
        // 工具调用结构（含 arguments 字符串）不能被当正文
        let v = json!({"choices":[{"message":{
            "content": null,
            "tool_calls":[{"id":"1","type":"function","function":{"name":"shell","arguments":"{\"command\":\"ls -la\"}"}}]
        }}]});
        assert_eq!(fuzzy_text(&v), None);
    }

    #[test]
    fn claude_tool_use_input_not_leaked() {
        // tool_use 块的 input 参数（可能含 content/text 键）不能被当正文
        let v = json!({"content":[{"type":"tool_use","id":"t1","name":"write_file",
            "input":{"path":"a.txt","content":"file body"}}]});
        assert_eq!(fuzzy_text(&v), None);
    }

    #[test]
    fn claude_message_delta_not_matched() {
        // 流式 message_delta：stop_reason / usage 不是正文
        let v = json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":10}});
        assert_eq!(fuzzy_text(&v), None);
    }

    #[test]
    fn deep_nested_container_found() {
        // 非标准嵌套：result → items → response
        let v = json!({"result":{"items":[{"response":"deep"}]}});
        assert_eq!(fuzzy_text(&v).as_deref(), Some("deep"));
    }

    #[test]
    fn usage_only_chunk_not_matched() {
        let v = json!({"id":"x","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":5}});
        assert_eq!(fuzzy_text(&v), None);
    }

    #[test]
    fn finish_reason_variants() {
        for f in ["length", "MAX_TOKENS", "max_output_tokens", "Length", "token_limit"] {
            assert!(finish_truncated(f), "{f} 应判定为截断");
        }
        for f in ["stop", "end_turn", "STOP", "tool_calls", "max_tokens_exceeded_ok", ""] {
            assert!(!finish_truncated(f), "{f} 不应判定为截断");
        }
    }
}

/// SSE 字节缓冲切行测试：多字节字符跨 TCP chunk 截断时不得损坏
#[cfg(test)]
mod sse_tests {
    use super::drain_complete_lines;

    #[test]
    fn multibyte_split_across_chunks() {
        // 「你好🌍」的 UTF-8 字节被从中间切开，逐字节喂入
        let line = format!("data: {}\n", "你好🌍BIT-OK");
        let bytes = line.as_bytes();
        let mut buf: Vec<u8> = Vec::new();
        let mut got = Vec::new();
        for b in bytes {
            got.extend(drain_complete_lines(&mut buf, &[*b]));
        }
        // 末尾 \n 喂入时切出完整一行，多字节字符逐字节喂入也不得损坏
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("你好🌍BIT-OK"), "内容损坏: {}", got[0]);
    }

    #[test]
    fn multibyte_split_produces_intact_line() {
        let payload = format!("data: {{\"a\":\"你好🌍\"}}");
        let bytes = payload.as_bytes();
        let mut buf: Vec<u8> = Vec::new();
        let mut got = Vec::new();
        // 每 2 字节一切，模拟最恶劣的分块
        for c in bytes.chunks(2) {
            got.extend(drain_complete_lines(&mut buf, c));
        }
        got.extend(drain_complete_lines(&mut buf, b"\n"));
        assert_eq!(got.len(), 1, "应切出一整行");
        assert!(got[0].contains("你好🌍"), "多字节字符不得损坏: {}", got[0]);
        assert!(!got[0].contains('\u{FFFD}'), "不得出现替换符");
    }

    #[test]
    fn multiple_lines_and_crlf_safe() {
        let mut buf: Vec<u8> = Vec::new();
        let got = drain_complete_lines(&mut buf, b"data: a\ndata: b\ndata: c");
        assert_eq!(got, vec!["data: a", "data: b"]);
        // 尾部不完整行保留在 buf，下一块补上换行后切出
        let got2 = drain_complete_lines(&mut buf, b"\n");
        assert_eq!(got2, vec!["data: c"]);
        assert!(buf.is_empty());
    }
}
