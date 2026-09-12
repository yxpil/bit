// yxpil · BIT
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
    /// 目标自动推进：回合结束后本会话有未完成目标时，自动把规划的下一步发给 AI 续跑直到完成
    #[serde(default = "default_true")]
    pub auto_drive: bool,
    /// 子代理自动委派：Autopilot 周期里由宿主把「活跃目标下未开始的待办」派生给子代理并行推进。
    /// 只是宿主是否主动派活的开关（模型侧始终可自主调用 sub_agent 派生，共用并行上限）；默认关闭以免悄悄烧 token
    #[serde(default)]
    pub auto_delegate: bool,
    /// 自动委派并行子代理数上限（1..=8）：宿主每周期最多同时推进这么多条待办。
    /// 子代理会完整跑 agent 循环（多轮工具 + token），默认 3；越大越烧配额
    #[serde(default = "default_subagent_max")]
    pub subagent_max: u32,
    /// 兼容模式（全局）：默认关 = 标准协议（原生 function calling，请求带 tools 参数）；
    /// 开启 = 文本约定——在系统提示词注入 JSON 调用契约，并解析回复正文里的单行 JSON 数组
    /// 工具调用。只用于不支持 tools 参数的端点（纯文本中转等），无需探测、不做自动降级
    #[serde(default)]
    pub compat_mode: bool,
    /// 工作区沙箱根（绝对路径）：设置后 shell 未显式传 cwd 时以此为工作目录，
    /// read_file/write_file/edit 的相对路径锚定于此、绝对路径禁止逃出根外。
    /// None = 不限制；TUI 模式启动时自动取进程当前目录（运行时值，不回写配置）
    #[serde(default)]
    pub workspace_root: Option<String>,
    /// 极简提示词：开启后默认系统提示词只保留一句身份 + 兼容模式契约（如启用）
    /// + 按启用工具动态注入的少量提示；用户自定义覆盖模板时不生效
    #[serde(default)]
    pub minimal_prompt: bool,
    /// 工具权限闸门：screen / mouse / keyboard 三个本机操控能力（实验性功能）默认关闭——
    /// 关闭时不给模型下发 schema/一览（省 token），工具调用也直接拒绝。
    /// 现状：macOS 上整体屏蔽（tool_gate 恒 false，见下）；Windows/Linux 构建保留实验性入口
    /// 但平台后端尚未落地，即使手动打开，实际调用也会返回“实现未就绪”的明确错误，绝不误报成功。
    /// 历史遗留的注册项在启动时按出厂清单自动移除
    #[serde(default)]
    pub tool_screen: bool,
    #[serde(default)]
    pub tool_mouse: bool,
    #[serde(default)]
    pub tool_keyboard: bool,
    /// 画图工具闸门（draw_diagram）：默认关闭，与三件套同一张设置卡片；打开即生效（配置保存已修复）
    #[serde(default)]
    pub tool_diagram: bool,
    /// 图片查看闸门（view_image）：默认关闭，模型不主动看本机图片
    #[serde(default)]
    pub tool_viewimage: bool,
    /// 幻觉防护：单个词在一条回复里出现次数达到该值即判定为幻觉循环（0=关闭）
    #[serde(default = "default_word_repeat_max")]
    pub word_repeat_max: u32,
    /// 幻觉防护：单个回合内工具调用轮次达到该值即强制终止回合（0=不设限）
    #[serde(default = "default_tool_loop_max")]
    pub tool_loop_max: u32,
    /// 自定义工具（解释器/脚本/Rhai 脚本）单次执行超时秒数：默认 120，上限 600。
    /// 鼠标/屏幕类自定义工具常含循环监听，30 秒硬上限会把它们误杀
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u32,
    /// 默认 shell（agent 后台 shell 与命令执行用）：空 = 自动识别
    /// （Windows 优先 pwsh 7 → powershell 5.1；Unix 优先 $SHELL → bash）。
    /// 设置页可从检测到的可用 shell 列表里指定
    #[serde(default)]
    pub default_shell: String,
    /// 禁用的本地插件 id 列表（toolhomes/plugins/<id>）：不在包里存，重扫不丢用户选择
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
    /// 唤出主界面的全局快捷键（如 "Alt+Shift+B"）：窗口隐藏/最小化时也能系统级唤出；
    /// 空字符串 = 关闭。格式遵循 tauri-plugin-global-shortcut（Ctrl/Alt/Shift/Super + 键名）
    #[serde(default = "default_hotkey_show")]
    pub hotkey_show: String,
    /// 写入/编辑代码文件后的轻量语法检查（json 解析 / node --check / python ast）：
    /// 发现语法错误时注入工具结果提醒模型自我修复，并向聊天 UI 发警告。默认开
    #[serde(default = "default_true")]
    pub syntax_check: bool,
    /// 禁用的本地插件 id 列表（toolhomes/plugins/<id>）：不在包里存，重扫不丢用户选择
    #[serde(default = "default_true")]
    pub agent_worker_enabled: bool,
    /// agent worker 卡死判定秒数：回合在飞但超过该时长无任何事件 → 监督循环杀掉重启（0=关闭）
    #[serde(default = "default_worker_hang_secs")]
    pub worker_hang_secs: u32,
    /// 用户自定义提示词/人设：追加到 system prompt 开头，空则不追加
    #[serde(default)]
    pub custom_prompt: String,
    /// 系统提示词模板覆盖：非空时完全替换内置 system prompt；空则用默认模板
    #[serde(default)]
    pub system_prompt: String,
    /// 云中继地址（手机远程 App 用）：对称 NAT 无法直连时改连该地址，如 Cloudflare Tunnel 域名
    #[serde(default)]
    pub cloud_relay_url: Option<String>,
    /// 云中继识别码：客户端生成的 128 位随机数（32 hex 字符）。中继服务器按它路由，
    /// 二维码携带、手机端据此改连；非空时应用端自动接入云中继
    #[serde(default)]
    pub relay_id: String,
    /// 自定 STUN 服务器列表（host:port）：整体替换内置免费列表（None/空 = 用内置默认），
    /// NAT 类型探测与公网映射的数据源
    #[serde(default)]
    pub stun_servers: Option<Vec<String>>,
    /// 敏感词审核开关：HTTP 对话端点（/api/chat、/v1/chat/completions）输入/输出双向过滤
    #[serde(default = "default_true")]
    pub moderation_enabled: bool,
    /// 自定敏感词表：整体替换内置默认词库（None/空 = 用内置默认）
    #[serde(default)]
    pub blocked_words: Option<Vec<String>>,
    /// 每 IP 并发在途对话请求上限（/api/chat 与 /v1/chat/completions 按客户端 IP 分别计数；
    /// 0=不限）。防单 IP 洪泛占用对话通道
    #[serde(default = "default_max_active_per_ip")]
    pub max_active_per_ip: u32,
    /// IP 黑名单：命中的客户端 IP 直接 403（精确匹配，IPv4/IPv6 字符串）
    #[serde(default)]
    pub ip_blocklist: Option<Vec<String>>,
    /// 信道防护：经云中继进来的请求必须携带合法 bitsign-v2 签名（只有完成设备注册的
    /// 本 App 信道实现能算出），第三方 App 借道中继直接 403。LAN 直连不受影响
    #[serde(default = "default_true")]
    pub channel_guard: bool,
    /// 设备凭证（bitdev_*）：硬件指纹+公网IP+注册时间戳派生（device.rs），账号凭证级
    /// 稳定锚点；并入信道签名材料并作为封禁/限额的服务端依据。首次启动自动注册
    #[serde(default)]
    pub device_key: Option<String>,
    /// 设备注册时刻（unix 秒）：参与设备 key 派生，同指纹不同时刻得到不同 key
    #[serde(default)]
    pub device_registered_at: Option<i64>,
    /// 注册时指纹哈希前 16 hex（sha256）：用于检测硬件指纹整体漂移（不存原始指纹）
    #[serde(default)]
    pub device_fp_hash: Option<String>,
    /// 远程对话限速：每 60 秒滑动窗口内允许的对话请求上限（/api/chat 与 /v1/chat/completions
    /// 按客户端 IP 分别计数；0=不限）
    #[serde(default = "default_chat_rpm_max")]
    pub chat_rpm_max: u32,
    /// 中继每用户响应带宽：按手机端真实公网 IP 的令牌桶（2 秒突发余量），KB/s；
    /// 正常聊天无感，防借道拉大文件。0=不限
    #[serde(default = "default_relay_kbps_per_user")]
    pub relay_kbps_per_user: u32,
    /// 中继文本长度上限：经中继进入的聊天请求输入文本总字符数
    /// （/api/chat 单条 + /v1/chat/completions 全部消息），0=不限。
    /// 只算文本（图片另计不占预算）——防把中继当大文本传输通道；
    /// 正常对话通常 < 10K 字符，64K 足够宽松。仅限中继面，LAN 直连不受限
    #[serde(default = "default_relay_max_text_chars")]
    pub relay_max_text_chars: u32,
    /// 开机自动启动：登录系统后自动在后台启动（登录项由 tauri-plugin-autostart 管理）
    #[serde(default)]
    pub autostart: bool,
    /// 高权限模式意图：开启后以管理员/root 身份重启（实际是否提权以运行时探测为准）
    #[serde(default)]
    pub elevated: bool,
}

/// 目标自动推进的安全上限：同一目标最多自动续跑轮数（防空转无限烧 token）
pub const AUTO_DRIVE_MAX: u32 = 25;

fn default_word_repeat_max() -> u32 {
    20
}

fn default_subagent_max() -> u32 {
    3
}

fn default_tool_loop_max() -> u32 {
    20
}

fn default_tool_timeout_secs() -> u32 {
    120
}

fn default_worker_hang_secs() -> u32 {
    600
}

fn default_hotkey_show() -> String {
    "Alt+Shift+B".into()
}

fn default_chat_rpm_max() -> u32 {
    20
}

fn default_relay_kbps_per_user() -> u32 {
    // 200 KB/s ≈ 0.2 MB/s：正常流式对话（通常 < 10 KB）无感，借道拉大文件被摊平到预算内
    200
}

fn default_relay_max_text_chars() -> u32 {
    // 64K 字符：正常对话（含较长历史回传）足够宽松，MB 级文本搬运被拒之门外
    65_536
}

fn default_max_active_per_ip() -> u32 {
    // 6：中继场景下同一家庭/办公 NAT 的多台手机共享公网 IP，预算过小会互相挤兑
    6
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
            tool_diagram: false,
            tool_viewimage: false,
            host: "127.0.0.1".into(),
            port: 8600,
            client_key: generate_client_key(),
            access_password: Some(generate_access_password()),
            password_enabled: true,
            revision: 1,
            tool_approval: default_approval(),
            auto_drive: true,
            auto_delegate: false,
            subagent_max: default_subagent_max(),
            compat_mode: false,
            workspace_root: None,
            minimal_prompt: false,
            tool_screen: false,
            tool_mouse: false,
            tool_keyboard: false,
            agent_worker_enabled: true,
            worker_hang_secs: default_worker_hang_secs(),
            word_repeat_max: default_word_repeat_max(),
            tool_loop_max: default_tool_loop_max(),
            tool_timeout_secs: default_tool_timeout_secs(),
            default_shell: String::new(),
            disabled_plugins: Vec::new(),
            hotkey_show: default_hotkey_show(),
            syntax_check: true,
            custom_prompt: String::new(),
            system_prompt: String::new(),
            cloud_relay_url: None,
            relay_id: String::new(),
            stun_servers: None,
            moderation_enabled: true,
            blocked_words: None,
            max_active_per_ip: default_max_active_per_ip(),
            ip_blocklist: None,
            channel_guard: true,
            device_key: None,
            device_registered_at: None,
            device_fp_hash: None,
            chat_rpm_max: default_chat_rpm_max(),
            relay_kbps_per_user: default_relay_kbps_per_user(),
            relay_max_text_chars: default_relay_max_text_chars(),
            autostart: false,
            elevated: false,
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
        // 原子写 + 重试：直接 fs::write 在文件被并发占用时会静默失败（let _ 吞掉错误），
        // 表现为"设置开关不保存"。先写临时文件再 rename，rename 被占用则短暂重试，
        // 全部失败时回退直接覆盖并把错误打到 stderr（不再无痕迹丢失）
        let path = dir.join("config.json");
        let body = match serde_json::to_string_pretty(self) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[config] serialize failed: {e}");
                crate::worker::notify_reload();
                return;
            }
        };
        let tmp = dir.join("config.json.tmp");
        let mut ok = fs::write(&tmp, &body).is_ok();
        if ok {
            ok = false;
            for _ in 0..4 {
                if fs::rename(&tmp, &path).is_ok() {
                    ok = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
        }
        if !ok {
            if fs::write(&path, &body).is_ok() {
                eprintln!("[config] rename retry exhausted, fallback direct write ok");
            } else {
                eprintln!("[config] SAVE FAILED — settings may be lost (file locked?)");
            }
        }
        // agent worker 模式：宿主改配置后通知 worker 重读（worker 进程内调用为空操作）
        crate::worker::notify_reload();
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
        join_host_port(&self.host, self.port)
    }
}

/// host:port 拼接：IPv6 字面量自动加方括号（TcpListener 绑定与 URL 均要求 `[::1]:8600` 形态）。
/// host 允许传入带括号的 `[::1]`，统一剥掉后按需重加，避免双括号。
pub fn join_host_port(host: &str, port: u16) -> String {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.contains(':') {
        format!("[{h}]:{port}")
    } else {
        format!("{h}:{port}")
    }
}

/// 归一化监听主机输入：剥掉 IPv6 方括号，统一存裸地址（展示时由 join_host_port 加回）。
/// 返回 Err 的输入：空串、含空白或路径分隔符、既非合法 IP 也非域名形态
pub fn normalize_host(input: &str) -> Result<String, String> {
    let h = input.trim().trim_start_matches('[').trim_end_matches(']').to_string();
    if h.is_empty() {
        return Err("监听地址不能为空".into());
    }
    let ok = h.parse::<std::net::IpAddr>().is_ok()
        || h == "localhost"
        || (h.contains('.') && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')));
    if !ok {
        return Err(format!("无效的监听地址: {h}"));
    }
    Ok(h)
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

    /// host:port 拼接：IPv6 字面量加方括号，IPv4/域名原样；输入带括号不双括号
    #[test]
    fn test_join_host_port() {
        assert_eq!(join_host_port("::1", 8600), "[::1]:8600");
        assert_eq!(join_host_port("::", 8600), "[::]:8600");
        assert_eq!(join_host_port("[::1]", 8600), "[::1]:8600");
        assert_eq!(join_host_port("127.0.0.1", 8600), "127.0.0.1:8600");
        assert_eq!(join_host_port("0.0.0.0", 8600), "0.0.0.0:8600");
        assert_eq!(join_host_port("localhost", 8600), "localhost:8600");
    }

    /// 主机归一化：剥括号存裸地址；拒绝空串/垃圾输入；IPv4/IPv6/域名放行
    #[test]
    fn test_normalize_host() {
        assert_eq!(normalize_host("[::1]").unwrap(), "::1");
        assert_eq!(normalize_host("::1").unwrap(), "::1");
        assert_eq!(normalize_host("  0.0.0.0 ").unwrap(), "0.0.0.0");
        assert_eq!(normalize_host("my-host.local").unwrap(), "my-host.local");
        assert!(normalize_host("").is_err());
        assert!(normalize_host("[  ]").is_err());
        assert!(normalize_host("bad host").is_err());
        assert!(normalize_host("1.2.3.4:80").is_err());
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

impl Config {
    /// 工具权限闸门：screen / mouse / keyboard 需在设置页开启才对模型生效
    /// （关闭 = 不下发 schema/一览 + 调用直接拒绝，省 token 且避免无谓报错）。
    /// 三平台均可用：Windows SendInput+DXGI / macOS CG+TCC / Linux X11（见 desktop_ctl.rs）
    pub fn tool_gate(&self, name: &str) -> bool {
        match name {
            "screen" => self.tool_screen,
            "mouse" => self.tool_mouse,
            "keyboard" => self.tool_keyboard,
            "draw_diagram" => self.tool_diagram,
            "view_image" => self.tool_viewimage,
            _ => true,
        }
    }
}
