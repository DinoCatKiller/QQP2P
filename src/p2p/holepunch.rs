//! STUN 映射查询 + UDP 打洞
//!
//! 从 legacy/holepunch-mvp 移植精简版，仅保留 STUN 查询和打洞核心逻辑。
//! 关键原则：STUN 查询与打洞必须使用**同一个 UDP socket**（映射一致性）。

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::{Context, Result};
use rand::RngCore;
use tokio::net::UdpSocket;

/// STUN Magic Cookie（RFC 5389）
const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
/// Binding Request 消息类型
const STUN_MSG_BINDING_REQUEST: u16 = 0x0001;
/// Binding Response 消息类型
const STUN_MSG_BINDING_RESPONSE: u16 = 0x0101;
/// XOR-MAPPED-ADDRESS 属性类型
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
/// MAPPED-ADDRESS 属性类型（旧版兼容）
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;

/// STUN 查询超时
const STUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// 解析 STUN 服务器地址（支持域名:port 和 IP:port）
pub async fn resolve_stun_server(s: &str) -> Result<SocketAddr> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let (host, port) = s.rsplit_once(':').context("STUN 地址格式应为 host:port")?;
    let port: u16 = port.parse().context("STUN 端口无效")?;
    let mut addrs = tokio::net::lookup_host((host, port)).await?;
    addrs
        .find(|a| a.is_ipv4())
        .context("STUN 域名解析失败(无 IPv4 记录)")
}

/// 构造 STUN Binding Request（20 字节）
pub fn build_binding_request(transaction_id: [u8; 12]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(20);
    buf.extend_from_slice(&STUN_MSG_BINDING_REQUEST.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // 属性长度 = 0
    buf.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    buf.extend_from_slice(&transaction_id);
    buf
}

/// 从 STUN Binding Response 解析映射地址
pub fn parse_stun_response(buf: &[u8]) -> Result<SocketAddr> {
    if buf.len() < 20 {
        anyhow::bail!("STUN 响应过短: {} 字节", buf.len());
    }
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != STUN_MSG_BINDING_RESPONSE {
        anyhow::bail!("非 Binding Response: 0x{:04x}", msg_type);
    }
    let cookie = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if cookie != STUN_MAGIC_COOKIE {
        anyhow::bail!("Magic Cookie 不匹配");
    }
    let attrs_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let end = (20 + attrs_len).min(buf.len());
    let mut offset = 20;
    while offset + 4 <= end {
        let attr_type = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let attr_len = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
        let value_start = offset + 4;
        let value_end = (value_start + attr_len).min(end);
        match attr_type {
            ATTR_XOR_MAPPED_ADDRESS if value_end >= value_start + 8 && buf[value_start + 1] == 0x01 => {
                let x_port = u16::from_be_bytes([buf[value_start + 2], buf[value_start + 3]]);
                let port = x_port ^ (STUN_MAGIC_COOKIE >> 16) as u16;
                let ip = Ipv4Addr::new(
                    buf[value_start + 4] ^ (STUN_MAGIC_COOKIE >> 24) as u8,
                    buf[value_start + 5] ^ (STUN_MAGIC_COOKIE >> 16) as u8,
                    buf[value_start + 6] ^ (STUN_MAGIC_COOKIE >> 8) as u8,
                    buf[value_start + 7] ^ STUN_MAGIC_COOKIE as u8,
                );
                return Ok(SocketAddr::new(ip.into(), port));
            }
            ATTR_MAPPED_ADDRESS if value_end >= value_start + 8 && buf[value_start + 1] == 0x01 => {
                let port = u16::from_be_bytes([buf[value_start + 2], buf[value_start + 3]]);
                let ip = Ipv4Addr::new(
                    buf[value_start + 4],
                    buf[value_start + 5],
                    buf[value_start + 6],
                    buf[value_start + 7],
                );
                return Ok(SocketAddr::new(ip.into(), port));
            }
            _ => {}
        }
        let padding = (4 - attr_len % 4) % 4;
        offset = value_end + padding;
    }
    anyhow::bail!("STUN 响应中未找到映射地址属性")
}

/// 通过 STUN 服务器查询本机 NAT 映射地址
pub async fn query_mapped_addr(sock: &UdpSocket, stun_server: SocketAddr) -> Result<SocketAddr> {
    let mut tx_id = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut tx_id);
    let req = build_binding_request(tx_id);
    sock.send_to(&req, stun_server)
        .await
        .context("发送 STUN Binding Request 失败")?;

    let mut buf = [0u8; 1500];
    let deadline = tokio::time::sleep(STUN_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => anyhow::bail!("STUN 查询超时"),
            res = sock.recv_from(&mut buf) => {
                let (n, _src) = res.context("接收 STUN 响应失败")?;
                if n < 20 { continue; }
                if u16::from_be_bytes([buf[0], buf[1]]) != STUN_MSG_BINDING_RESPONSE { continue; }
                if buf[8..20] != tx_id { continue; }
                return parse_stun_response(&buf[..n]);
            }
        }
    }
}

/// STUN 查询带重试（3 次，间隔 500ms）
pub async fn query_mapped_addr_retry(sock: &UdpSocket, stun_server: SocketAddr) -> Result<SocketAddr> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=3 {
        match query_mapped_addr(sock, stun_server).await {
            Ok(m) => return Ok(m),
            Err(e) => {
                eprintln!("[!] STUN 查询失败(第{}次): {}", attempt, e);
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("STUN 查询失败")))
}

/// NAT 类型探测结果
#[derive(Debug, Clone)]
pub enum NatType {
    /// 两次 STUN 映射端口一致 → Cone NAT (打洞可行)
    Cone { mapped: SocketAddr },
    /// 两次 STUN 映射端口不同 → Symmetric NAT (纯 UDP 打洞基本无解)
    Symmetric { mapped1: SocketAddr, mapped2: SocketAddr },
    /// 探测失败
    Unknown { reason: String },
}

impl NatType {
    #[allow(dead_code)]
    pub fn is_symmetric(&self) -> bool {
        matches!(self, NatType::Symmetric { .. })
    }

    /// 返回主映射地址 (Symmetric 时返回第一次查到的)
    #[allow(dead_code)]
    pub fn mapped(&self) -> Option<SocketAddr> {
        match self {
            NatType::Cone { mapped } => Some(*mapped),
            NatType::Symmetric { mapped1, .. } => Some(*mapped1),
            NatType::Unknown { .. } => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            NatType::Cone { .. } => "Cone (端口稳定, 打洞可行)",
            NatType::Symmetric { .. } => "Symmetric (端口变化, 纯 UDP 打洞基本无解)",
            NatType::Unknown { .. } => "未知",
        }
    }
}

/// NAT 类型探测：向两个不同 STUN 服务器查询映射地址，比对端口
///
/// - 端口一致 → Cone NAT (打洞有戏)
/// - 端口变化 → Symmetric NAT (纯 UDP 打洞基本无解)
///
/// 注意：必须用同一 socket 查询，否则测的不是 NAT 行为而是 socket 行为
pub async fn detect_nat_type(sock: &UdpSocket, stun_primary: SocketAddr, stun_secondary: SocketAddr) -> NatType {
    let m1 = match query_mapped_addr_retry(sock, stun_primary).await {
        Ok(m) => m,
        Err(e) => return NatType::Unknown { reason: format!("主 STUN 查询失败: {}", e) },
    };
    let m2 = match query_mapped_addr_retry(sock, stun_secondary).await {
        Ok(m) => m,
        Err(e) => return NatType::Unknown { reason: format!("备 STUN 查询失败: {}", e) },
    };

    if m1.port() == m2.port() {
        NatType::Cone { mapped: m1 }
    } else {
        NatType::Symmetric { mapped1: m1, mapped2: m2 }
    }
}

/// 双方同时 UDP 打洞
///
/// 双方同时向对方映射地址发探测包，互开 NAT 洞。
///
/// 退出条件（任一满足即 Ok）：
/// - 收到对方 ACK：对方收到我方探测包才回 ACK → 双向都通
/// - 收到对方探测包：对方→我方 通，立即回 ACK 让对方也能判定
///
/// 返回 Ok(()) 表示打洞成功。
pub async fn hole_punch(
    sock: &UdpSocket,
    peer_mapped: SocketAddr,
    my_uid: u64,
    timeout_secs: u64,
) -> Result<()> {
    let mut buf = [0u8; 1500];
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs));
    tokio::pin!(deadline);

    let send_interval = tokio::time::interval(std::time::Duration::from_millis(200));
    tokio::pin!(send_interval);

    println!("[*] 开始打洞 → {} (超时 {}s)", peer_mapped, timeout_secs);

    loop {
        tokio::select! {
            _ = &mut deadline => {
                anyhow::bail!("打洞超时 ({}s)", timeout_secs);
            }
            _ = send_interval.tick() => {
                // 持续发探测包开洞 (收到对方任何包即退出, 不再需要 ACK 模式切换)
                let msg = format!("HOLEPUNCH {}", my_uid);
                let _ = sock.send_to(msg.as_bytes(), peer_mapped).await;
            }
            res = sock.recv_from(&mut buf) => {
                let (n, src) = res.context("打洞阶段 recv 失败")?;
                let line = String::from_utf8_lossy(&buf[..n]).trim().to_string();

                if line.starts_with("HOLEPUNCH-ACK") {
                    // 对方收到我方探测包才回 ACK → 我方→对方 通 (对方发包到我也通了)
                    println!("[+] 收到 ACK from {} → 双向连通, 打洞成功", src);
                    return Ok(());
                } else if line.starts_with("HOLEPUNCH") {
                    // 收到对方探测包 = 对方→我方通; UDP NAT 洞双向, 即证明双向连通
                    // (旧逻辑"等 ACK 再退"会死锁: 对方收到我方 ACK 退出后不再发 ACK,
                    //  我方永远等不到 → 超时。故收到任何打洞包即退出, 并回 ACK 帮对方退出)
                    println!("[+] 收到探测包 from {} → 双向连通, 打洞成功", src);
                    let ack = format!("HOLEPUNCH-ACK {}", my_uid);
                    let _ = sock.send_to(ack.as_bytes(), src).await;
                    return Ok(());
                }
            }
        }
    }
}
