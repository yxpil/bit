// yxpil · BIT
// 本地插件系统：toolhomes/plugins/<插件目录>/plugin.json 声明式插件。
//
// plugin.json 语法（全部字段除 name 外可选）：
// {
//   "name": "我的插件",
//   "version": "0.1.0",
//   "description": "一句话说明",
//   "tools": [{
//     "name": "mouse_move", "description": "…",
//     "parameters": { "type": "object", "properties": { … } },
//     "kind": "interpreter",              // interpreter（本机解释器）| script（Rhai 沙盒）
//     "runtime": "py",                    // interpreter 必填：py/js/ps1/…
//     "code": "…源码…",                   // 内联源码（stdin 收 JSON params，stdout 出结果）
//     "file": "tool.py"                   // 或相对插件目录的源码文件（与 code 二选一，file 优先）
//   }],
//   "prompts": ["启用时追加到系统提示词的指令片段"],
//   "skills": [{ "name": "技能名", "summary": "技能说明" }],
//   "memories": ["作为记忆注入的内容"],
//   "jobs": [{
//     "name": "日报", "schedule": "every 1h | daily 09:00",
//     "runtime": "py", "code": "…", "file": "…",
//     "session": "可选：结果注入的会话 id（复用后台 shell 唤回链路）"
//   }]
// }
//
// 启用开关存 config.disabled_plugins（不在插件包里），重扫不丢用户选择。
// 工具经 ToolKind::Interpreter/Script 走既有执行链（自动继承 toolhomes 环境与超时配置）。
use serde::{Deserialize, Serialize};
use chrono::Timelike;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// 插件声明的工具
#[derive(Serialize, Deserialize, Clone)]
pub struct PluginTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema 参数描述；缺省为无参
    #[serde(default)]
    pub parameters: serde_json::Value,
    /// interpreter（本机解释器）| script（Rhai 沙盒），默认 interpreter
    #[serde(default = "default_tool_kind")]
    pub kind: String,
    /// interpreter 模式必填：运行时 id（py/js/ps1/…）
    #[serde(default)]
    pub runtime: String,
    /// 内联源码
    #[serde(default)]
    pub code: Option<String>,
    /// 相对插件目录的源码文件（优先于 code）
    #[serde(default)]
    pub file: Option<String>,
}

fn default_tool_kind() -> String {
    "interpreter".into()
}

/// 插件声明的技能（注入技能列表，与自动提炼技能同构）
#[derive(Serialize, Deserialize, Clone)]
pub struct PluginSkill {
    pub name: String,
    #[serde(default)]
    pub summary: String,
}

/// 插件声明的定时任务
#[derive(Serialize, Deserialize, Clone)]
pub struct PluginJob {
    pub name: String,
    /// "every 30m" / "every 2h" / "every 90s" / "daily 09:00"
    pub schedule: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    /// 结果要注入的会话 id（空 = 只记审计日志）
    #[serde(default)]
    pub session: Option<String>,
}

/// 一个本地插件包（plugin.json）
#[derive(Serialize, Deserialize, Clone)]
pub struct Plugin {
    /// 插件 id = 目录名（sync 时强制用目录名覆盖，故 manifest 里可不写此字段；
    /// 不加 default 会让按文档格式编写的 plugin.json 解析失败、整个插件被静默跳过）
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tools: Vec<PluginTool>,
    #[serde(default)]
    pub prompts: Vec<String>,
    #[serde(default)]
    pub skills: Vec<PluginSkill>,
    #[serde(default)]
    pub memories: Vec<String>,
    #[serde(default)]
    pub jobs: Vec<PluginJob>,
}

pub fn dir(ctx: &Arc<crate::state::Ctx>) -> PathBuf {
    crate::toolenv::dir(ctx).join("plugins")
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// 解析工具源码：file 相对插件目录优先，回退内联 code
fn resolve_code(plugin_dir: &std::path::Path, file: &Option<String>, code: &Option<String>) -> Option<String> {
    if let Some(f) = file {
        let p = plugin_dir.join(f);
        if let Ok(s) = fs::read_to_string(&p) {
            return Some(s);
        }
    }
    code.clone().filter(|s| !s.trim().is_empty())
}

/// 扫描插件目录：读取全部 plugin.json（解析失败的目录跳过并列出错误）
pub fn scan(ctx: &Arc<crate::state::Ctx>) -> (Vec<Plugin>, Vec<String>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    let root = dir(ctx);
    let Ok(entries) = fs::read_dir(&root) else {
        return (out, errs);
    };
    for e in entries.flatten() {
        let path = e.path();
        if !path.is_dir() {
            continue;
        }
        let Some(pid) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let manifest = path.join("plugin.json");
        match fs::read_to_string(&manifest) {
            Ok(text) => match serde_json::from_str::<Plugin>(&text) {
                Ok(mut p) => {
                    p.id = pid.to_string();
                    if p.name.trim().is_empty() {
                        p.name = pid.to_string();
                    }
                    out.push(p);
                }
                Err(e) => errs.push(format!("{pid}: {e}")),
            },
            Err(_) => errs.push(format!("{pid}: plugin.json missing or unreadable")),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    (out, errs)
}

/// 工具/技能/记忆 与插件状态的同步：
/// 先清掉旧的 plugin 来源条目，再按当前启用列表重新写入。
/// 幂等：重扫/开关后调用都安全。
pub fn sync(ctx: &Arc<crate::state::Ctx>) {
    let (plugins, errs) = scan(ctx);
    let disabled: Vec<String> = { ctx.config.lock().unwrap().disabled_plugins.clone() };
    let is_on = |p: &Plugin| !disabled.contains(&p.id);

    // ---- 工具 ----
    {
        let mut tools = ctx.tools.lock().unwrap();
        tools.retain(|t| !t.created_by.starts_with("plugin:"));
        for p in plugins.iter().filter(|p| is_on(p)) {
            let pdir = dir(ctx).join(&p.id);
            for t in &p.tools {
                let Some(code) = resolve_code(&pdir, &t.file, &t.code) else {
                    continue;
                };
                let kind = if t.kind == "script" {
                    crate::registry::ToolKind::Script { code }
                } else {
                    if t.runtime.trim().is_empty() {
                        continue; // interpreter 模式必须指定运行时
                    }
                    crate::registry::ToolKind::Interpreter { runtime: t.runtime.trim().to_string(), code }
                };
                let parameters = if t.parameters.is_null() {
                    serde_json::json!({ "type": "object", "properties": {} })
                } else {
                    t.parameters.clone()
                };
                tools.push(crate::registry::ToolDef {
                    id: format!("plugin::{}::{}", p.id, sanitize(&t.name)),
                    name: format!("p_{}_{}", sanitize(&p.id), sanitize(&t.name)),
                    description: if t.description.is_empty() {
                        format!("Plugin `{}` tool `{}`", p.name, t.name)
                    } else {
                        t.description.clone()
                    },
                    parameters,
                    kind,
                    created_by: format!("plugin:{}", p.id),
                    created_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    enabled: true,
                });
            }
        }
    }
    let _ = ctx.save_tools();

    // ---- 技能 / 记忆：来源标记 plugin:<id>，重扫时整体重建 ----
    {
        let mut skills = ctx.skills.lock().unwrap();
        skills.retain(|s| !s.source.starts_with("plugin:"));
        for p in plugins.iter().filter(|p| is_on(p)) {
            for sk in &p.skills {
                skills.push(crate::memory::Skill {
                    id: String::new(), // 注入型技能不参与 memory(id) 取回，空 id
                    ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    name: sk.name.clone(),
                    summary: sk.summary.clone(),
                    source: format!("plugin:{}", p.id),
                });
            }
        }
    }
    {
        let mut memories = ctx.memories.lock().unwrap();
        memories.retain(|m| !m.source.starts_with("plugin:"));
        for p in plugins.iter().filter(|p| is_on(p)) {
            for content in &p.memories {
                memories.push(crate::memory::Memory {
                    id: String::new(),
                    ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    kind: "raw".into(),
                    content: content.clone(),
                    source: format!("plugin:{}", p.id),
                });
            }
        }
    }
    // 持久化 skills/memories
    {
        let skills = ctx.skills.lock().unwrap();
        let _ = fs::write(ctx.data_dir.join("skills.json"), serde_json::to_string(&*skills).unwrap_or_default());
    }
    {
        let memories = ctx.memories.lock().unwrap();
        let _ = fs::write(ctx.data_dir.join("memories.json"), serde_json::to_string(&*memories).unwrap_or_default());
    }

    // 更新内存里的插件列表
    {
        let mut cur = ctx.plugins.lock().unwrap();
        *cur = plugins;
    }
    // 成功也留痕：出问题时可区分"没扫到目录/没插件"与"解析失败"（此前排查靠它定位 id 字段缺失 bug）
    let total_tools: usize = ctx.tools.lock().unwrap().iter().filter(|t| t.created_by.starts_with("plugin:")).count();
    crate::trace::event(
        "plugins",
        &format!(
            "sync: {} plugin(s), {} plugin tool(s), {} memory(ies), {} skill(s), {} err(s)",
            ctx.plugins.lock().unwrap().len(),
            total_tools,
            ctx.memories.lock().unwrap().iter().filter(|m| m.source.starts_with("plugin:")).count(),
            ctx.skills.lock().unwrap().iter().filter(|s| s.source.starts_with("plugin:")).count(),
            errs.len()
        ),
    );
    if !errs.is_empty() {
        crate::trace::event("plugins", &format!("scan errors: {}", errs.join("; ")));
    }
}

/// 启用插件注入的提示词片段（禁用插件不注入）。供 ai.rs 组装系统提示词时调用。
pub fn prompt_fragment(ctx: &Arc<crate::state::Ctx>) -> String {
    let plugins = ctx.plugins.lock().unwrap();
    let disabled: Vec<String> = { ctx.config.lock().unwrap().disabled_plugins.clone() };
    let mut out = String::new();
    for p in plugins.iter().filter(|p| !disabled.contains(&p.id)) {
        for text in &p.prompts {
            if !text.trim().is_empty() {
                out.push_str(&format!("### Plugin `{}`\n{}\n", p.name, text.trim()));
            }
        }
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("\n## Plugin instructions\n{out}")
    }
}

// ---------- 定时任务调度 ----------

/// 任务上次执行时间（toolhomes/plugins/_jobs.json）：key = "<插件id>/<job名>"
type JobState = HashMap<String, String>;

fn jobs_state_path(ctx: &Arc<crate::state::Ctx>) -> PathBuf {
    dir(ctx).join("_jobs.json")
}

fn load_job_state(ctx: &Arc<crate::state::Ctx>) -> JobState {
    fs::read_to_string(jobs_state_path(ctx))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_job_state(ctx: &Arc<crate::state::Ctx>, st: &JobState) {
    let _ = fs::create_dir_all(dir(ctx));
    let _ = fs::write(
        jobs_state_path(ctx),
        serde_json::to_string_pretty(st).unwrap_or_default(),
    );
}

/// 解析 "every Ns/m/h" 与 "daily HH:MM"。
enum Sched {
    Every(std::time::Duration),
    Daily(u32, u32),
}

fn parse_schedule(s: &str) -> Option<Sched> {
    let s = s.trim().to_lowercase();
    if let Some(rest) = s.strip_prefix("every ") {
        let rest = rest.trim();
        let (num, unit) = rest.split_at(rest.len().saturating_sub(1));
        let n: u64 = num.trim().parse().ok()?;
        let secs = match unit {
            "s" => n,
            "m" => n * 60,
            "h" => n * 3600,
            _ => return None,
        };
        if secs == 0 {
            return None;
        }
        return Some(Sched::Every(std::time::Duration::from_secs(secs)));
    }
    if let Some(rest) = s.strip_prefix("daily ") {
        let parts: Vec<&str> = rest.trim().split(':').collect();
        if parts.len() == 2 {
            let h: u32 = parts[0].trim().parse().ok()?;
            let m: u32 = parts[1].trim().parse().ok()?;
            if h < 24 && m < 60 {
                return Some(Sched::Daily(h, m));
            }
        }
    }
    None
}

fn is_due(sched: &Sched, last: Option<&String>) -> bool {
    match sched {
        Sched::Every(d) => match last.and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()) {
            Some(t) => {
                let elapsed = chrono::Local::now().naive_local() - t;
                elapsed.to_std().map(|e| e >= *d).unwrap_or(true)
            }
            None => true,
        },
        Sched::Daily(h, m) => {
            let now = chrono::Local::now();
            // 今天没跑过 && 已到点（当天任意时刻错过则补跑一次）
            let ran_today = last
                .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok())
                .map(|t| t.date() == now.date_naive())
                .unwrap_or(false);
            !ran_today && (now.hour() > *h || (now.hour() == *h && now.minute() >= *m))
        }
    }
}

/// 执行一个插件任务：跑代码 → 审计 → 指定了 session 就复用后台 shell 唤回链路注入会话
async fn run_job(ctx: Arc<crate::state::Ctx>, pid: String, job: PluginJob) {
    let pdir = dir(&ctx).join(&pid);
    let Some(code) = resolve_code(&pdir, &job.file, &job.code) else {
        crate::audit::record(&ctx, "host", "plugin.job", &format!("{pid}/{}", job.name), serde_json::json!({ "error": "no code" }), false);
        return;
    };
    let runtime = if job.runtime.trim().is_empty() { "py".to_string() } else { job.runtime.trim().to_string() };
    let timeout = {
        let cfg = ctx.config.lock().unwrap();
        std::time::Duration::from_secs(cfg.tool_timeout_secs.clamp(1, 600) as u64)
    };
    let ctx2 = ctx.clone();
    let rt = runtime.clone();
    let code2 = code.clone();
    let handle = tauri::async_runtime::spawn_blocking(move || {
        crate::script_runtime::run(&ctx2, &rt, &code2, &serde_json::json!({}), timeout)
    });
    let out = tokio::time::timeout(std::time::Duration::from_secs(600), handle).await;
    let (ok, stdout, stderr) = match out {
        Ok(Ok(Ok(v))) => (true, serde_json::to_string_pretty(&v).unwrap_or_default(), String::new()),
        Ok(Ok(Err(e))) => (false, String::new(), e),
        Ok(Err(e)) => (false, String::new(), format!("task failed: {e}")),
        Err(_) => (false, String::new(), "job timed out (600s)".into()),
    };
    crate::audit::record(
        &ctx,
        "host",
        "plugin.job",
        &format!("{pid}/{}", job.name),
        serde_json::json!({ "ok": ok, "runtime": runtime }),
        ok,
    );
    // 有目标会话 → 结果作为后台任务产物唤回该会话的 AI（复用 shellbg 链路）
    if let Some(sid) = job.session.clone().filter(|s| !s.trim().is_empty()) {
        crate::shellbg::notify_session_result(
            &sid,
            &format!("plugin:{pid}/{}", job.name),
            &format!("定时任务 {}（插件 {pid}）", job.name),
            if ok { 0 } else { 1 },
            stdout,
            stderr,
            if ok { "done" } else { "timeout" },
        );
    }
}

/// 插件定时任务调度循环：每 30 秒检查一次到期任务。
/// 全程静默失败：单个任务出错不影响调度循环与其他插件。
pub async fn scheduler(ctx: Arc<crate::state::Ctx>) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let plugins = ctx.plugins.lock().unwrap().clone();
        if plugins.is_empty() {
            continue;
        }
        let disabled: Vec<String> = { ctx.config.lock().unwrap().disabled_plugins.clone() };
        let mut state = load_job_state(&ctx);
        let mut dirty = false;
        for p in plugins.iter().filter(|p| !disabled.contains(&p.id)) {
            for job in &p.jobs {
                let key = format!("{}/{}", p.id, job.name);
                let Some(sched) = parse_schedule(&job.schedule) else {
                    continue;
                };
                if !is_due(&sched, state.get(&key)) {
                    continue;
                }
                // 记录执行时间（先写后跑：同间隔长任务不会在每 tick 重复触发）
                state.insert(key, chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
                dirty = true;
                let jc = ctx.clone();
                let j = job.clone();
                let pid = p.id.clone();
                tauri::async_runtime::spawn(async move { run_job(jc, pid, j).await });
            }
        }
        if dirty {
            save_job_state(&ctx, &state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_schedule_works() {
        assert!(matches!(parse_schedule("every 30m"), Some(Sched::Every(d)) if d.as_secs() == 1800));
        assert!(matches!(parse_schedule("every 2h"), Some(Sched::Every(d)) if d.as_secs() == 7200));
        assert!(matches!(parse_schedule("daily 09:00"), Some(Sched::Daily(9, 0))));
        assert!(parse_schedule("weekly").is_none());
        assert!(parse_schedule("every 0m").is_none());
    }

    #[test]
    fn sanitize_keeps_model_friendly_names() {
        assert_eq!(sanitize("我的-工具 v1"), "______v1");
        assert_eq!(sanitize("mouse_move"), "mouse_move");
    }
}
