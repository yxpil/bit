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
    /// 工具审批模式：ask = 每次询问 / auto = 危险操作询问、安全操作自动通过 / allow_all = 完全放行
    #[serde(default = "default_approval")]
    pub tool_approval: String,
}

fn default_approval() -> String {
    "allow_all".into()
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
            // 远程访问默认关闭：仅当用户在设置里主动开启后才监听端口
            remote_enabled: false,
            host: "127.0.0.1".into(),
            port: 8600,
            client_key: generate_client_key(),
            access_password: Some(generate_access_password()),
            password_enabled: true,
            revision: 1,
            tool_approval: default_approval(),
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
            Some(expected) => ct_eq(provided.as_bytes(), expected.as_bytes()),
            None => false,
        }
    }

    /// 校验 Client Key（常数时间比较，避免时序侧信道）
    pub fn verify_client_key(&self, provided: &str) -> bool {
        if self.client_key.is_empty() {
            return false;
        }
        ct_eq(provided.as_bytes(), self.client_key.as_bytes())
    }

    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// 常数时间字节比较（长度不等直接 false，长度本身不构成敏感信息）
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 远程访问默认必须关闭：新装环境不监听端口，除非用户在设置里主动开启
    #[test]
    fn test_default_remote_disabled() {
        let cfg = Config::default();
        assert!(!cfg.remote_enabled);
        // 关闭默认不影响鉴权要素：Client Key 与访问密码照常生成
        assert!(cfg.client_key.starts_with("bit_"));
        assert!(cfg.access_password.as_deref().unwrap_or("").len() == 8);
        assert!(cfg.password_enabled);
    }

    /// 已有配置文件里的 remote_enabled 必须原样保留（老用户已开启的不被静默关闭）
    #[test]
    fn test_load_preserves_enabled() {
        let dir = std::env::temp_dir().join(format!("bit-cfg-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(
            &path,
            r#"{"remote_enabled":true,"host":"0.0.0.0","port":8600,"client_key":"bit_x","revision":3}"#,
        )
        .unwrap();
        let cfg = Config::load(&dir);
        assert!(cfg.remote_enabled);
        assert_eq!(cfg.port, 8600);
        assert_eq!(cfg.revision, 3);
        fs::remove_dir_all(&dir).ok();
    }

    /// Client Key 校验：正确/错误/长度不等/空 key，均按预期判定（常数时间比较语义不变）
    #[test]
    fn test_verify_client_key() {
        let cfg = Config {
            client_key: "bit_ab12ab12ab12ab12ab12ab12ab12ab12".into(),
            ..Config::default()
        };
        assert!(cfg.verify_client_key("bit_ab12ab12ab12ab12ab12ab12ab12ab12"));
        assert!(!cfg.verify_client_key("bit_ab12ab12ab12ab12ab12ab12ab12ab13"));
        assert!(!cfg.verify_client_key("bit_"));
        assert!(!cfg.verify_client_key(""));
        // 空 client_key 一律拒绝（配合 check_auth 的 503 前置）
        let empty = Config {
            client_key: "".into(),
            ..Config::default()
        };
        assert!(!empty.verify_client_key(""));
    }
}
