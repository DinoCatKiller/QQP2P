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

/// 双方同时 UDP 打洞
///
/// 双方同时向对方映射地址发探测包，互开 NAT 洞。
/// 收到对方探测包即回 ACK，互收 ACK 判定连通。
///
/// 返回 Ok(()) 表示打洞成功。
pub async fn hole_punch(
    sock: &UdpSocket,
    peer_mapped: SocketAddr,
    my_uid: u64,
    timeout_secs: u64,
) -> Result<()> {
    let probe = format!("HOLEPUNCH {}", my_uid);
    let mut buf = [0u8; 1500];
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs));
    tokio::pin!(deadline);

    let send_interval = tokio::time::interval(std::time::Duration::from_millis(200));
    tokio::pin!(send_interval);

    let mut got_probe = false;
    let mut got_ack = false;

    println!("[*] 开始打洞 → {} (超时 {}s)", peer_mapped, timeout_secs);

    loop {
        tokio::select! {
            _ = &mut deadline => {
                if got_ack {
                    return Ok(());
                }
                anyhow::bail!("打洞超时 ({}s)", timeout_secs);
            }
            _ = send_interval.tick() => {
                // 每次发探测包（带 ACK 如果已收到对方探测）
                let msg = if got_probe {
                    format!("HOLEPUNCH-ACK {}", my_uid)
                } else {
                    format!("HOLEPUNCH {}", my_uid)
                };
                let _ = sock.send_to(msg.as_bytes(), peer_mapped).await;
            }
            res = sock.recv_from(&mut buf) => {
                let (n, src) = res.context("打洞阶段 recv 失败")?;
                let line = String::from_utf8_lossy(&buf[..n]).trim().to_string();

                if line.starts_with("HOLEPUNCH-ACK") {
                    println!("[+] 收到 ACK from {}", src);
                    got_ack = true;
                    if got_probe { return Ok(()); }
                } else if line.starts_with("HOLEPUNCH") {
                    println!("[+] 收到探测包 from {}", src);
                    got_probe = true;
                    // 立即回 ACK
                    let ack = format!("HOLEPUNCH-ACK {}", my_uid);
                    let _ = sock.send_to(ack.as_bytes(), src).await;
                }
            }
        }
    }
}
