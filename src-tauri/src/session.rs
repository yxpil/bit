use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::ai::ChatMessage;

/// 一段独立的对话会话（多会话分组）
#[derive(Serialize, Deserialize, Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created: String,
    pub updated: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
}

impl Session {
    pub fn new(title: &str) -> Self {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        Session {
            id: uuid::Uuid::new_v4().simple().to_string(),
            title: if title.trim().is_empty() { "新对话".into() } else { title.trim().to_string() },
            created: now.clone(),
            updated: now,
            messages: Vec::new(),
        }
    }

    pub fn touch(&mut self) {
        self.updated = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    }

    /// 会话摘要（用于列表右侧预览）：取最后一条非工具反馈消息的开头
    pub fn preview(&self) -> String {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role != "system")
            .map(|m| {
                let s: String = m.content.chars().take(40).collect();
                s.replace('\n', " ")
            })
            .unwrap_or_default()
    }
}

/// 会话存储：始终保证至少有一个会话，并记录当前激活会话
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SessionStore {
    #[serde(default)]
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub active: String,
}

impl SessionStore {
    /// 载入：从新格式读取；若不存在则尝试迁移旧的 chats.json 为一个默认会话
    pub fn load(data_dir: &std::path::Path) -> SessionStore {
        if let Some(store) = read_json::<SessionStore>(&data_dir.join("sessions.json")) {
            if !store.sessions.is_empty() {
                return store.normalized();
            }
        }
        // 迁移旧的单一历史
        let legacy: Vec<ChatMessage> =
            read_json(&data_dir.join("chats.json")).unwrap_or_default();
        let mut s = Session::new("默认对话");
        s.messages = legacy;
        let active = s.id.clone();
        SessionStore { sessions: vec![s], active }
    }

    /// 保证至少有一个会话、active 指向存在的会话
    fn normalized(mut self) -> SessionStore {
        if self.sessions.is_empty() {
            let s = Session::new("默认对话");
            self.active = s.id.clone();
            self.sessions.push(s);
        }
        if !self.sessions.iter().any(|s| s.id == self.active) {
            self.active = self.sessions[0].id.clone();
        }
        self
    }

    pub fn active_mut(&mut self) -> &mut Session {
        let active = self.active.clone();
        let idx = self.sessions.iter().position(|s| s.id == active).unwrap_or(0);
        &mut self.sessions[idx]
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> Option<T> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

pub fn persist(ctx: &Arc<crate::state::Ctx>) {
    let store = ctx.sessions.lock().unwrap();
    let _ = std::fs::write(
        ctx.data_dir.join("sessions.json"),
        serde_json::to_string(&*store).unwrap_or_default(),
    );
}
