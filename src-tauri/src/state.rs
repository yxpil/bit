use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use tauri::Manager;

use crate::ai::AiConfig;
use crate::audit::AuditEntry;
use crate::goal::{Goal, Todo};
use crate::memory::{Memory, Skill};
use crate::registry::ToolDef;
use crate::runtime::Runtime;
use crate::session::SessionStore;

pub struct Ctx {
    pub app: tauri::AppHandle,
    pub data_dir: PathBuf,
    pub config: Mutex<crate::config::Config>,
    pub ai_config: Mutex<AiConfig>,
    pub tools: Mutex<Vec<ToolDef>>,
    pub runtimes: Mutex<Vec<Runtime>>,
    pub audit: Mutex<Vec<AuditEntry>>,
    pub memories: Mutex<Vec<Memory>>,
    pub skills: Mutex<Vec<Skill>>,
    pub goals: Mutex<Vec<Goal>>,
    pub todos: Mutex<Vec<Todo>>,
    pub sessions: Mutex<SessionStore>,
    /// 已接入的 MCP 服务器（Streamable HTTP）
    pub mcp: Mutex<Vec<crate::mcp::McpServer>>,
    /// 小圆片播放/暂停状态（true = 播放，自动总结进行中）
    pub autopilot_running: AtomicBool,
    /// 会话中断标志（session_id → flag），chat_interrupt 置位后执行循环在检查点停止
    pub interrupts: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// 待审批工具调用（request_id → 应答通道）
    pub approvals: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    /// 审批请求自增 id
    pub approval_seq: AtomicU64,
    pub server_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

pub const AUDIT_MAX: usize = 2000;
pub const CHAT_MAX: usize = 60;

impl Ctx {
    pub fn load(app: tauri::AppHandle) -> Arc<Ctx> {
        let data_dir = app
            .path()
            .app_data_dir()
            .expect("failed to resolve app data dir");
        fs::create_dir_all(&data_dir).ok();

        let config = crate::config::Config::load(&data_dir);
        let ai_config: AiConfig = read_json(&data_dir.join("ai_config.json")).unwrap_or_default();
        let tools: Vec<ToolDef> =
            read_json(&data_dir.join("tools.json")).unwrap_or_default();
        let audit: Vec<AuditEntry> = read_json(&data_dir.join("audit.json"))
            .unwrap_or_default();
        let memories: Vec<Memory> =
            read_json(&data_dir.join("memories.json")).unwrap_or_default();
        let skills: Vec<Skill> = read_json(&data_dir.join("skills.json")).unwrap_or_default();
        let goals: Vec<Goal> = read_json(&data_dir.join("goals.json")).unwrap_or_default();
        let todos: Vec<Todo> = read_json(&data_dir.join("todos.json")).unwrap_or_default();
        let sessions = SessionStore::load(&data_dir);
        let mcp: Vec<crate::mcp::McpServer> =
            read_json(&data_dir.join("mcp_servers.json")).unwrap_or_default();

        // 内置工具随版本演进：始终以当前出厂的内置工具为准，
        // 移除历史遗留的内置项，保留用户 / AI 自建的工具，再把最新内置放到最前。
        let tools = {
            let builtin = crate::registry::builtin_tools();
            let builtin_names: std::collections::HashSet<String> =
                builtin.iter().map(|t| t.name.clone()).collect();
            let mut custom: Vec<ToolDef> = tools
                .into_iter()
                .filter(|t| {
                    !matches!(t.kind, crate::registry::ToolKind::Builtin { .. })
                        && !builtin_names.contains(&t.name)
                })
                .collect();
            let mut merged = builtin;
            merged.append(&mut custom);
            let _ = fs::write(
                data_dir.join("tools.json"),
                serde_json::to_string_pretty(&merged).unwrap(),
            );
            merged
        };

        // 解释器列表：每次启动都重新探测本机（自动发现新装的语言），
        // 同时沿用旧列表里的启用状态，并保留用户手动添加的项。
        let cached: Vec<Runtime> =
            read_json(&data_dir.join("runtimes.json")).unwrap_or_default();
        // 解释器列表：启动时直接用缓存（探测在后台进行，不阻塞窗口显示），
        // 后台 refresh_runtimes() 完成后更新状态并通知前端
        let runtimes: Vec<Runtime> = cached;

        Arc::new(Ctx {
            app,
            data_dir,
            config: Mutex::new(config),
            ai_config: Mutex::new(ai_config),
            tools: Mutex::new(tools),
            runtimes: Mutex::new(runtimes),
            audit: Mutex::new(audit),
            memories: Mutex::new(memories),
            skills: Mutex::new(skills),
            goals: Mutex::new(goals),
            todos: Mutex::new(todos),
            sessions: Mutex::new(sessions),
            mcp: Mutex::new(mcp),
            interrupts: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            approval_seq: AtomicU64::new(1),
            autopilot_running: AtomicBool::new(false),
            server_task: Mutex::new(None),
        })
    }

    pub fn save_config(&self) {
        let cfg = self.config.lock().unwrap();
        cfg.save(&self.data_dir);
    }

    /// 后台重新探测本机解释器（保留启用状态与手动添加项）。
    /// 返回列表是否发生变化（由调用方决定是否通知前端）。
    pub fn refresh_runtimes(&self) -> bool {
        let cached: Vec<Runtime> =
            read_json(&self.data_dir.join("runtimes.json")).unwrap_or_default();
        let prev_enabled: std::collections::HashMap<String, bool> =
            cached.iter().map(|r| (r.id.clone(), r.enabled)).collect();
        let manual: Vec<Runtime> = cached.iter().filter(|r| r.manual).cloned().collect();
        let mut runtimes = crate::runtime::detect();
        for r in runtimes.iter_mut() {
            if let Some(&en) = prev_enabled.get(&r.id) {
                r.enabled = en;
            }
        }
        for m in manual {
            if !runtimes.iter().any(|r| r.id == m.id) {
                runtimes.push(m);
            }
        }
        let changed = serde_json::to_string(&runtimes).unwrap()
            != serde_json::to_string(&cached).unwrap();
        let _ = fs::write(
            self.data_dir.join("runtimes.json"),
            serde_json::to_string_pretty(&runtimes).unwrap(),
        );
        *self.runtimes.lock().unwrap() = runtimes;
        changed
    }

    pub fn save_ai_config(&self) {
        let cfg = self.ai_config.lock().unwrap();
        let _ = fs::write(
            self.data_dir.join("ai_config.json"),
            serde_json::to_string_pretty(&*cfg).unwrap(),
        );
    }

    pub fn save_tools(&self) {
        let tools = self.tools.lock().unwrap();
        let _ = fs::write(
            self.data_dir.join("tools.json"),
            serde_json::to_string_pretty(&*tools).unwrap(),
        );
    }

    pub fn save_runtimes(&self) {
        let runtimes = self.runtimes.lock().unwrap();
        let _ = fs::write(
            self.data_dir.join("runtimes.json"),
            serde_json::to_string_pretty(&*runtimes).unwrap(),
        );
    }

    pub fn save_sessions(&self) {
        let store = self.sessions.lock().unwrap();
        let _ = fs::write(
            self.data_dir.join("sessions.json"),
            serde_json::to_string(&*store).unwrap(),
        );
    }

    pub fn save_mcp(&self) {
        let mcp = self.mcp.lock().unwrap();
        let _ = fs::write(
            self.data_dir.join("mcp_servers.json"),
            serde_json::to_string_pretty(&*mcp).unwrap(),
        );
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> Option<T> {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

impl Serialize for Ctx {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("Ctx")
    }
}
