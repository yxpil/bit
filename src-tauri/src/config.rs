use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub remote_enabled: bool,
    pub host: String,
    pub port: u16,
    pub client_key: String,
    /// 远程访问密码（第二重校验，与 Client Key 独立）
    #[serde(default)]
    pub access_password: Option<String>,
    /// 是否启用密码校验（关闭则仅靠 Client Key）
    #[serde(default = "default_true")]
    pub password_enabled: bool,
    /// 每次保存配置自动递增
    pub revision: u64,
}

fn default_true() -> bool {
    true
}

fn generate_client_key() -> String {
    let mut rng = rand::thread_rng();
    let hex: String = (0..32).map(|_| format!("{:x}", rng.gen_range(0..16))).collect();
    format!("bit_{}", hex)
}

/// 生成 8 位数字访问密码（方便记忆输入）
fn generate_access_password() -> String {
    let mut rng = rand::thread_rng();
    (0..8).map(|_| format!("{}", rng.gen_range(0..10))).collect()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            remote_enabled: true,
            host: "127.0.0.1".into(),
            port: 8600,
            client_key: generate_client_key(),
            access_password: Some(generate_access_password()),
            password_enabled: true,
            revision: 1,
        }
    }
}

impl Config {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("config.json");
        let mut cfg = match fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        // 旧版本配置无密码字段：自动生成并持久化
        if cfg.access_password.as_deref().unwrap_or("").is_empty() {
            cfg.access_password = Some(generate_access_password());
        }
        let _ = fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap());
        cfg
    }

    pub fn save(&self, dir: &Path) {
        let _ = fs::write(
            dir.join("config.json"),
            serde_json::to_string_pretty(self).unwrap(),
        );
    }

    pub fn new_client_key(&mut self) -> String {
        self.client_key = generate_client_key();
        self.client_key.clone()
    }

    /// 重新生成随机访问密码
    pub fn new_access_password(&mut self) -> String {
        self.access_password = Some(generate_access_password());
        self.access_password.clone().unwrap()
    }

    /// 校验远程访问密码（未启用密码校验时直接通过）
    pub fn verify_access_password(&self, provided: &str) -> bool {
        if !self.password_enabled {
            return true;
        }
        match &self.access_password {
            Some(expected) => {
                // 常数时间比较，避免时序侧信道
                let a = provided.as_bytes();
                let b = expected.as_bytes();
                if a.len() != b.len() {
                    return false;
                }
                a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
            }
            None => false,
        }
    }

    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}
