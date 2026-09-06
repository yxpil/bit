// yxpil · BIT
// 网络探测：网卡地址枚举 + 极简 STUN 公网映射查询。
// 用途：远程二维码携带全部候选连接地址（LAN IPv4 / 全球 IPv6 / 公网映射），
// 手机 App 扫码后按候选依次尝试直连（局域网 → IPv6 公网 → 云中继）。
use std::net::{IpAddr, SocketAddr};

/// 全球单播 IPv6（2000::/3）：std 的 is_global 尚未全平台稳定，手动判前 3 位
fn is_global_v6(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V6(v6) => v6.octets()[0] & 0xE0 == 0x20,
        _ => false,
    }
}

fn is_private_v4(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
        }
        _ => false,
    }
}

/// 极简 STUN Binding（RFC 5389）：只发请求、只解析 XOR-MAPPED-ADDRESS。
/// server 形如 "stun.qq.com:3478" / "162.159.200.1:3478" / "[2606:4700:4700::a29f:2001]:3478"。
/// 返回 (v4 映射, v6 映射)：域名解析后各取首个对应族的地址探测，两族互不干扰。
async fn stun_mapped(server: &str) -> (Option<SocketAddr>, Option<SocketAddr>) {
    use rand::Rng;
    // 域名/IP 统一解析（tokio 异步解析，失败 → 空列表该服务器跳过）
    let addrs: Vec<SocketAddr> = match tokio::net::lookup_host(server).await {
        Ok(it) => it.collect(),
        Err(_) => Vec::new(),
    };
    let v4 = addrs.iter().find(|a| a.is_ipv4()).copied();
    let v6 = addrs.iter().find(|a| a.is_ipv6()).copied();
    let query = |addr: SocketAddr| async move {
        // 请求头：type=0x0001(Binding) / len=0 / magic cookie / 12 字节随机事务 ID
        let mut req = Vec::with_capacity(20);
        req.extend_from_slice(&0x0001u16.to_be_bytes());
        req.extend_from_slice(&0u16.to_be_bytes());
        req.extend_from_slice(&0x2112A44Du32.to_be_bytes());
        let txid: [u8; 12] = rand::thread_rng().gen();
        req.extend_from_slice(&txid);

        let bind: &str = if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        let sock = tokio::net::UdpSocket::bind(bind).await.ok()?;
        sock.connect(addr).await.ok()?;
        sock.send(&req).await.ok()?;
        let mut buf = vec![0u8; 1024];
        let n = tokio::time::timeout(std::time::Duration::from_secs(3), sock.recv(&mut buf))
            .await
            .ok()?
            .ok()?;
        // 响应：type=0x0101(Binding 成功) + 同 magic cookie
        if n < 20 || buf[0] != 0x01 || buf[1] != 0x01 {
            return None;
        }
        if u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) != 0x2112A44D {
            return None;
        }
        let mut i = 20;
        while i + 4 <= n {
            let attr_type = u16::from_be_bytes([buf[i], buf[i + 1]]);
            let attr_len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
            if attr_type == 0x0020 && attr_len >= 8 {
                // XOR-MAPPED-ADDRESS：value[0]=0 保留 / [1]=family / [2..4]=异或端口 / [4..]=异或地址
                let family = buf[i + 5];
                let port = u16::from_be_bytes([buf[i + 6], buf[i + 7]]) ^ 0x2112;
                let magic = [0x21u8, 0x12, 0xA4, 0x4D];
                let ip = match family {
                    0x01 if attr_len >= 8 => {
                        let mut o = [0u8; 4];
                        for (k, b) in o.iter_mut().enumerate() {
                            *b = buf[i + 8 + k] ^ magic[k];
                        }
                        IpAddr::from(o)
                    }
                    0x02 if attr_len >= 20 => {
                        let mut o = [0u8; 16];
                        for (k, b) in o.iter_mut().enumerate() {
                            *b = buf[i + 8 + k] ^ magic[k % 4];
                        }
                        IpAddr::from(o)
                    }
                    _ => return None,
                };
                return Some(SocketAddr::new(ip, port));
            }
            i += 4 + attr_len + ((4 - attr_len % 4) % 4); // 跳过 4 字节对齐填充
        }
        None
    };
    let (r4, r6) = tokio::join!(
        async move { match v4 { Some(a) => query(a).await, None => None } },
        async move { match v6 { Some(a) => query(a).await, None => None } }
    );
    (r4, r6)
}

/// 内置免费 STUN 服务器（host:port，v4/v6 由解析结果决定）：全部并发探测，
/// 取前两个成功映射做 NAT 粗判。国内（腾讯/小米）与海外（Cloudflare/Google/Twilio/
/// Nextcloud/Syncthing/sipgate/ideasiP/voipgate）混编，任一可达即可用；
/// 不可达条目仅 3s UDP 超时无副作用；用户可在设置里整体替换为自定列表
pub const DEFAULT_STUN_SERVERS: &[&str] = &[
    "stun.cloudflare.com:3478",
    "stun.qq.com:3478",
    "stun.l.google.com:19302",
    "stun.miwifi.com:3478",
    "global.stun.twilio.com:3478",
    "stun.nextcloud.com:443",
    "stun.syncthing.net:3478",
    "stun.sipgate.net:3478",
    "stun.ideasip.com:3478",
    "stun.voipgate.com:3478",
];

/// 局域网/公网候选地址快照：手机端按 lan → lan6 → pub6/pub4 依次尝试直连。
/// custom 为用户自定 STUN 列表（None = 用内置默认）
pub async fn lan_probe(custom: Option<&[String]>) -> serde_json::Value {
    let mut lan: Vec<String> = Vec::new();
    let mut lan6: Vec<String> = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for it in ifaces {
            let ip = it.ip();
            match ip {
                IpAddr::V4(_) if is_private_v4(&ip) => lan.push(ip.to_string()),
                IpAddr::V6(_) if is_global_v6(&ip) => lan6.push(ip.to_string()),
                _ => {}
            }
        }
    }
    // STUN 并发探测：列表内所有服务器同时发 Binding（UDP 轻量，3s 超时），
    // 按列表顺序取前两个 v4 成功映射做 NAT 粗判；v6 取首个成功映射
    let list: Vec<String> = custom
        .filter(|l| !l.is_empty())
        .map(|l| l.to_vec())
        .unwrap_or_else(|| DEFAULT_STUN_SERVERS.iter().map(|s| s.to_string()).collect());
    let probes = futures_util::future::join_all(list.iter().map(|s| stun_mapped(s))).await;
    let mut v4_hits = probes.iter().filter_map(|(a, _)| *a);
    let pub4 = v4_hits.next();
    let pub4_alt = v4_hits.next();
    let pub6 = probes
        .iter()
        .find_map(|(_, b)| *b)
        .map(|s| s.to_string())
        .or_else(|| lan6.first().cloned());
    // NAT 粗判（启发式）：
    // none      — 映射地址与本机网卡地址一致（无 NAT / 直接公网）
    // cone      — 两台 STUN 映射端口一致（Endpoint-Independent Mapping，NAT1 特征，可尝试直连）
    // symmetric — 两台映射端口不一致（对称型，直连无望走中继）
    // unknown   — STUN 不可达
    let nat = match (pub4, pub4_alt) {
        (Some(x), _) if lan.iter().any(|l| *l == x.ip().to_string()) => "none",
        (Some(x), Some(y)) if x.port() == y.port() => "cone",
        (Some(_), Some(_)) => "symmetric",
        (Some(_), None) => "unknown",
        _ => "unknown",
    };
    serde_json::json!({
        "lan": lan,
        "lan6": lan6,
        "pub4": pub4.map(|s| s.to_string()),
        "pub4_alt": pub4_alt.map(|s| s.to_string()),
        "pub6": pub6,
        "nat": nat,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_v6_detection() {
        assert!(is_global_v6(&"240e:1234::1".parse::<IpAddr>().unwrap()));
        assert!(is_global_v6(&"2606:4700:4700::a29f:2001".parse::<IpAddr>().unwrap()));
        assert!(!is_global_v6(&"fe80::1".parse::<IpAddr>().unwrap())); // 链路本地
        assert!(!is_global_v6(&"::1".parse::<IpAddr>().unwrap())); // 回环
        assert!(!is_global_v6(&"192.168.1.1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn private_v4_detection() {
        for ok in ["10.0.0.5", "172.16.1.1", "172.31.9.9", "192.168.0.10"] {
            assert!(is_private_v4(&ok.parse::<IpAddr>().unwrap()), "{ok}");
        }
        for no in ["8.8.8.8", "172.32.0.1", "11.0.0.1", "169.254.1.1", "127.0.0.1"] {
            assert!(!is_private_v4(&no.parse::<IpAddr>().unwrap()), "{no}");
        }
    }
}
