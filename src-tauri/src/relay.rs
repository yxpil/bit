// yxpil · BIT
//! 云中继客户端：手机端无法直连（对称 NAT / 防火墙）时，经 Cloudflare Worker 中继转发。
//! 应用端主动出站长轮询拉取隧道请求（无需公网地址与端口映射），收到请求后回环调用本地
//! HTTP API，再把响应回传中继。中继只转发、不留存任何内容（与二维码隐私声明一致）。
//!
//! 协议（与 relay-worker 约定）：
//!   POST {base}/relay/poll/{rid}          应用端长轮询（服务器最多持 25s），返回隧道请求或 204
//!         请求体 JSON: { rid, m, p, h, b }  m=方法 p=路径 h=头 b=base64 体
//!   POST {base}/relay/answer/{rid}        应用端回传响应 { rid, s, h, b }（s=状态码）
//!   手机端:  {base}/relay/req/{rid}/{path} 任意方法 → 中继排队 → 应用端取走 → 回传
use base64::Engine;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::state::Ctx;

/// 中继默认入口（osbt.space 上的 Cloudflare Worker；可在设置页改为自建地址）
pub const DEFAULT_RELAY_BASE: &str = "https://osbt.space";

/// 隧道单请求/响应体上限：文字信道（Worker 站点层 4MB + 只收 JSON/text，二进制 415），
/// 此处放宽到 20MB 只作兜底——真正的门在站点层（客户端开关绕不过）
const BODY_MAX: usize = 20 * 1024 * 1024;

/// 隧道请求允许转发的请求头（其余丢弃：防 Hop-by-hop / 伪装注入）。
/// x-bit-* 三件套为 bitsign-v1 信道签名（手机端生成 → Worker 透传 → 本地验签）
const PASS_HEADERS: [&str; 7] = [
    "authorization",
    "x-access-password",
    "content-type",
    "user-agent",
    "x-bit-sign",
    "x-bit-ts",
    "x-bit-nonce",
];

/// 生成 128 位识别码（32 hex 字符）：中继路由 + 访问凭据（二维码携带）
pub fn gen_relay_id() -> String {
    use rand::Rng;
    let b: [u8; 16] = rand::thread_rng().gen();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 本地回环基址：host 为通配/回环时用 127.0.0.1，否则用绑定的具体地址
fn loopback_base(host: &str, port: u16) -> String {
    let h = match host {
        "" | "0.0.0.0" | "::" => "127.0.0.1",
        x => x,
    };
    format!("http://{}", crate::config::join_host_port(h, port))
}

/// 中继常驻循环：随远程 HTTP 服务启停。每轮动态读配置——cloud_relay_url 与 relay_id
/// 均非空才发起连接（未配置时不产生任何外联流量），配置变更无需重启即生效
pub async fn run_loop(ctx: Arc<Ctx>) {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client");
    loop {
        let (base, rid, port, host, dev, bind) = {
            let c = ctx.config.lock().unwrap();
            (
                c.cloud_relay_url.clone().unwrap_or_default().trim_end_matches('/').to_string(),
                c.relay_id.clone(),
                c.port,
                c.host.clone(),
                // 设备指纹哈希（注册时定版落盘）：Worker 据此做 rid↔设备绑定与单设备
                // 识别码数上限（服务器侧防滥用）。未注册时为空，服务器会拒绝 poll
                c.device_fp_hash.clone().unwrap_or_default(),
                // 信道握手密钥（每识别码动态派生，经本认证通道下发给服务器）：
                // S = HMAC-SHA256(client_key, "bit-worker-bind:{rid}")。Worker 收到后才能
                // 验证手机端的挑战应答并签发连接许可（permit）——手机端从二维码内的
                // client_key 自行派生同一把 S，见 relay-worker/src/index.js 协议注释
                (!c.client_key.is_empty()).then(|| {
                    crate::security::hmac_sha256(
                        c.client_key.as_bytes(),
                        format!("bit-worker-bind:{rid_s}", rid_s = c.relay_id).as_bytes(),
                    ).iter().map(|b| format!("{b:02x}")).collect::<String>()
                }).unwrap_or_default(),
            )
        };
        if base.is_empty() || rid.is_empty() {
            // 未配置中继：静默等待（配置热生效，无需重启）
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }
        match poll_once(&client, &base, &rid, &dev, &bind).await {
            Ok(Some(req)) => {
                eprintln!("[BIT] relay tunnel <- {} {} (reqid {})", req.method, req.path, &req.rid[..8.min(req.rid.len())]);
                // 每个隧道请求独立任务并发回传：串行处理时一个用户被带宽限速的等待会
                // 卡住整个 poller，其他用户的请求全部排队——并发化后互不阻塞
                let client2 = client.clone();
                let ctx2 = ctx.clone();
                let base2 = base.clone();
                let rid2 = rid.clone();
                let host2 = host.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_and_answer(&client2, &ctx2, &base2, &rid2, req, port, &host2).await {
                        eprintln!("[BIT] relay serve failed: {e}");
                    }
                });
            }
            Ok(None) => { /* 空轮询：立即可再取，避免打爆服务器加微延迟 */ }
            Err(e) => {
                // 网络抖动 / 服务器暂不可达：退避重试
                eprintln!("[BIT] relay poll error: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

#[derive(Debug)]
struct TunnelReq {
    rid: String,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// Worker 按请求体探测的流式标记（OpenAI stream:true / SSE Accept）：true 时响应必须
    /// 走分块 answer（哪怕本地响应不是 SSE——如鉴权失败的 JSON，也要 last=true 收口）
    stream: bool,
    /// 手机端真实公网 IP（Worker 从 cf-connecting-ip 附带）：回环时注入 x-bit-client-ip，
    /// 本地 API 的 IP 黑名单 / 限速 / 并发上限按真实来源计数。手机端伪造不出该头
    /// （PASS_HEADERS 白名单不含它，只有本机 poller 会注入）
    client_ip: String,
    /// 许可签发时绑定的来源网段（Worker 从许可记录盖章，非现算）：回环时注入
    /// x-bit-permit-pfx，本地 API 比对其与 client_ip 的网段一致性——不一致 = 中继
    /// 实现被替换/破坏，拒绝放行。与 client_ip 同为 poller 专属注入，手机端伪造不了
    permit_pfx: String,
}

/// 长轮询一次：Some=取到隧道请求；None=无待处理（204）。
/// dev 为设备指纹哈希（x-bit-dev）：服务器做 rid↔设备绑定与单设备识别码上限。
/// bind 为信道握手密钥（x-bit-bind，每识别码动态派生）：服务器收到后才启用
/// 挑战-应答验证并向通过握手的手机端签发连接许可（permit）
async fn poll_once(
    client: &reqwest::Client,
    base: &str,
    rid: &str,
    dev: &str,
    bind: &str,
) -> Result<Option<TunnelReq>, String> {
    let mut rb = client.post(format!("{base}/relay/poll/{rid}"));
    if !dev.is_empty() {
        rb = rb.header("x-bit-dev", dev);
    }
    if !bind.is_empty() {
        rb = rb.header("x-bit-bind", bind);
    }
    let resp = rb
        .timeout(Duration::from_secs(30)) // 服务器最长持 25s
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status == 204 {
        return Ok(None);
    }
    if status != 200 {
        // 设备绑定被拒 / 被限流等：打印原因便于排查（服务器返回 JSON error）
        if let Ok(v) = resp.json::<serde_json::Value>().await {
            if let Some(err) = v["error"].as_str() {
                return Err(format!("poll HTTP {status}: {err}"));
            }
        }
        return Err(format!("poll HTTP {status}"));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let b64 = v["b"].as_str().unwrap_or("");
    let body = if b64.is_empty() {
        Vec::new()
    } else {
        base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| e.to_string())?
    };
    Ok(Some(TunnelReq {
        rid: v["rid"].as_str().unwrap_or_default().to_string(),
        method: v["m"].as_str().unwrap_or("GET").to_string(),
        path: v["p"].as_str().unwrap_or("/").to_string(),
        headers: v["h"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, val)| (k.clone(), val.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        body,
        stream: v["stream"].as_bool().unwrap_or(false),
        client_ip: v["ip"].as_str().unwrap_or_default().to_string(),
        permit_pfx: v["pfx"].as_str().unwrap_or_default().to_string(),
    }))
}

// ── 每用户响应带宽限速（relay_kbps_per_user，0=不限）──
// 中继是文字信道：默认 200 KB/s（≈0.2 MB/s）正常聊天完全无感，但借道拉大文件会被
// 摊平到预算内。令牌桶按手机端真实公网 IP 独立计数（每用户一份预算，互不挤兑），
// 允许 2 秒突发（起步即有 2×rate 余量），超发记账为"欠账"、下次发放前按速率还清。
// 等待在任务内异步进行（请求已并发化）：不阻塞 poller，也不阻塞其他用户。
struct BwBucket {
    tokens: f64,
    last: Instant,
}

fn bw_map() -> &'static Mutex<HashMap<String, BwBucket>> {
    static BW: OnceLock<Mutex<HashMap<String, BwBucket>>> = OnceLock::new();
    BW.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn pace_user_bw(ctx: &Ctx, key: &str, bytes: usize) {
    if bytes == 0 {
        return;
    }
    let kbps = ctx.config.lock().unwrap().relay_kbps_per_user;
    if kbps == 0 {
        return;
    }
    let rate = f64::from(kbps) * 1024.0; // B/s
    let cap = rate * 2.0; // 2 秒突发
    let wait = {
        let mut map = bw_map().lock().unwrap();
        // 防膨胀：键过多时清理 10 分钟未活跃的桶（手机端 IP 集合有限，常态远小于此）
        if map.len() > 512 {
            let stale: Vec<String> = map
                .iter()
                .filter(|(_, b)| b.last.elapsed().as_secs() > 600)
                .map(|(k, _)| k.clone())
                .collect();
            for k in stale {
                map.remove(&k);
            }
        }
        let now = Instant::now();
        let b = map.entry(key.to_string()).or_insert(BwBucket { tokens: cap, last: now });
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(cap);
        b.last = now;
        b.tokens -= bytes as f64; // 允许透支（欠账）
        if b.tokens < 0.0 { (-b.tokens) / rate } else { 0.0 }
    };
    // 上限 55s：压在 Worker 60s 手机等待超时之内，避免限速本身制造断流
    if wait > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(wait.min(55.0))).await;
    }
}

/// 回环调用本地 API 并把响应回传中继。认证头（client key / 访问密码）由手机端在隧道
/// 请求里携带，本地 API 的双重认证原样生效——中继只是转发管道。
/// 本机自加 x-bit-via / x-bit-rid 头：auth 中间件据此识别"经中继进入"并强制 bitsign 验签
/// （手机端伪造不出这两者——直连请求不带 via 头，走 LAN 通道不受影响）。
/// 流式响应（text/event-stream）逐块回传，手机端实时消费 OpenAI SSE 增量
async fn serve_and_answer(
    client: &reqwest::Client,
    ctx: &Arc<Ctx>,
    base: &str,
    rid: &str,
    req: TunnelReq,
    port: u16,
    host: &str,
) -> Result<(), String> {
    // 带宽限速键：手机端真实公网 IP（envelope.ip ← Worker cf-connecting-ip）——每用户
    // 独立预算；缺失时退化为中继识别码（同 rid 共享一份预算）
    let bw_key = if req.client_ip.is_empty() { rid.to_string() } else { req.client_ip.clone() };
    if req.body.len() > BODY_MAX {
        return answer(client, base, rid, &req.rid, 413, &[], b"payload too large".to_vec()).await;
    }
    let path = if req.path.starts_with('/') { req.path.clone() } else { format!("/{}", req.path) };
    let url = format!("{}{}", loopback_base(host, port), path);
    let mut r = client.request(
        reqwest::Method::from_bytes(req.method.as_bytes()).unwrap_or(reqwest::Method::GET),
        &url,
    );
    for (k, v) in &req.headers {
        let kl = k.to_lowercase();
        if PASS_HEADERS.contains(&kl.as_str()) && !v.is_empty() {
            r = r.header(k, v);
        }
    }
    r = r.header("x-bit-via", "relay").header("x-bit-rid", rid);
    // 手机端真实 IP 注入（envelope.ip ← Worker cf-connecting-ip）：本地按真实来源做
    // 黑名单 / 限速。该头不在 PASS_HEADERS 白名单里，手机端无法自带伪造
    if !req.client_ip.is_empty() {
        r = r.header("x-bit-client-ip", &req.client_ip);
    }
    // 许可网段盖章注入（envelope.pfx ← Worker 许可记录）：本地比对来源一致性，
    // 不一致拒绝放行。同属 poller 专属注入，手机端无法伪造
    if !req.permit_pfx.is_empty() {
        r = r.header("x-bit-permit-pfx", &req.permit_pfx);
    }
    if !req.body.is_empty() {
        r = r.body(req.body.clone());
    }
    // 回环调用必须带超时：本地 API 一旦无响应，显式 502 回传中继并让循环继续，
    // 否则整个 poller 无声挂死、后续隧道请求全部无人应答且不留任何日志
    let resp = match r.timeout(Duration::from_secs(30)).send().await {
        Ok(resp) => resp,
        Err(e) => {
            let msg = serde_json::json!({ "error": "local api unreachable", "detail": e.to_string() }).to_string();
            return answer(
                client,
                base,
                rid,
                &req.rid,
                502,
                &[("content-type".into(), "application/json".into())],
                msg.into_bytes(),
            )
            .await;
        }
    };
    let status = resp.status().as_u16();
    let mut headers = Vec::new();
    if let Some(ct) = resp.headers().get("content-type") {
        headers.push(("content-type".into(), ct.to_str().unwrap_or_default().into()));
    }
    let is_sse = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("text/event-stream"))
        .unwrap_or(false);
    if !req.stream {
        // 非流式隧道：一次性完整回传（与旧协议一致）
        let body = resp.bytes().await.map_err(|e| e.to_string())?;
        if body.len() > BODY_MAX {
            return answer(client, base, rid, &req.rid, 502, &[], b"response too large".to_vec()).await;
        }
        pace_user_bw(ctx, &bw_key, body.len()).await;
        return answer(client, base, rid, &req.rid, status, &headers, body.to_vec()).await;
    }
    // 流式隧道（Worker 已开 TransformStream 通道）：必须分块回传并以 last=true 收口，
    // 否则手机端连接悬挂到空闲超时
    if !is_sse {
        // 本地响应不是 SSE（如鉴权失败的 JSON）：整体作为一块发出并收口
        let body = resp.bytes().await.map_err(|e| e.to_string())?;
        if body.len() > BODY_MAX {
            return answer_chunk(client, base, rid, &req.rid, 0, true, b"response too large").await;
        }
        pace_user_bw(ctx, &bw_key, body.len()).await;
        return answer_chunk(client, base, rid, &req.rid, 0, true, &body).await;
    }
    // SSE：逐块增量回传，手机端按 OpenAI SSE 实时消费——与直连本地 API 的流式体验一致
    let mut stream = resp.bytes_stream();
    let mut seq = 0u64;
    loop {
        match futures_util::StreamExt::next(&mut stream).await {
            Some(Ok(c)) => {
                if c.len() > BODY_MAX {
                    return answer_chunk(client, base, rid, &req.rid, seq, true, b"response too large").await;
                }
                pace_user_bw(ctx, &bw_key, c.len()).await;
                if answer_chunk(client, base, rid, &req.rid, seq, false, &c).await.is_err() {
                    return Err("stream chunk answer failed".into());
                }
                seq += 1;
            }
            Some(Err(e)) => {
                // 上游读取出错：错误说明作为最后一块发出，手机端感知断流
                let _ = answer_chunk(client, base, rid, &req.rid, seq, true, format!("\n[BIT stream error: {e}]\n").as_bytes()).await;
                return Err(format!("stream read failed: {e}"));
            }
            None => break,
        }
    }
    answer_chunk(client, base, rid, &req.rid, seq, true, &[]).await
}

/// 回传响应给中继
async fn answer(
    client: &reqwest::Client,
    base: &str,
    rid: &str,
    reqid: &str,
    status: u16,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<(), String> {
    let h: serde_json::Map<String, serde_json::Value> = headers
        .iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    let payload = json!({
        "rid": reqid,
        "s": status,
        "h": h,
        "b": base64::engine::general_purpose::STANDARD.encode(&body),
    });
    answer_post(client, base, rid, &payload).await
}

/// 回传一个流式分块给中继（seq 递增；last=true 收口关闭手机端响应流）
async fn answer_chunk(
    client: &reqwest::Client,
    base: &str,
    rid: &str,
    reqid: &str,
    seq: u64,
    last: bool,
    chunk: &[u8],
) -> Result<(), String> {
    let payload = json!({
        "rid": reqid,
        "seq": seq,
        "last": last,
        "b": base64::engine::general_purpose::STANDARD.encode(chunk),
    });
    answer_post(client, base, rid, &payload).await
}

/// answer POST 共用：15s 超时，非 2xx 视为失败
async fn answer_post(
    client: &reqwest::Client,
    base: &str,
    rid: &str,
    payload: &serde_json::Value,
) -> Result<(), String> {
    client
        .post(format!("{base}/relay/answer/{rid}"))
        .timeout(Duration::from_secs(15))
        .json(payload)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    Ok(())
}
