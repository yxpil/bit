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

/// 流式对话：on_token 回调返回 false 表示调用方要求立即停止（如会话中断）。
/// 返回 (完整文本, token 用量)
pub async fn chat_stream_with_images<F: FnMut(&str) -> bool>(
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
            on_token(&full);
            Ok((full, usage))
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
async fn stream_openai<F: FnMut(&str) -> bool>(
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
    read_sse(resp, |data| {
        if data == "[DONE]" {
            return true;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            if v.get("usage").is_some() {
                usage = usage_from_openai(&v);
            }
            if let Some(delta) = v.pointer("/choices/0/delta/content").and_then(|x| x.as_str()) {
                full.push_str(delta);
                // 回调返回 false = 调用方中止（中断会话）：停止读取 SSE
                if !on_token(delta) {
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

/// Claude 流式：/v1/messages stream=true（content_block_delta）
async fn stream_claude<F: FnMut(&str) -> bool>(
    client: &reqwest::Client,
    p: &Provider,
    messages: &[ChatMessage],
    images: &[String],
    params: &AiConfig,
    on_token: &mut F,
) -> Result<(String, TokenUsage), String> {
    let (system_txt, mut msgs) = claude_messages(messages, images);
    let mut body = serde_json::json!({
        "model": p.model, "max_tokens": 4096, "messages": msgs, "stream": true
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
                if let Some(delta) = v.pointer("/delta/text").and_then(|x| x.as_str()) {
                    full.push_str(delta);
                    if !on_token(delta) {
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
async fn stream_gemini<F: FnMut(&str) -> bool>(
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
            if let Some(delta) = v.pointer("/candidates/0/content/parts/0/text").and_then(|x| x.as_str()) {
                full.push_str(delta);
                if !on_token(delta) {
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
    let content = value
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "响应中缺少 content".to_string())?;
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
        "max_tokens": 4096,
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
        .take(12)
        .map(|s| format!("- {}: {}", s.name, s.summary))
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

    // shell 工具说明按平台区分：Windows 用 PowerShell，macOS/Linux 用 POSIX shell
    let shell_syntax = if cfg!(windows) {
        "执行命令行（Windows PowerShell 语法，路径用 C:\\ 形式）"
    } else {
        "执行命令行（POSIX shell 语法，路径用 / 形式，如 /Users/xxx 与 /home/xxx）"
    };

    // 操作手册 / skill 示例 / 收尾句：文本约定与原生函数调用两种模式各自一份
    let (manual, skill_examples, closing) = if native {
        (
            "## 操作手册：如何调用工具（原生函数调用）\n\
            你处于原生函数调用模式：需要动手做事时直接发起 function call（可一次并行发起多个），\
            系统会执行并把每个工具的结果以工具消息回传给你；拿到结果后继续思考或再次调用，\
            全部完成后用自然语言输出最终答案（不要在回答里再输出任何调用格式说明或 JSON 调用数组）。",
            "写 SKILL、搜 SKILL 都用【工具6 · skill】：save 是自己写一条技能（同名覆盖），search 是搜索已有技能。",
            "不再需要动手时直接用自然语言输出最终答案即可。",
        )
    } else {
        (
            "## 操作手册：如何调用工具（务必遵守）\n\
            当你需要动手做事（执行命令、读写文件、制定计划、扩展自己、沉淀/查找技能）时，\
            在回答里【单独一行】输出一个 JSON 数组，数组每个元素形如 {{\"tool\":\"工具名\",\"params\":{{...}}}}。\n\
            这一行必须是纯 JSON，前后不要加解释文字、不要用代码块包裹；系统会执行后把结果回给你，你再据此继续。\n\
            严禁自创标记语法（如 <xxx_function_call>）、严禁输出不带方括号的裸对象、严禁把多个调用拆成多行——\
            多个调用必须放在同一个数组里：[{{...}},{{...}}]。不需要动手时正常用自然语言回答即可。\n\
            单个工具调用示例：[{{\"tool\":\"shell\",\"params\":{{\"command\":\"echo hi\"}}}}]",
            "写 SKILL、搜 SKILL 都用【工具6 · skill】：save 是自己写一条技能，search 是搜索已有技能。示例：\n\
            - 写 SKILL：[{{\"tool\":\"skill\",\"params\":{{\"action\":\"save\",\"name\":\"批量重命名\",\"summary\":\"用 shell 遍历目录并 mv 重命名文件的步骤…\"}}}}]\n\
            - 搜 SKILL：[{{\"tool\":\"skill\",\"params\":{{\"action\":\"search\",\"query\":\"重命名\"}}}}]",
            "需要调用能力时输出 JSON 数组，每个元素 {{\"tool\":string,\"params\":object}}，单独一行，不要包裹其他文字之外的内容。",
        )
    };

    format!(
        "你是 BIT，一个可以自我扩展的 AI 助手。你能调用工具、并通过写代码为自己增加新工具。\n\
        \n\
        {manual}\n\
        \n\
        ## 七个出厂内置工具（编号即「工具N」，在「已注册工具」清单中）\n\
        - 工具1 · shell：{shell_syntax}。参数 {{\"command\":string,\"cwd\":string(可选)}}\n\
        - 工具2 · write_file：写入/覆盖文件（文档编辑）。参数 {{\"path\":string,\"content\":string}}\n\
        - 工具3 · plan：制定计划，登记目标与分步待办。参数 {{\"goal\":string,\"steps\":[string]}}\n\
        - 工具4 · edit：增量补丁改文件，精确替换。参数 {{\"path\":string,\"old_string\":string,\"new_string\":string,\"replace_all\":bool(可选)}}\n\
        - 工具5 · add_tool：给自己增加工具——用本机某解释器把一段代码沉淀为常驻工具。参数 {{\"name\":string,\"description\":string,\"runtime\":string,\"code\":string}}\n\
        - 工具6 · skill：技能库读写。写入技能 {{\"action\":\"save\",\"name\":string,\"summary\":string}}（同名覆盖）；搜索技能 {{\"action\":\"search\",\"query\":string}}（query 留空返回全部）\n\
        - 工具7 · sub_agent：派生子智能体——新建独立会话执行自包含的大任务（调研/批量整理/写大文件），阻塞等待并返回其最终结论，子会话保留在侧栏可查看全过程。参数 {{\"task\":string,\"title\":string(可选)}}。注意 task 必须自包含：子智能体看不到当前对话历史，请把背景、目标、验收标准写全\n\
        {skill_examples}\n\
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
        {closing}",
        if runtime_lines.is_empty() { "（未探测到任何解释器，可在「工具」页点刷新探测）".to_string() } else { runtime_lines.join("\n") },
        if goal_lines.is_empty() { "（暂无）".to_string() } else { goal_lines.join("\n") },
        if todo_lines.is_empty() { "（暂无）".to_string() } else { todo_lines.join("\n") },
        serde_json::to_string_pretty(&tools_manifest(ctx)).unwrap_or_default(),
        if mem_lines.is_empty() { "（空）".to_string() } else { mem_lines.join("\n") },
        if skill_lines.is_empty() { "（空）".to_string() } else { skill_lines.join("\n") },
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
            "用本机解释器把一段代码沉淀为常驻工具",
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
        "messages": msgs,
        "tools": tools,
        "tool_choice": "auto",
    });
    apply_params("openai", &mut body, params);
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", p.api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| NativeErr::Other(format!("请求失败: {e}")))?;
    let value = read_json_native(resp).await?;
    let msg = &value["choices"][0]["message"];
    let content = msg
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut calls = Vec::new();
    if let Some(arr) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in arr {
            let name = tc
                .pointer("/function/name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let args_raw = tc
                .pointer("/function/arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let args = serde_json::from_str(args_raw).unwrap_or(serde_json::json!({}));
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
    Ok(NativeRound { content, calls, usage: usage_from_openai(&value) })
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
        "max_tokens": 4096,
        "messages": msgs,
        "tools": tools,
        "tool_choice": {"type": "auto"},
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
        .map_err(|e| NativeErr::Other(format!("请求失败: {e}")))?;
    let value = read_json_native(resp).await?;
    let mut content = String::new();
    let mut calls = Vec::new();
    if let Some(arr) = value.get("content").and_then(|v| v.as_array()) {
        for b in arr {
            match b.get("type").and_then(|v| v.as_str()) {
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                        content.push_str(t);
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
    Ok(NativeRound { content, calls, usage: usage_from_claude(&value) })
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
    let mut calls = Vec::new();
    if let Some(parts) = value
        .pointer("/candidates/0/content/parts")
        .and_then(|v| v.as_array())
    {
        for part in parts {
            if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                content.push_str(t);
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
    Ok(NativeRound { content, calls, usage: usage_from_gemini(&value) })
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
            &mut |t| { tokens.push(t.to_string()); true },
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
            &mut |_t| { n += 1; n < 2 },
        )
        .await;
        assert!(matches!(r, Err(ref e) if e == STREAM_STOP), "应为 STREAM_STOP: {r:?}");
        assert_eq!(n, 2);
        assert!(start.elapsed() < std::time::Duration::from_millis(1000), "中止应及时返回");
    }
}
