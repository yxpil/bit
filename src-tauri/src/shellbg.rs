// yxpil · BIT
// 后台 shell（长命令异步化）：
//   - shell 工具调用若在「前台窗口」内未结束，自动转入后台作业（job）；
//   - 转后台立即返回 { status:"background", job_id }，AI 回合不再干等；
//   - 命令生命周期全程广播 `shell-job` 事件（started / done / killed），供 UI 面板展示；
//   - 命令自然结束时，若所属会话空闲，把结果作为新消息自动唤回该会话的 AI 继续处理；
//     会话忙则先把结果注入会话历史，等下一次上下文自然读到。
//   - 提供 cancel / list / find_running：用户可手动停止，重复命令会被拦截。
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::sync::Notify;

/// 前台判定窗口：命令在窗口内结束走原有「快命令」路径；否则转后台。
const FRONT_WINDOW_MS: u128 = 2000;
/// 后台命令硬上限（小时）：防止程序失控后作业永久挂起泄漏。正常作业由用户/结果终止。
const BG_TIMEOUT_SECS: u64 = 6 * 3600;

/// 一条自然结束的后台命令：交给顶层续跑 worker，把结果唤回所属会话的 AI。
/// 之所以走 channel 而不是在 run/finish 链里直接 await agent 回合：
/// agent 回合最终又会经过 builtin_invoke 的 shell 分支（spawn(run)），若在 run 链内 await 会形成
/// 类型级的无限递归（E0391: opaque future not Send）。顶层 worker 是进程启动时单独 spawn 的，
/// 与 run 链无类型依赖，彻底断开递归。
pub struct JobDone {
    pub session: String,
    pub job_id: String,
    pub command: String,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// 结束方式：done=自然结束 / cancelled=用户手动停止 / timeout=运行超时被强制终止。
    /// 三种结束都要告知所属会话的 AI——尤其 cancelled 必须让 AI 知道「是用户主动干预」，
    /// 而不是误以为任务失败或仍在运行。
    pub reason: String,
}

static DONE_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<JobDone>> = OnceLock::new();

pub struct ShellJob {
    pub id: String,
    /// 发起命令的会话 id：完成后要唤回这个会话
    pub session: Option<String>,
    pub command: String,
    pub cwd: Option<String>,
    pub started: std::time::Instant,
    /// cancel() 时 notify 一次，后台等待任务随即 kill 进程
    pub cancel: Arc<Notify>,
    /// 后台进程句柄（仅 finish 任务取走）
    pub child: Mutex<Option<Child>>,
}

fn jobs() -> &'static Mutex<HashMap<String, Arc<ShellJob>>> {
    static J: OnceLock<Mutex<HashMap<String, Arc<ShellJob>>>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    format!("sh{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 构造 shell 命令：按 config.default_shell 解析（空 = 自动识别，指定不存在时回退自动）。
/// PowerShell 系沿用强制 UTF-8（与旧实现一致），Unix 用解析出的 -c 系 shell。
fn shell_command(pref: &str, command: &str, cwd: Option<&str>) -> tokio::process::Command {
    let (prog, args) = match crate::toolenv::resolve_shell(pref) {
        Ok(v) => v,
        Err(_) => crate::toolenv::resolve_shell("")
            .unwrap_or_else(|_| ("powershell".to_string(), vec!["-Command".to_string()])),
    };
    let is_ps = prog
        .rsplit(['/', '\\'])
        .next()
        .map(|b| b == "pwsh" || b == "powershell")
        .unwrap_or(false);
    // PowerShell 强制 UTF-8 输出，避免中文乱码
    let full = if is_ps {
        format!("[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; {command}")
    } else {
        command.to_string()
    };
    let mut c = tokio::process::Command::new(prog);
    c.args(args);
    c.arg(full);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    crate::registry::no_window_tokio(&mut c);
    c
}

/// 解析出的默认 shell 是否为 PowerShell 系（裸 `&` 语义判断需要）
fn shell_is_ps(pref: &str) -> bool {
    let (prog, _) = match crate::toolenv::resolve_shell(pref) {
        Ok(v) => v,
        Err(_) => return true, // 两次解析都失败时的兜底就是 powershell
    };
    prog.rsplit(['/', '\\'])
        .next()
        .map(|b| b == "pwsh" || b == "powershell")
        .unwrap_or(false)
}

/// PowerShell 裸 `&` 检测（引号感知）。`a & b` 在 PS 里是后台 Job 操作符：
/// 前序命令输出进 Job 表丢失、退出码恒 0（假成功）。合法用法需排除：
/// `&&`（逻辑与）、`2>&1` / `&>`（重定向）、语句开头的调用操作符（`& script.ps1`）。
/// 判定：引号外的 `&`，所在语句段已有内容，且其后是空白/串尾 → Job 操作符。
fn ps_bare_ampersand(cmd: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut seg_has_content = false; // 自 ; | ( { 或串首以来是否已出现非空白
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else {
            match c {
                '\'' | '"' => quote = Some(c),
                ';' | '|' | '(' | '{' | '\n' => seg_has_content = false,
                '&' => {
                    let prev = if i > 0 { chars[i - 1] } else { '\0' };
                    let next = chars.get(i + 1).copied().unwrap_or('\0');
                    let is_chain = prev == '&' || next == '&';
                    let is_redirect = prev == '>' || next == '>';
                    if !is_chain && !is_redirect && seg_has_content {
                        return true;
                    }
                    if next == '&' {
                        i += 1; // && 的第二个 & 无需复判
                    }
                }
                c if c.is_whitespace() => {}
                _ => seg_has_content = true,
            }
        }
        i += 1;
    }
    false
}

fn emit(ctx: &Arc<crate::state::Ctx>, phase: &str, job: &ShellJob, extra: Option<serde_json::Value>) {
    use tauri::Emitter;
    let mut payload = json!({
        "phase": phase,
        "job_id": job.id,
        "command": job.command,
        "cwd": job.cwd,
        "session_id": job.session,
        "at": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    });
    if let (Some(obj), Some(ext)) =
        (payload.as_object_mut(), extra.as_ref().and_then(|v| v.as_object()))
    {
        for (k, v) in ext {
            obj.insert(k.clone(), v.clone());
        }
    }
    let _ = crate::worker::emit_ui(&ctx.app, "shell-job", payload);
}

/// shell 工具入口：短命令照旧秒回；超过前台窗口的命令转后台（含登记 + 事件 + 自动唤回）。
/// force_background=true（AI 显式标记长任务）时跳过前台窗口，spawn 后直接转后台。
pub async fn run(
    ctx: &Arc<crate::state::Ctx>,
    command: &str,
    cwd: Option<&str>,
    session: Option<&str>,
    force_background: bool,
) -> Result<serde_json::Value, String> {
    // 重复命令拦截：同一会话同一命令正在后台跑时，不重复执行，提示等待或停止
    if let Some(jid) = find_running(session, command) {
        return Err(format!(
            "命令已在后台运行（job {jid}）：`{}`。请等它结束，或发送「停止 {jid}」取消它，不要重复执行同一命令。",
            crate::registry::safe_trunc(command, 120)
        ));
    }
    // 快照默认 shell 后立即释放配置锁（锁序纪律：不跨 spawn 持锁）
    let pref = { ctx.config.lock().unwrap().default_shell.clone() };
    // PS 裸 `&` 前置检测：Job 操作符会静默丢输出 + 假成功（code=0），提前告知 AI 正确写法
    let amp_warning = if shell_is_ps(&pref) && ps_bare_ampersand(command) {
        Some("PowerShell 语义警告：命令含裸 `&`（后台 Job 操作符），前序命令的输出会丢失且退出码恒为 0。多条命令请用 `;` 串联；需要 cmd 的 `&` 语义请用 cmd /c \"...\" 包裹。".to_string())
    } else {
        None
    };
    let mut cmd = shell_command(&pref, command, cwd);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn command: {e}"))?;

    // 前台判定：显式标记后台 → 跳过窗口直接转后台；否则窗口内轮询是否已退出
    let mut exited: Option<std::process::ExitStatus> = None;
    if !force_background {
        let t0 = std::time::Instant::now();
        loop {
            if t0.elapsed().as_millis() >= FRONT_WINDOW_MS {
                break;
            }
            match child.try_wait() {
                Ok(Some(st)) => {
                    exited = Some(st);
                    break;
                }
                Ok(None) => {}
                Err(e) => return Err(format!("Failed to wait for command: {e}")),
            }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }

    // 快命令：与旧行为一致——收集输出后直接返回
    if exited.is_some() {
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| format!("Failed to collect command output: {e}"))?;
        let mut result = json!({
            "code": out.status.code(),
            "stdout": crate::registry::safe_trunc(&String::from_utf8_lossy(&out.stdout), 60000),
            "stderr": crate::registry::safe_trunc(&String::from_utf8_lossy(&out.stderr), 60000),
        });
        if let (Some(obj), Some(w)) = (result.as_object_mut(), amp_warning) {
            obj.insert("warning".into(), json!(w));
        }
        return Ok(result);
    }

    // 长命令：转后台
    let job = Arc::new(ShellJob {
        id: next_id(),
        session: session.map(|s| s.to_string()),
        command: command.to_string(),
        cwd: cwd.map(|s| s.to_string()),
        started: std::time::Instant::now(),
        cancel: Arc::new(Notify::new()),
        child: Mutex::new(Some(child)),
    });
    jobs().lock().unwrap().insert(job.id.clone(), job.clone());
    emit(ctx, "started", &job, None);
    crate::audit::record(
        ctx,
        "host",
        "shell.background",
        &job.id,
        json!({ "command": job.command, "session": job.session }),
        true,
    );
    let c2 = ctx.clone();
    let j2 = job.clone();
    tauri::async_runtime::spawn(async move {
        finish(c2, j2).await;
    });
    let mut bg_result = json!({
        "status": "background",
        "job_id": job.id,
        "note": if force_background {
            format!(
                "Started in background as job {}. The chat continues; the result will be delivered to this session when the command finishes — do not run this command again.",
                job.id
            )
        } else {
            format!(
                "Command was still running after {}ms; it has been moved to background job {}. The result will be delivered to the session when it finishes — do not run this command again.",
                FRONT_WINDOW_MS, job.id
            )
        },
    });
    if let (Some(obj), Some(w)) = (bg_result.as_object_mut(), amp_warning) {
        obj.insert("warning".into(), json!(w));
    }
    Ok(bg_result)
}

#[cfg(test)]
mod amp_tests {
    use super::ps_bare_ampersand;

    #[test]
    fn bare_ampersand_detected() {
        // 典型踩坑写法：裸 & 分隔多条命令
        assert!(ps_bare_ampersand("echo one & echo two"));
        assert!(ps_bare_ampersand("echo one &echo two"));
        assert!(ps_bare_ampersand("echo one &"));
        assert!(ps_bare_ampersand("echo one&echo two"));
    }

    #[test]
    fn legitimate_ampersand_not_flagged() {
        // 逻辑与
        assert!(!ps_bare_ampersand("cmd /c \"exit 0\" && echo ok"));
        assert!(!ps_bare_ampersand("exit 1 || echo fallback"));
        // 重定向
        assert!(!ps_bare_ampersand("node app.js 2>&1"));
        assert!(!ps_bare_ampersand("foo &> log.txt"));
        // 调用操作符（语句开头）
        assert!(!ps_bare_ampersand("& \"C:\\my script.ps1\""));
        assert!(!ps_bare_ampersand("echo a; & \"x.ps1\""));
        // 引号内的 &（cmd /c 包裹、字符串字面量）
        assert!(!ps_bare_ampersand("cmd /c \"echo one & echo two\""));
        assert!(!ps_bare_ampersand("echo \"a & b\""));
        assert!(!ps_bare_ampersand("echo 'a & b'"));
        // 无 & 的普通命令
        assert!(!ps_bare_ampersand("echo hello"));
    }
}

/// 进程树终止：shell 壳（pwsh/cmd）被杀后，它启动的孙进程会变成孤儿继续运行，
/// 既占资源又握着 stdout 管道写端，卡住收尾任务。start_kill 只杀直接子进程，
/// 这里 Windows 用 taskkill /T（整棵树）/F（强制），其他平台逐个杀子进程树尽力而为。
fn tree_kill(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .output();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = std::process::Command::new("pkill")
                .args(["-TERM", "-P", &pid.to_string()])
                .output();
        }
    }
    let _ = child.start_kill(); // 兜底：直接子进程必杀
}

/// 后台作业收尾：等待进程结束（或用户取消/硬超时）→ 广播 done/killed → 移除登记 → 自然结束时唤回 AI
async fn finish(ctx: Arc<crate::state::Ctx>, job: Arc<ShellJob>) {
    let child = { job.child.lock().unwrap().take() };
    let Some(mut child) = child else {
        jobs().lock().unwrap().remove(&job.id);
        return;
    };
    // 并发读 stdout / stderr（管道必须先被读，否则输出大的进程会写满管道被卡死）
    let so_pipe = child.stdout.take();
    let se_pipe = child.stderr.take();
    let so_task = tauri::async_runtime::spawn(async move {
        let mut buf: Vec<u8> = Vec::new();
        if let Some(mut p) = so_pipe {
            let _ = p.read_to_end(&mut buf).await;
        }
        buf
    });
    let se_task = tauri::async_runtime::spawn(async move {
        let mut buf: Vec<u8> = Vec::new();
        if let Some(mut p) = se_pipe {
            let _ = p.read_to_end(&mut buf).await;
        }
        buf
    });

    let timeout_sleep = tokio::time::sleep(std::time::Duration::from_secs(BG_TIMEOUT_SECS));
    tokio::pin!(timeout_sleep);
    // 结束方式：done=自然结束 / cancelled=用户手动停止 / timeout=运行超时强制终止
    let mut reason = "done".to_string();
    let status: Option<std::process::ExitStatus> = tokio::select! {
        st = child.wait() => st.ok(),
        _ = job.cancel.notified() => {
            reason = "cancelled".into();
            tree_kill(&mut child);
            let _ = child.wait().await;
            None
        }
        _ = &mut timeout_sleep => {
            reason = "timeout".into();
            tree_kill(&mut child);
            let _ = child.wait().await;
            None
        }
    };
    // 停止后读取剩余输出，但最多等 5s：管道写端可能被 shell 的孙进程继承持有
    // （shell 壳被杀后孙进程变孤儿继续握着 stdout），无限等会让 killed 事件
    // 和审计遥遥无期，UI 卡片"停了但一直显示运行中"。超时放弃读取即可，
    // cancelled/timeout 的输出本就不需要完整回收。
    let (stdout, stderr) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async { (so_task.await.unwrap_or_default(), se_task.await.unwrap_or_default()) },
    )
    .await
    .unwrap_or_default();
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    jobs().lock().unwrap().remove(&job.id);
    let ms = job.started.elapsed().as_millis() as u64;
    let code = status.and_then(|st| st.code());

    if reason == "done" {
        emit(&ctx, "done", &job, Some(json!({ "code": code, "ms": ms })));
        crate::audit::record(
            &ctx,
            "host",
            "shell.done",
            &job.id,
            json!({ "command": job.command, "code": code }),
            true,
        );
    } else {
        emit(&ctx, "killed", &job, Some(json!({ "ms": ms, "reason": reason.as_str() })));
        crate::audit::record(
            &ctx,
            "host",
            if reason == "cancelled" { "shell.cancelled" } else { "shell.timeout" },
            &job.id,
            json!({ "command": job.command }),
            true,
        );
    }

    // 无论自然结束、被用户手动停止还是超时终止，都把「结束方式」投递给顶层续跑 worker，
    // 由 resume 按 reason 生成说明唤回所属会话的 AI：
    //   自然结束 → 带结果继续推进任务；cancelled → 明确告知「用户手动停止」，避免 AI 误以为
    //   任务失败或仍在运行而干等 / 重复执行同一命令；timeout → 告知超时被终止、需与用户确认再走。
    if let Some(tx) = DONE_TX.get() {
        if let Some(sid) = job.session.clone() {
            let _ = tx.send(JobDone {
                session: sid,
                job_id: job.id.clone(),
                command: job.command.clone(),
                code,
                stdout,
                stderr,
                reason,
            });
        }
    }
}

/// 插件定时任务等外部产物的会话注入入口：把结果打包成 JobDone 交给顶层续跑 worker。
/// 会话空闲则自动开新回合处理，忙则注入历史等下轮上下文读到（与后台 shell 同链路）。
pub fn notify_session_result(
    session: &str,
    job_id: &str,
    command: &str,
    code: i32,
    stdout: String,
    stderr: String,
    reason: &str,
) {
    if let Some(tx) = DONE_TX.get() {
        let _ = tx.send(JobDone {
            session: session.to_string(),
            job_id: job_id.to_string(),
            command: command.to_string(),
            code: Some(code),
            stdout,
            stderr,
            reason: reason.to_string(),
        });
    }
}

/// 后台命令结束 → 唤回所属会话的 AI：
/// 会话空闲则自动开一个新回合处理结果；会话忙则先把结果注入历史，等下一次上下文自然读到。
async fn resume(ctx: &Arc<crate::state::Ctx>, msg: &JobDone) {
    use tauri::Emitter;
    let sid = &msg.session;
    if crate::agent::interrupted(ctx, sid) {
        return; // 会话已被中断，不自动续跑
    }
    {
        let store = ctx.sessions.lock().unwrap();
        if !store.sessions.iter().any(|s| &s.id == sid) {
            return; // 会话已删除
        }
    }
    let cmd = crate::registry::safe_trunc(&msg.command, 120);
    let code_txt = msg
        .code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".to_string());
    // 结束方式决定正文口径。cancelled 必须明确「用户手动停止」，避免 AI 误判为命令失败
    // 或以为任务仍在跑（干等 / 重复执行同一条命令）；timeout 同理给出处置指引。
    let (title, cap_out, cap_err, guide) = match msg.reason.as_str() {
        "cancelled" => (
            "[后台任务已手动停止]",
            4000,
            2000,
            "命令被用户手动停止（用户在面板点了「停止」或发送了停止指令），任务未完成——这是用户主动干预，不是命令本身出错。不要继续等待它的结果，也不要自动重新执行同一条命令；若任务仍需推进，先询问用户希望如何调整或是否重新运行。",
        ),
        "timeout" => (
            "[后台任务超时终止]",
            4000,
            2000,
            "命令运行超过后台上限（6 小时）被强制终止，任务未完成。不要原样自动重跑同一条命令；先与用户确认是否需要分拆步骤或改用更稳妥的方式执行。",
        ),
        _ => (
            "[后台任务完成]",
            16000,
            6000,
            "请基于上面的结果继续推进任务；若任务已全部完成，直接给出结论即可，不要重复执行该命令。",
        ),
    };
    let so = crate::registry::safe_trunc(msg.stdout.trim_end(), cap_out);
    let se = crate::registry::safe_trunc(msg.stderr.trim_end(), cap_err);
    let body = format!(
        "{title} 命令 `{cmd}`（job {}）已结束，退出码 {code_txt}。{guide}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        msg.job_id,
        if so.is_empty() { "(空)".to_string() } else { so },
        if se.is_empty() { "(空)".to_string() } else { se },
    );

    // 会话忙（用户正在发的回合 / 其他回合在跑）：只注入历史，不强开回合抢锁
    let busy = ctx.turn_locks.lock().unwrap().contains_key(sid);
    if busy {
        push_system_user(ctx, sid, &body);
        return;
    }
    // 会话空闲：稍作让渡避免与刚结束的回合抢锁，随后自动唤回 AI 处理
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    if ctx.turn_locks.lock().unwrap().contains_key(sid) {
        push_system_user(ctx, sid, &body);
        return;
    }
    let _ = crate::engine::chat_auto(ctx, sid, &body, Vec::new()).await;
    let _ = crate::worker::emit_ui(&ctx.app, "sessions-updated", json!(sid));
}

/// 启动后台 shell 的顶层续跑 worker（进程 setup 时调用一次）：
/// 常驻消费 JobDone，把命令结果逐个唤回所属会话的 AI。
pub fn init(ctx: &Arc<crate::state::Ctx>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<JobDone>();
    let _ = DONE_TX.set(tx);
    let c = ctx.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(msg) = rx.recv().await {
            resume(&c, &msg).await;
        }
    });
}

/// 把后台任务结果作为一条 role=user 的系统说明消息注入会话历史（前端会特殊渲染 [后台任务] 前缀）
fn push_system_user(ctx: &Arc<crate::state::Ctx>, sid: &str, body: &str) {
    use tauri::Emitter;
    {
        let mut store = ctx.sessions.lock().unwrap();
        if let Some(sess) = store.get_mut(sid) {
            sess.messages.push(crate::ai::ChatMessage::user(body));
            sess.touch();
            if sess.messages.len() > crate::state::CHAT_MAX {
                let drop_n = sess.messages.len() - crate::state::CHAT_MAX;
                sess.messages.drain(0..drop_n);
            }
        }
    }
    crate::session::persist(ctx);
    let _ = crate::worker::emit_ui(&ctx.app, "sessions-updated", json!(sid));
}

/// 用户 / UI 停止一个后台命令：通知其等待任务 kill 进程，事件 killed 会在片刻后广播
pub fn cancel(id: &str) -> bool {
    let job = jobs().lock().unwrap().get(id).cloned();
    match job {
        Some(j) => {
            j.cancel.notify_one();
            true
        }
        None => false,
    }
}

/// 所有在跑的后台命令（面板初始拉取用）
pub fn list() -> serde_json::Value {
    let m = jobs().lock().unwrap();
    let arr: Vec<serde_json::Value> = m
        .iter()
        .map(|(id, j)| {
            json!({
                "job_id": id,
                "command": j.command,
                "cwd": j.cwd,
                "session_id": j.session,
                "elapsed_ms": j.started.elapsed().as_millis() as u64,
            })
        })
        .collect();
    serde_json::Value::Array(arr)
}

/// 重复命令检测：同一会话里是否已有同一条命令在后台跑。返回其 job_id
pub fn find_running(session: Option<&str>, command: &str) -> Option<String> {
    let m = jobs().lock().unwrap();
    m.values()
        .find(|j| j.session.as_deref() == session && j.command == command)
        .map(|j| j.id.clone())
}
