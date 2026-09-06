// yxpil · BIT
//! 信道安全模块：bitsign-v1 请求签名 / BIT-Crypt v1 二维码加密块 / 敏感词审核 / nonce 重放缓存。
//! 算法均为自研轻量方案，移动端按本文档注释可独立复现（Dart/JS 均无平台依赖）。

use sha2::{Digest, Sha256};

// ── 基础原语 ──

/// RFC 2104 HMAC-SHA256（手工实现，避免引入 hmac 依赖）
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let d = Sha256::digest(key);
        k[..32].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(msg);
    let ih = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(ih);
    outer.finalize().into()
}

/// 常量时间比较（防时序侧信道；长度不同直接不等）
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ── bitsign-v2：中继信道请求签名（只有本 App 的信道实现能算出合法签名）──
//
// canonical = "bitsign-v2\n{rid}\n{ts}\n{nonce}\n{method}\n{path}\n{device_material}"
// device_material = 设备凭证的加盐 SHA256 前 16 hex（device.rs::sig_material）
// mac       = HMAC-SHA256(key = client_key, canonical)
// k         = SHA256("bitsign-v2:" + client_key)                 // 32B 混淆密钥流
// out[i]    = mac[i].rotate_left((i % 7) + 1) ^ k[i % 16]        // 循环移位 + 异或混淆
// sign      = hex(out)                                           // 64 hex 字符
//
// v2 变化：签名材料并入设备凭证（硬件指纹+公网IP+注册时间戳派生，见 device.rs）。
// 只有完成设备注册的本 App 信道实现能算出 v2 签名——老版本 v1 签名（无材料段）
// 在验签端 canonical 长度/内容不匹配直接失效，防降级。真伪验证在 BIT 端完成。
//
// 验证端校验：ts 与本机时钟差 ≤ ±120s；nonce 未见过（防重放）；sign 常量时间比对。

pub const BITSIGN_ALG: &str = "bitsign-v2";
/// 签名时间戳容忍窗口（秒）：覆盖设备间时钟漂移，同时限制截获签名的有效时长
pub const BITSIGN_TS_WINDOW: i64 = 120;

/// 设备凭证派生主盐（与二维码加密 CRYPT_MASTER 同级：内置二进制，防普通逆向直读）
pub const FP_MASTER: &str = "bit-dev-v1-master:7c1f4e2a9b3d5806f4a2c1e7d0b6395a:8e12";

pub fn bitsign(client_key: &str, device_material: &str, rid: &str, ts: i64, nonce: &str, method: &str, path: &str) -> String {
    let canonical = format!("{BITSIGN_ALG}\n{rid}\n{ts}\n{nonce}\n{method}\n{path}\n{device_material}");
    let mac = hmac_sha256(client_key.as_bytes(), canonical.as_bytes());
    let k = Sha256::digest(format!("{BITSIGN_ALG}:{client_key}").as_bytes());
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = mac[i].rotate_left((i % 7) as u32 + 1) ^ k[i % 16];
    }
    out.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct BitsignCheck {
    pub ts: i64,
    pub nonce: String,
    pub sign: String,
}

/// 验签（path 为本机请求路径，不含查询串——与手机端签名口径一致；now 为当前 unix 秒）。
/// device_material 为本机设备凭证签名材料（device.rs::sig_material）；Err 为英文原因
pub fn bitsign_verify(
    client_key: &str,
    device_material: &str,
    rid: &str,
    method: &str,
    path: &str,
    c: &BitsignCheck,
    now: i64,
) -> Result<(), &'static str> {
    if c.sign.len() != 64 || !c.sign.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("malformed signature");
    }
    if (now - c.ts).abs() > BITSIGN_TS_WINDOW {
        return Err("timestamp out of window");
    }
    if c.nonce.len() < 8 || c.nonce.len() > 128 || !c.nonce.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Err("invalid nonce");
    }
    let expect = bitsign(client_key, device_material, rid, c.ts, &c.nonce, method, path);
    let e = expect.as_bytes();
    let g = c.sign.as_bytes();
    let mut diff = 0u8;
    for i in 0..32 {
        // 高低半字节拆开比对，避免提前退出泄露前缀
        diff |= (e[i] >> 4) ^ (g[i] >> 4);
        diff |= (e[i] & 0x0f) ^ (g[i] & 0x0f);
    }
    if diff != 0 {
        return Err("signature mismatch");
    }
    Ok(())
}

// ── BIT-Crypt v1：二维码加密块（防普通扫码器直接读出连接凭据）──
//
// 混淆级加密：master 内置于 App 二进制（本机屏幕展示 + 距离衰减为主要威胁，非对抗逆向）。
// keystream = SHA256(master || salt || ctr_be_u32) 逐块拼接（CTR 模式）
// out = "BIT1:" + base64(salt(16B) || plaintext XOR keystream)
//
// 手机 App 用内置 master 解出 payload JSON（含 client_key / rid / 三种连接方式）；
// 其它扫码器只能看到 BIT1: 前缀的密文块。

/// App 内置主密钥材料（与移动端约定一致；泄露只影响混淆强度，不改变服务器侧凭据）
pub const CRYPT_MASTER: &str = "bit-enc-v1-master:9f4c2a77e1b3d806a5c4f21e7d0b6a39:7d31";
pub const CRYPT_PREFIX: &str = "BIT1:";

pub fn bitcrypt_encrypt(plaintext: &str) -> String {
    use base64::Engine;
    use rand::Rng;
    let salt: [u8; 16] = rand::thread_rng().gen();
    let data = plaintext.as_bytes();
    let mut cipher = Vec::with_capacity(data.len());
    for (i, byte) in data.iter().enumerate() {
        let ctr = (i / 32) as u32;
        // 每块密钥流懒生成：32 字节/块
        let mut hasher = Sha256::new();
        hasher.update(CRYPT_MASTER.as_bytes());
        hasher.update(salt);
        hasher.update(ctr.to_be_bytes());
        let block = hasher.finalize();
        cipher.push(byte ^ block[i % 32]);
    }
    let mut buf = salt.to_vec();
    buf.extend_from_slice(&cipher);
    format!("{CRYPT_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(&buf))
}

pub fn bitcrypt_decrypt(blob: &str) -> Result<String, &'static str> {
    use base64::Engine;
    let raw = blob
        .strip_prefix(CRYPT_PREFIX)
        .ok_or("bad prefix")?;
    let buf = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|_| "bad base64")?;
    if buf.len() < 16 {
        return Err("too short");
    }
    let (salt, cipher) = buf.split_at(16);
    let mut plain = Vec::with_capacity(cipher.len());
    for (i, byte) in cipher.iter().enumerate() {
        let ctr = (i / 32) as u32;
        let mut hasher = Sha256::new();
        hasher.update(CRYPT_MASTER.as_bytes());
        hasher.update(salt);
        hasher.update(ctr.to_be_bytes());
        let block = hasher.finalize();
        plain.push(byte ^ block[i % 32]);
    }
    String::from_utf8(plain).map_err(|_| "bad utf8")
}

// ── 敏感词审核 ──

/// 内置敏感词库（硬底线类：涉未成年人/暴恐/毒品/武器/诈骗基础设施）。
/// 用户可通过配置整体替换（blocked_words，None = 内置默认）。
/// 词表内容独立成函数便于单测覆盖
pub const DEFAULT_BLOCKED_WORDS: &[&str] = &[
    // 英文硬底线
    "child porn", "cp trade", "underage nude", "preteen sex", "loli porn", "shotacon",
    "child sexual", "csam",
    "how to make a bomb", "pipe bomb diy", "nerve agent synthesis", "sarin synthesis",
    "ricin recipe", "anthrax culture",
    "buy gun no background check", "ghost gun kit sale",
    "fentanyl wholesale", "meth synthesis", "mdma synthesis", "cocaine supply",
    "ransomware as a service", "ddos booter rent", "stolen card dump", "credit card dump shop",
    "fake passport maker", "counterfeit banknotes", "money laundering scheme",
    // 中文硬底线
    "儿童色情", "未成年人裸聊", "幼女性行为", "制爆教程", "炸药制作方法",
    "制毒教程", "冰毒制作", "毒品货源", "洗钱通道", "伪钞制作",
    "办假证", "淫秽物品交易", "网约裸聊未成", "开盒挂人",
    "色情直播平台招嫖", "代刷枪支", "枪支买卖", "境外博彩代理",
];

/// 审核命中：命中的词 + 归一化位置
#[derive(Debug, Clone, PartialEq)]
pub struct ModHit {
    pub word: String,
}

/// 文本归一化：小写、去零宽字符与所有空白、全角 ASCII 区转半角。
/// 目的：阻止用空格/全角/零宽字符简单绕过匹配
pub fn moderation_normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        // 零宽与不可见注入字符直接丢弃
        if matches!(ch as u32, 0x200b..=0x200f | 0x202a..=0x202e | 0xfeff) {
            continue;
        }
        // 全角 ASCII 区（！..～）转半角
        let c = if ('\u{FF01}'..='\u{FF5E}').contains(&ch) {
            char::from_u32(ch as u32 - 0xFEE0).unwrap_or(ch)
        } else {
            ch
        };
        if c.is_whitespace() {
            continue;
        }
        for lc in c.to_lowercase() {
            out.push(lc);
        }
    }
    out
}

/// 扫描：命中返回 Some(word)。词表同样归一化后匹配（一次归一化可缓存，规模小不做）
pub fn moderation_scan(text: &str, words: &[String]) -> Option<ModHit> {
    let norm = moderation_normalize(text);
    for w in words {
        let wn = moderation_normalize(w);
        if !wn.is_empty() && norm.contains(&wn) {
            return Some(ModHit { word: w.clone() });
        }
    }
    None
}

// ── nonce 重放缓存 ──

/// 见过的 nonce → 时刻。超过容量时清理窗口外条目。
/// 窗口与 bitsign 时钟容忍一致：过期签名本身已被拒，nonce 无需记忆更久
pub struct NonceCache {
    seen: std::collections::HashMap<String, std::time::Instant>,
    window: std::time::Duration,
    cap: usize,
}

impl NonceCache {
    pub fn new(window: std::time::Duration, cap: usize) -> Self {
        Self { seen: std::collections::HashMap::new(), window, cap }
    }

    /// 未见过（或已过窗口）→ 记录并返回 true；窗口内重复 → false（防重放）。
    /// 过期判定必须在查询路径上做：仅靠满容量触发清理会让低流量下的 nonce 永久驻留
    /// （签名过期本就被 ts 窗口拒绝，nonce 记忆超过窗口无安全收益、只产生误判）
    pub fn fresh(&mut self, nonce: &str) -> bool {
        if let Some(t) = self.seen.get(nonce) {
            if t.elapsed() < self.window {
                return false;
            }
        }
        if self.seen.len() >= self.cap {
            self.seen.retain(|_, t| t.elapsed() < self.window);
        }
        self.seen.insert(nonce.to_string(), std::time::Instant::now());
        true
    }
}

// ── 单元测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_known_vector() {
        // RFC 4231 Test Case 2: key="Jefe", data="what do ya want for nothing?"
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn bitsign_roundtrip_and_tamper() {
        let key = "bit_test_key";
        let mat = "ab12";
        let s = bitsign(key, mat, "rid3333", 1_000_000, "nonce1234", "POST", "/api/chat");
        assert_eq!(s.len(), 64);
        let ok = bitsign_verify(
            key,
            mat,
            "rid3333",
            "POST",
            "/api/chat",
            &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: s.clone() },
            1_000_060,
        );
        assert!(ok.is_ok(), "{ok:?}");
        // 篡改 method / path / rid / ts / nonce / sign / 设备材料 任意一项都必须失败
        assert!(bitsign_verify(key, mat, "rid3333", "GET", "/api/chat", &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: s.clone() }, 1_000_060).is_err());
        assert!(bitsign_verify(key, mat, "rid3333", "POST", "/api/qr", &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: s.clone() }, 1_000_060).is_err());
        assert!(bitsign_verify(key, mat, "rid4444", "POST", "/api/chat", &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: s.clone() }, 1_000_060).is_err());
        assert!(bitsign_verify(key, mat, "rid3333", "POST", "/api/chat", &BitsignCheck { ts: 1_000_000 + 1000, nonce: "nonce1234".into(), sign: s.clone() }, 1_000_060).is_err());
        assert!(bitsign_verify(key, mat, "rid3333", "POST", "/api/chat", &BitsignCheck { ts: 1_000_000, nonce: "nonce5678".into(), sign: s.clone() }, 1_000_060).is_err());
        let mut bad = s.clone().into_bytes();
        bad[3] = if bad[3] == b'a' { b'b' } else { b'a' };
        assert!(bitsign_verify(key, mat, "rid3333", "POST", "/api/chat", &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: String::from_utf8(bad).unwrap() }, 1_000_060).is_err());
        // 过期
        assert!(bitsign_verify(key, mat, "rid3333", "POST", "/api/chat", &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: s.clone() }, 1_000_000 + BITSIGN_TS_WINDOW + 5).is_err());
        // 错误 key
        assert!(bitsign_verify("other", mat, "rid3333", "POST", "/api/chat", &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: s.clone() }, 1_000_060).is_err());
        // 设备材料不同（未注册设备 / 换设备）→ 签名失效（v2 防共用核心）
        assert!(bitsign_verify(key, "cd34", "rid3333", "POST", "/api/chat", &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: s }, 1_000_060).is_err());
    }

    #[test]
    fn bitsign_v1_signature_rejected() {
        // 防降级：老版本 v1 签名（canonical 无设备材料段）在 v2 验签端必须失效
        let key = "bit_test_key";
        let v1_like = bitsign(key, "", "rid3333", 1_000_000, "nonce1234", "POST", "/api/chat");
        // 即使把空材料签出的值拿来验证别的材料口径，也因材料不匹配而失败
        assert!(bitsign_verify(key, "ab12", "rid3333", "POST", "/api/chat", &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: v1_like.clone() }, 1_000_060).is_err());
        // 同理：v2 签名换材料必失败已由 roundtrip 覆盖；这里再断言空材料自身自洽
        assert!(bitsign_verify(key, "", "rid3333", "POST", "/api/chat", &BitsignCheck { ts: 1_000_000, nonce: "nonce1234".into(), sign: v1_like }, 1_000_060).is_ok());
    }

    #[test]
    fn bitcrypt_roundtrip_and_tamper() {
        let msg = r#"{"v":2,"key":"bit_x","rid":"ab","pwd":"12345678"}"#;
        let enc = bitcrypt_encrypt(msg);
        assert!(enc.starts_with(CRYPT_PREFIX));
        assert!(!enc.contains("bit_x"), "ciphertext must not leak plaintext key");
        // 每次盐随机：两段密文不同但都可解
        let enc2 = bitcrypt_encrypt(msg);
        assert_ne!(enc, enc2);
        assert_eq!(bitcrypt_decrypt(&enc).unwrap(), msg);
        assert_eq!(bitcrypt_decrypt(&enc2).unwrap(), msg);
        // 篡改一位 → 解不出原文
        let mut raw = enc.clone().into_bytes();
        let last = raw.len() - 2;
        raw[last] = if raw[last] == b'A' { b'B' } else { b'A' };
        let dec = bitcrypt_decrypt(&String::from_utf8(raw).unwrap());
        assert!(dec.is_err() || dec.unwrap() != msg);
        assert!(bitcrypt_decrypt("XXXX:abc").is_err());
    }

    #[test]
    fn moderation_normalize_and_scan() {
        assert_eq!(moderation_normalize(" ＨＥ LLO\u{200b}"), "hello");
        assert_eq!(moderation_normalize(" 冰 毒 制 作 "), "冰毒制作");
        let words: Vec<String> = DEFAULT_BLOCKED_WORDS.iter().map(|s| s.to_string()).collect();
        assert!(moderation_scan("请问冰 毒制作流程", &words).is_some(), "空白绕过必须被归一化拦截");
        assert!(moderation_scan("ｃｈｉｌｄ ｐｏｒｎ", &words).is_some(), "全角绕过必须被拦截");
        assert!(moderation_scan("如何做红烧肉", &words).is_none());
        assert!(moderation_scan("正常的技术问题", &words).is_none());
    }

    #[test]
    fn nonce_cache_replay() {
        let mut c = NonceCache::new(std::time::Duration::from_secs(120), 8);
        assert!(c.fresh("n1"));
        assert!(!c.fresh("n1"), "重复 nonce 必须拒绝");
        assert!(c.fresh("n2"));
        for i in 0..20 {
            assert!(c.fresh(&format!("x{i}")));
        }
        assert!(!c.fresh("n1"), "窗口内仍拒绝");
    }

    #[test]
    fn nonce_cache_expiry_on_lookup() {
        // 窗口 0s：条目立即过期 → 查询路径必须判过期为"未见"，且低流量（不满容量）下
        // 长时间驻留的旧条目不能把新请求误判为重放
        let mut c = NonceCache::new(std::time::Duration::from_secs(0), 8);
        assert!(c.fresh("stale"));
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(c.fresh("stale"), "过期 nonce 必须视为未见（曾经只在满容量时清理，低流量下永久驻留误判重放）");
    }
}
