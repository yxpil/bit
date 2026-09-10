// yxpil · BIT
// Agent 引擎进程化：对话回合 / 工具执行 / 审批 / 后台 shell 搬进独立子进程（同 exe --agent-worker）。
//
// 拓扑：
//   宿主（GUI）：UI / 托盘 / 远程 HTTP / Autopilot / 守护。经 localhost HTTP 把对话请求转发给 worker。
//   worker（无 UI 头less）：Ctx::load 完整重建，跑 chat_turn* / execute_tool_call / shellbg / 审批等待。
//
// IPC（全部只绑 127.0.0.1，随机 128-bit token 鉴权，经环境变量下发给 worker）：
//   宿主 → worker：POST /w/ping | /w/chat_stream | /w/chat | /w/chat_single | /w/interrupt | /w/approve | /w/approvals | /w/reload
//   worker → 宿主：POST /host-event（UI 事件转发，宿主收到后 emit 给 webview）
//
// 容错（监督循环）：
//   - 子进程退出 / ping 失败 → 杀掉重启（指数退避）；
//   - 连续 3 次起不来 → 回退进程内模式（对调用方透明），每 60 秒重试恢复；
//   - 回合在飞但超过 worker_hang_secs 无任何事件 → 判定卡死，杀掉重启（自动恢复，不再永久挂）。
//   代理请求失败时对调用方透明回退进程内执行，功能不中断。
use crate::state::Ctx;
use serde_json::json;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// 宿主拉起 worker 时追加的命令行参数（也用于日志标识）
pub const WORKER_FLAG: &str = "--agent-worker";

/// 当前进程是否为 agent worker 子进程（main.rs setup 早期置位）
pub static IN_WORKER: AtomicBool = AtomicBool::new(false);
/// 宿主侧：worker 是否健康可用（false = 进程内回退模式）
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// worker 是否有对话回合在飞（监督循环 ping 缓存，托盘状态展示用）
pub static BUSY: AtomicBool = AtomicBool::new(false);
static CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);
static TOKEN: OnceLock<String> = OnceLock::new();
static PORT: AtomicU32 = AtomicU32::new(0);
static FAILS: AtomicU32 = AtomicU32::new(0);
/// 宿主事件接收器地址（worker 转发 UI 事件的目标）
static EVENT_SINK_URL: OnceLock<String> = OnceLock::new();
/// worker 侧：UI 事件转发通道（单消费者保序）
static EV_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<(String, serde_json::Value)>> = OnceLock::new();
/// worker 侧：回合在飞起始时刻 / 最后一次 UI 事件时刻（宿主卡死检测数据源）
static TURN_SINCE: Mutex<Option<std::time::Instant>> = Mutex::new(None);
static LAST_EMIT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

fn token() -> &'static str {
    TOKEN.get_or_init(new_token)
}

fn new_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..32).map(|_| format!("{:x}", rng.gen::<u8>())).collect()
}

/// 长请求客户端（对话回合可能跑几十分钟：auto_drive 连续多轮 + 长工具）
fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(6 * 3600))
        .build()
        .unwrap_or_default()
}

/// 健康检查等短请求客户端
fn http_quick() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default()
}

fn worker_url(path: &str) -> String {
    format!("http://127.0.0.1:{}/{}", PORT.load(Ordering::Relaxed), path.trim_start_matches('/'))
}

pub fn enabled(ctx: &Arc<Ctx>) -> bool {
    ctx.config.lock().unwrap().agent_worker_enabled
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed) && PORT.load(Ordering::Relaxed) != 0
}

// ============================================================================================
// UI 事件发射（双进程统一入口）：宿主进程直发 webview；worker 进程转发给宿主代发。
// agent / registry / shellbg / state 里的 ctx.app.emit 一律改走这里。
// ============================================================================================
pub fn emit_ui(app: &tauri::AppHandle, name: &str, payload: serde_json::Value) {
    if IN_WORKER.load(Ordering::Relaxed) {
        *LAST_EMIT.lock().unwrap() = Some(std::time::Instant::now());
        if let Some(tx) = EV_TX.get() {
            let _ = tx.send((name.to_string(), payload));
        }
    } else {
        use tauri::Emitter;
        let _ = app.emit(name, payload);
    }
}

/// 宿主侧：worker 起来/回退/重启后通知前端
fn emit_worker_state(ctx: &Arc<Ctx>, state: &str, detail: serde_json::Value) {
    use tauri::Emitter;
    let _ = ctx
        .app
        .emit("agent-worker", json!({ "state": state, "detail": detail }));
}

// ============================================================================================
// 宿主侧：监督 + 代理
// ============================================================================================

/// 宿主启动时调用（仅桌面主进程）：起事件接收器 + 监督循环
pub fn boot_host(ctx: &Arc<Ctx>) {
    let c = ctx.clone();
    tauri::async_runtime::spawn(async move {
        match start_event_sink(&c).await {
            Ok(url) => {
                EVENT_SINK_URL.set(url).ok();
                supervise(c).await;
            }
            Err(e) => {
                // 接收器起不来：永回进程内模式（active() 恒 false，调用方走原路径）
                crate::audit::record(&c, "host", "worker.boot", "event-sink", json!({ "error": e }), false);
            }
        }
    });
}

/// 事件接收器：127.0.0.1 随机端口；收 worker 的 UI 事件 → 宿主 emit 给 webview
async fn start_event_sink(ctx: &Arc<Ctx>) -> Result<String, String> {
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?.to_string();
    let token = token().to_string();
    let app = ctx.app.clone();

    async fn receive(
        State((app, token)): State<(tauri::AppHandle, String)>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> StatusCode {
        if headers.get("x-bit-worker-token").and_then(|v| v.to_str().ok()) != Some(token.as_str()) {
            return StatusCode::UNAUTHORIZED;
        }
        let (Some(name), Some(payload)) = (body.get("name").and_then(|v| v.as_str()), body.get("payload").cloned()) else {
            return StatusCode::BAD_REQUEST;
        };
        use tauri::Emitter;
        let _ = app.emit(name, payload);
        StatusCode::OK
    }

    let router = Router::new()
        .route("/host-event", post(receive))
        .with_state((app, token));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(format!("http://{addr}/host-event"))
}

/// worker ping 返回：健康 + 回合在飞 + 最后事件距今毫秒
struct PingInfo {
    turn_in_flight: bool,
    idle_ms: u128,
}

async fn ping() -> Result<PingInfo, String> {
    let v: serde_json::Value = http_quick()
        .post(worker_url("/w/ping"))
        .header("x-bit-worker-token", token())
        .json(&json!({}))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(PingInfo {
        turn_in_flight: v.get("turn_in_flight").and_then(|x| x.as_bool()).unwrap_or(false),
        idle_ms: v.get("idle_ms").and_then(|x| x.as_u64()).unwrap_or(u64::MAX) as u128,
    })
}

fn kill_child() {
    if let Some(mut c) = CHILD.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// 拉起 worker 子进程并完成握手（stdout 首行 BIT_WORKER_READY <port>）
async fn ensure_spawned(ctx: &Arc<Ctx>) -> Result<(), String> {
    kill_child();
    let Some(sink) = EVENT_SINK_URL.get() else {
        return Err("event sink not ready".into());
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::channel::<Result<u16, String>>();
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg(WORKER_FLAG)
        .env("BIT_WORKER_TOKEN", token())
        .env("BIT_HOST_EVENT_URL", sink)
        .env("BIT_HOST_PID", std::process::id().to_string())
        .env("BIT_DATA_DIR", ctx.data_dir.to_string_lossy().to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        // debug 构建是控制台子系统：防每次拉起 worker 闪黑窗（release GUI 子系统本就无窗口）
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    // 握手线程：读首行 READY 行（阻塞线程，不阻塞异步运行时）
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        let mut acc = Vec::new();
        loop {
            match std::io::Read::read(&mut stdout, &mut buf) {
                Ok(0) => {
                    let _ = tx.send(Err("worker exited before ready".into()));
                    break;
                }
                Ok(n) => {
                    acc.extend_from_slice(&buf[..n]);
                    if let Some(pos) = acc.iter().position(|&b| b == b'\n') {
                        let line = String::from_utf8_lossy(&acc[..pos]).to_string();
                        let _ = tx.send(match line.strip_prefix("BIT_WORKER_READY ") {
                            Some(p) => p.trim().parse::<u16>().map_err(|e| e.to_string()),
                            None => Err(format!("unexpected first line: {line}")),
                        });
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    break;
                }
            }
        }
    });
    // worker 启动含状态文件加载，给 30 秒握手窗口（阻塞放 blocking 线程池）
    let port = tokio::task::spawn_blocking(move || {
        rx.recv_timeout(Duration::from_secs(30)).map_err(|e| e.to_string()).and_then(|r| r)
    })
    .await
    .map_err(|e| e.to_string())??;
    PORT.store(port as u32, Ordering::Relaxed);
    *CHILD.lock().unwrap() = Some(child);
    // 就绪后立刻 ping 一次确认可服务
                ping().await.map(|_| ()).map_err(|e| format!("first ping: {e}"))
}

/// 宿主侧汇总：是否有对话回合在跑（进程内回合看 turn_locks，worker 回合看 ping 缓存）
pub fn busy(ctx: &Arc<Ctx>) -> bool {
    if IN_WORKER.load(Ordering::Relaxed) {
        return TURN_SINCE.lock().unwrap().is_some();
    }
    if BUSY.load(Ordering::Relaxed) {
        return true;
    }
    !ctx.turn_locks.lock().unwrap().is_empty()
}

/// 监督循环：保活 + 健康检查 + 卡死检测，全程静默自愈
async fn supervise(ctx: Arc<Ctx>) {
    let mut backoff = 1u64;
    loop {
        tokio::time::sleep(Duration::from_secs(if active() { 5 } else { backoff.min(60) })).await;
        if !enabled(&ctx) {
            if ACTIVE.swap(false, Ordering::Relaxed) {
                kill_child();
                emit_worker_state(&ctx, "disabled", json!({}));
            }
            backoff = 1;
            continue;
        }
        if active() {
            // 健康检查 + 卡死检测
            match ping().await {
                Ok(p) => {
                    FAILS.store(0, Ordering::Relaxed);
                    backoff = 1;
                    BUSY.store(p.turn_in_flight, Ordering::Relaxed);
                    let hang_secs = { ctx.config.lock().unwrap().worker_hang_secs } as u128;
                    if p.turn_in_flight && hang_secs > 0 && p.idle_ms > hang_secs * 1000 {
                        crate::audit::record(
                            &ctx,
                            "host",
                            "worker.hang_kill",
                            "agent-worker",
                            json!({ "idle_ms": p.idle_ms, "hang_secs": hang_secs }),
                            false,
                        );
                        kill_child();
                        ACTIVE.store(false, Ordering::Relaxed);
                        emit_worker_state(&ctx, "restarting", json!({ "reason": "hang" }));
                    }
                }
                Err(_) => {
                    // ping 失败：worker 可能已死，杀掉重拉
                    kill_child();
                    ACTIVE.store(false, Ordering::Relaxed);
                    BUSY.store(false, Ordering::Relaxed);
                }
            }
            continue;
        }
        // 不在运行：尝试拉起
        match ensure_spawned(&ctx).await {
            Ok(()) => {
                FAILS.store(0, Ordering::Relaxed);
                backoff = 1;
                ACTIVE.store(true, Ordering::Relaxed);
                emit_worker_state(&ctx, "running", json!({ "pid": CHILD.lock().unwrap().as_ref().map(|c| c.id()) }));
                crate::audit::record(&ctx, "host", "worker.started", "agent-worker", json!({}), true);
            }
            Err(e) => {
                let fails = FAILS.fetch_add(1, Ordering::Relaxed) + 1;
                backoff = (backoff * 2).min(60);
                if fails == 3 {
                    // 连续 3 次起不来：回退进程内模式（每 60s 重试恢复），功能不断
                    emit_worker_state(&ctx, "fallback", json!({ "error": e }));
                    crate::audit::record(&ctx, "host", "worker.fallback", "agent-worker", json!({ "error": e }), false);
                }
            }
        }
    }
}

// ---------- 宿主 → worker 代理 ----------

async fn post_worker(path: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    if !active() {
        return Err("worker not active".into());
    }
    let resp = http()
        .post(worker_url(path))
        .header("x-bit-worker-token", token())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("worker unreachable: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(v
            .get("error")
            .and_then(|x| x.as_str())
            .unwrap_or("worker error")
            .to_string());
    }
    Ok(v)
}

/// 保存配置 / 工具等状态后通知 worker 重读磁盘（fire-and-forget）
pub fn notify_reload() {
    if IN_WORKER.load(Ordering::Relaxed) || !active() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let _ = post_worker("/w/reload", json!({})).await;
    });
}

pub async fn proxy_chat_stream(session_id: &str, message: &str, event_name: &str, images: Vec<String>) -> Result<serde_json::Value, String> {
    post_worker(
        "/w/chat_stream",
        json!({ "session_id": session_id, "message": message, "event_name": event_name, "images": images }),
    )
    .await
}

/// auto 端点：worker 侧跑 chat_turn_auto（auto_drive 循环在 worker 内完成）
pub async fn proxy_chat(session_id: &str, message: &str, images: Vec<String>) -> Result<serde_json::Value, String> {
    post_worker("/w/chat", json!({ "session_id": session_id, "message": message, "images": images })).await
}

/// 单回合端点：worker 侧跑 chat_turn
pub async fn proxy_chat_single(session_id: &str, message: &str, images: Vec<String>) -> Result<serde_json::Value, String> {
    post_worker("/w/chat_single", json!({ "session_id": session_id, "message": message, "images": images })).await
}

pub async fn proxy_interrupt(session_id: &str) -> Result<serde_json::Value, String> {
    post_worker("/w/interrupt", json!({ "session_id": session_id })).await
}

pub async fn proxy_approve(id: &str, allow: bool) -> Result<serde_json::Value, String> {
    post_worker("/w/approve", json!({ "id": id, "allow": allow })).await
}

pub async fn proxy_list_approvals() -> Result<serde_json::Value, String> {
    post_worker("/w/approvals", json!({})).await
}

/// 后台 shell 作业：列表 / 取消。作业登记表是每进程独立的全局表，
/// AI 在 worker 里起的命令登记在 worker 侧，host 直接查本地表必落空（点了终止没反应）
pub async fn proxy_shell_list() -> Result<serde_json::Value, String> {
    post_worker("/w/shells", json!({})).await
}

pub async fn proxy_shell_cancel(id: &str) -> Result<serde_json::Value, String> {
    post_worker("/w/shell_cancel", json!({ "id": id })).await
}

// ============================================================================================
// worker 侧：无 UI 引擎服务
// ============================================================================================

/// worker 主服务：绑定 127.0.0.1 随机端口，stdout 握手后常驻
pub async fn serve(ctx: Arc<Ctx>) -> Result<(), String> {
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};

    let expect_token = std::env::var("BIT_WORKER_TOKEN").unwrap_or_default();
    if expect_token.is_empty() {
        return Err("BIT_WORKER_TOKEN missing; refusing to serve".into());
    }
    // 宿主退出检测：宿主 PID 消失后自杀，防孤儿泄漏
    if let Ok(pid) = std::env::var("BIT_HOST_PID") {
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                if !host_alive(&pid) {
                    std::process::exit(0);
                }
            }
        });
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    // 事件转发通道：emit_ui 产出的 UI 事件按序 POST 回宿主
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, serde_json::Value)>();
    EV_TX.set(tx).ok();
    let sink = std::env::var("BIT_HOST_EVENT_URL").unwrap_or_default();
    let sink_token = expect_token.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap_or_default();
        while let Some((name, payload)) = rx.recv().await {
            if sink.is_empty() {
                continue;
            }
            let _ = client
                .post(&sink)
                .header("x-bit-worker-token", &sink_token)
                .json(&json!({ "name": name, "payload": payload }))
                .send()
                .await;
        }
    });

    // 就绪握手：宿主从 stdout 读到端口号后开始转发请求
    println!("BIT_WORKER_READY {port}");
    let _ = std::io::stdout().flush();

    #[derive(Clone)]
    struct St {
        ctx: Arc<Ctx>,
        token: String,
    }

    async fn auth(
        State(st): State<St>,
        headers: HeaderMap,
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        if headers.get("x-bit-worker-token").and_then(|v| v.to_str().ok()) != Some(st.token.as_str()) {
            return (StatusCode::UNAUTHORIZED, "forbidden").into_response();
        }
        next.run(req).await
    }

    async fn ping(State(_st): State<St>) -> Json<serde_json::Value> {
        let idle_ms = LAST_EMIT
            .lock()
            .unwrap()
            .map(|t| t.elapsed().as_millis())
            .unwrap_or(u64::MAX as u128);
        Json(json!({ "ok": true, "turn_in_flight": TURN_SINCE.lock().unwrap().is_some(), "idle_ms": idle_ms }))
    }

    async fn chat_stream(
        State(st): State<St>,
        Json(body): Json<serde_json::Value>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
        let sid = body.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let msg = body.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let ev = body.get("event_name").and_then(|v| v.as_str()).unwrap_or("chat-stream").to_string();
        let images: Vec<String> = body
            .get("images")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        *TURN_SINCE.lock().unwrap() = Some(std::time::Instant::now());
        ensure_session(&st.ctx, &sid);
        let r = crate::agent::chat_turn_stream_auto(&st.ctx, &sid, &msg, &ev, images).await;
        *TURN_SINCE.lock().unwrap() = None;
        r.map(|msgs| Json(json!({ "messages": msgs })))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))))
    }

    async fn chat(
        State(st): State<St>,
        Json(body): Json<serde_json::Value>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
        let sid = body.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let msg = body.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let images: Vec<String> = body
            .get("images")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        *TURN_SINCE.lock().unwrap() = Some(std::time::Instant::now());
        ensure_session(&st.ctx, &sid);
        let r = crate::agent::chat_turn_auto(&st.ctx, &sid, &msg, images).await;
        *TURN_SINCE.lock().unwrap() = None;
        r.map(|msgs| Json(json!({ "messages": msgs })))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))))
    }

    async fn chat_single(
        State(st): State<St>,
        Json(body): Json<serde_json::Value>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
        let sid = body.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let msg = body.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let images: Vec<String> = body
            .get("images")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        *TURN_SINCE.lock().unwrap() = Some(std::time::Instant::now());
        ensure_session(&st.ctx, &sid);
        let r = crate::agent::chat_turn(&st.ctx, &sid, &msg, images).await;
        *TURN_SINCE.lock().unwrap() = None;
        r.map(|msgs| Json(json!({ "messages": msgs })))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))))
    }

    /// 会话缺失自愈：worker 只在启动时加载 sessions.json，host 之后新建的会话
    /// 它是看不见的 → 对话必报"会话不存在" → host 回退进程内执行（审批/中断表分裂的根源）。
    /// 这里在跑回合前发现会话缺失就从磁盘合并一次（mtime 守卫，正常路径零开销）。
    fn ensure_session(ctx: &Arc<Ctx>, sid: &str) {
        if sid.is_empty() {
            return;
        }
        let exists = ctx.sessions.lock().unwrap().get_mut(sid).is_some();
        if !exists {
            crate::session::refresh_from_disk(ctx);
        }
    }

    async fn interrupt(State(st): State<St>, Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
        let sid = body.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let sid = if sid.is_empty() { st.ctx.sessions.lock().unwrap().active.clone() } else { sid };
        let hit = {
            let map = st.ctx.interrupts.lock().unwrap();
            match map.get(&sid) {
                Some(flag) => {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                    true
                }
                None => false,
            }
        };
        Json(json!({ "id": sid, "interrupted": hit }))
    }

    async fn approve(
        State(st): State<St>,
        Json(body): Json<serde_json::Value>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
        let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let allow = body.get("allow").and_then(|v| v.as_bool()).unwrap_or(false);
        let sender = st.ctx.approvals.lock().unwrap().remove(&id).map(|p| p.tx);
        match sender {
            Some(tx) => {
                let _ = tx.send(allow);
                Ok(Json(json!({ "id": id, "allow": allow })))
            }
            None => Err((StatusCode::NOT_FOUND, Json(json!({ "error": "审批请求不存在或已处理" })))),
        }
    }

    async fn approvals(State(st): State<St>) -> Json<serde_json::Value> {
        let map = st.ctx.approvals.lock().unwrap();
        let items: Vec<serde_json::Value> = map
            .iter()
            .map(|(id, p)| {
                json!({
                    "id": id,
                    "tool": p.tool,
                    "params": p.params,
                    "age_secs": p.created.elapsed().as_secs(),
                })
            })
            .collect();
        Json(json!({ "approvals": items }))
    }

    async fn reload(State(st): State<St>) -> Json<serde_json::Value> {
        reload_state(&st.ctx);
        Json(json!({ "ok": true }))
    }

    /// 后台 shell 作业列表（worker 侧登记表）
    async fn shells() -> Json<serde_json::Value> {
        Json(crate::shellbg::list())
    }

    /// 取消 worker 侧登记的后台 shell 作业
    async fn shell_cancel(State(st): State<St>, Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
        let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let ok = crate::shellbg::cancel(&id);
        crate::audit::record(&st.ctx, "local-user", "shell.cancel_request", &id, json!({ "via": "worker" }), ok);
        Json(json!({ "cancelled": ok, "job_id": id }))
    }

    let st = St { ctx, token: expect_token };
    let app = Router::new()
        .route("/w/ping", post(ping))
        .route("/w/chat_stream", post(chat_stream))
        .route("/w/chat", post(chat))
        .route("/w/chat_single", post(chat_single))
        .route("/w/interrupt", post(interrupt))
        .route("/w/approve", post(approve))
        .route("/w/approvals", post(approvals))
        .route("/w/reload", post(reload))
        .route("/w/shells", post(shells))
        .route("/w/shell_cancel", post(shell_cancel))
        .layer(axum::middleware::from_fn_with_state(st.clone(), auth))
        .with_state(st);
    axum::serve(listener, app).await.map_err(|e| e.to_string())
}

/// 宿主 PID 存活检测（worker 防孤儿）
fn host_alive(pid: &str) -> bool {
    let Ok(p) = pid.parse::<u32>() else { return true };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // tasklist 精确匹配 PID；查无此进程即宿主已退出
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {p}"), "/FO", "CSV", "/NH"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
        matches!(out, Ok(o) if String::from_utf8_lossy(&o.stdout).contains(&format!("\"{p}\"")))
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(&format!("/proc/{p}")).exists()
    }
}

/// worker 收到 reload：从磁盘重读配置 / 模型配置 / 工具 / 技能 / 记忆 / 目标 / 待办
fn reload_state(ctx: &Arc<Ctx>) {
    use crate::state::read_json;
    let dir = ctx.data_dir.clone();
    *ctx.config.lock().unwrap() = crate::config::Config::load(&dir);
    *ctx.ai_config.lock().unwrap() = read_json(&dir.join("ai_config.json")).unwrap_or_default();
    let tools: Vec<crate::registry::ToolDef> = read_json(&dir.join("tools.json")).unwrap_or_default();
    let tools = {
        let builtin = crate::registry::builtin_tools();
        let builtin_names: std::collections::HashSet<String> = builtin.iter().map(|t| t.name.clone()).collect();
        let mut custom: Vec<crate::registry::ToolDef> = tools
            .into_iter()
            .filter(|t| !matches!(t.kind, crate::registry::ToolKind::Builtin { .. }) && !builtin_names.contains(&t.name))
            .collect();
        let mut merged = builtin;
        merged.append(&mut custom);
        merged
    };
    *ctx.tools.lock().unwrap() = tools;
    *ctx.skills.lock().unwrap() = read_json(&dir.join("skills.json")).unwrap_or_default();
    *ctx.memories.lock().unwrap() = read_json(&dir.join("memories.json")).unwrap_or_default();
    *ctx.goals.lock().unwrap() = read_json(&dir.join("goals.json")).unwrap_or_default();
    *ctx.todos.lock().unwrap() = read_json(&dir.join("todos.json")).unwrap_or_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_url_uses_port() {
        PORT.store(34567, Ordering::Relaxed);
        assert_eq!(worker_url("/w/ping"), "http://127.0.0.1:34567/w/ping");
        PORT.store(0, Ordering::Relaxed);
    }

    #[test]
    fn host_mode_default_inactive() {
        assert!(!IN_WORKER.load(Ordering::Relaxed));
    }
}
