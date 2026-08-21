//! NAT 打洞模块（M1）：STUN 映射获取 + UDP Hole Punching 会话
//!
//! 核心原则：STUN 查询与打洞发包必须使用**同一个 UDP socket**（映射一致性），
//! 否则拿到的映射端口与发包端口不一致，打洞必然失败。
//!
//! 打洞流程：
//! 1. 启动时绑定 UDP socket 并向 STUN 服务器查询本机 NAT 映射地址（`P2PNode::start_udp_server`），
//!    随后后台保活任务周期性刷新映射，等待/信令交换期间地址长期有效（`keepalive_loop`）
//! 2. 节点信息消息附带打洞映射行（`📡 P2P打洞: udp://ip:port 会话=xxx`），经 QQ 信令交换
//! 3. 收到对方映射后，双方同时向对方映射地址发包（200ms × 50 次/轮 ≈ 10 秒），
//!    未连通自动重试下一轮（`start_hole_punch_retry`）
//! 4. 探测包携带本机最新映射地址（`HOLEPUNCH <sid> <uid> <my_mapped>`），对端收到后动态更新，
//!    映射端口变化时仍能继续打新地址
//! 5. 收到 HOLEPUNCH 探测包即回 HOLEPUNCH-ACK，互收 ACK 判定连通
//!
//! 探测/确认报文为明文 UTF-8（M1 不加密、不重传、不传文件）。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rand::RngCore;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

use crate::p2p::{BotEvent, HolePunchSession, P2PNode, SessionState};

/// STUN Magic Cookie（RFC 5389）
const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
/// Binding Request 消息类型
const STUN_MSG_BINDING_REQUEST: u16 = 0x0001;
/// Binding Response 消息类型
const STUN_MSG_BINDING_RESPONSE: u16 = 0x0101;
/// XOR-MAPPED-ADDRESS 属性类型
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
/// MAPPED-ADDRESS 属性类型（旧版服务器兼容）
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;

/// 打洞探测包前缀："HOLEPUNCH <session_id> <user_id> <my_mapped>"
/// 探测包携带本机最新映射地址, 对端收到后动态更新我方地址(映射端口变化仍能继续打洞)
pub const PROBE_PREFIX: &str = "HOLEPUNCH";
/// 打洞确认包前缀："HOLEPUNCH-ACK <session_id>"
pub const ACK_PREFIX: &str = "HOLEPUNCH-ACK";
/// 节点信息消息中的打洞行关键词
pub const SIGNAL_PREFIX: &str = "P2P打洞:";
/// 会话 ID 关键词
pub const SESSION_KEYWORD: &str = "会话=";

/// STUN 查询超时
const STUN_TIMEOUT: Duration = Duration::from_secs(3);
/// 探测包发送间隔
const PROBE_INTERVAL: Duration = Duration::from_millis(200);
/// 最大发包次数（200ms × 50 ≈ 10 秒）
const MAX_PROBES: usize = 50;
/// 打洞重试轮间隔（一轮未连通后等待再试）
const RETRY_INTERVAL: Duration = Duration::from_secs(3);

/// 将 `stun.l.google.com:19302` 形式的字符串解析为 SocketAddr（域名走 DNS 查询）
pub async fn resolve_stun_server(s: &str) -> Result<SocketAddr> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let (host, port) = s.rsplit_once(':').context("STUN 地址格式应为 host:port")?;
    let port: u16 = port.parse().context("STUN 端口无效")?;
    let mut addrs = tokio::net::lookup_host((host, port)).await?;
    addrs.next().context("STUN 域名解析失败")
}

/// 构造 STUN Binding Request（20 字节 header，Length=0）
pub fn build_binding_request(transaction_id: [u8; 12]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(20);
    buf.extend_from_slice(&STUN_MSG_BINDING_REQUEST.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // 属性长度 = 0
    buf.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    buf.extend_from_slice(&transaction_id);
    buf
}

/// 从 STUN Binding Response 中解析映射地址。
/// 优先解析 XOR-MAPPED-ADDRESS（RFC 5389），兼容旧版 MAPPED-ADDRESS。
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
            // 1(reserved) + 1(family) + 2(x-port) + 4(x-address) = 8 字节
            ATTR_XOR_MAPPED_ADDRESS
                if value_end >= value_start + 8 && buf[value_start + 1] == 0x01 =>
            {
                let x_port = u16::from_be_bytes([buf[value_start + 2], buf[value_start + 3]]);
                // XOR 端口 = 端口 ^ (cookie >> 16)
                let port = x_port ^ (STUN_MAGIC_COOKIE >> 16) as u16;
                // XOR IPv4 = IP ^ cookie(32 位，大端逐字节)
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
        // 属性值按 4 字节对齐
        let padding = (4 - attr_len % 4) % 4;
        offset = value_end + padding;
    }
    anyhow::bail!("STUN 响应中未找到映射地址属性")
}

/// 通过 STUN 服务器查询本机 NAT 映射地址。
/// ⚠️ 使用传入的 socket 发送，返回的映射地址**只对该 socket 有效**（映射一致性）。
/// 响应按 Transaction ID 匹配，避免与其他共享收发（打洞探测包）混淆。
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
                if n < 20 {
                    continue;
                }
                // 只接受与本请求 Transaction ID 匹配的 Binding Response
                if u16::from_be_bytes([buf[0], buf[1]]) != STUN_MSG_BINDING_RESPONSE {
                    continue;
                }
                if buf[8..20] != tx_id {
                    continue;
                }
                return parse_stun_response(&buf[..n]);
            }
        }
    }
}

/// STUN 查询带重试：网络抖动导致单次超时/丢包时最多尝试 3 次，间隔 500ms。
/// 同样遵循映射一致性（使用传入的同一 socket）。
pub async fn query_mapped_addr_retry(sock: &UdpSocket, stun_server: SocketAddr) -> Result<SocketAddr> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=3 {
        match query_mapped_addr(sock, stun_server).await {
            Ok(m) => return Ok(m),
            Err(e) => {
                eprintln!("[!] STUN 查询失败(第{}次): {}", attempt, e);
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("STUN 查询失败")))
}

/// 从节点信息消息中提取对方打洞映射与会话 ID。
/// 格式示例: `📡 P2P打洞: udp://1.2.3.4:54321 会话=123456789`
/// 返回 (映射地址, 会话ID)；未附带打洞行或格式无效时返回 None。
pub fn extract_holepunch_info(message: &str) -> Option<(SocketAddr, u64)> {
    // 兼容全角冒号与字面 "\n"（与节点信息解析保持一致的规范化）
    let normalized = message.replace('：', ":").replace("\\n", "\n");
    let idx = normalized.find(SIGNAL_PREFIX)?;
    let rest = normalized[idx + SIGNAL_PREFIX.len()..].trim();

    let udp_idx = rest.find("udp://")?;
    let after_udp = &rest[udp_idx + "udp://".len()..];

    // 提取 "ip:port"（IPv4 形式）
    let addr_part: String = after_udp
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ':')
        .collect();
    let (host, port) = addr_part.split_once(':')?;
    let ip: Ipv4Addr = host.parse().ok()?;
    let port: u16 = port.parse().ok()?;
    if port == 0 {
        return None;
    }

    // 提取会话 ID（可选，仅用于日志追踪；缺失记为 0）
    let sid = after_udp
        .find(SESSION_KEYWORD)
        .and_then(|i| {
            let digits: String = after_udp[i + SESSION_KEYWORD.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            digits.parse::<u64>().ok()
        })
        .unwrap_or(0);

    Some((SocketAddr::new(ip.into(), port), sid))
}

/// UDP 监听循环：处理 HOLEPUNCH 探测包与 HOLEPUNCH-ACK 确认包。
/// 探测包 → 立即回 ACK（来源地址即对方 NAT 映射）；ACK → 匹配会话标记连通。
pub async fn run_udp_listener(node: Arc<Mutex<P2PNode>>) -> Result<()> {
    let sock = {
        let n = node.lock().await;
        n.udp_sock.clone()
    };
    let Some(sock) = sock else {
        anyhow::bail!("UDP socket 未初始化");
    };

    let mut buf = [0u8; 1500];
    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                // Windows: 对端 UDP 端口已关闭(ICMP)时 recv_from 报 10054(ConnectionReset),
                // 且错误状态粘滞。这在打洞场景属异常(对端退出), 不崩监听循环, 稍候重试
                if e.kind() == std::io::ErrorKind::ConnectionReset {
                    eprintln!("[!] UDP收到ICMP重置(10054), 监听循环继续");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                anyhow::bail!("UDP监听循环退出: {}", e);
            }
        };
        let line = String::from_utf8_lossy(&buf[..n]).trim().to_string();

        if let Some(rest) = line.strip_prefix(PROBE_PREFIX) {
            if let Some(ack_rest) = rest.strip_prefix("-ACK") {
                // "HOLEPUNCH-ACK <session_id>" → 标记会话连通
                if let Ok(sid) = ack_rest.trim().parse::<u64>() {
                    mark_connected(&node, sid).await;
                }
            } else {
                // "HOLEPUNCH <session_id> <user_id> <my_mapped>" → 回 ACK
                // 探测包携带对端最新映射地址, 动态更新我方持有的对端映射
                if let Some((sid, _uid, peer_addr)) = parse_probe(rest) {
                    if let Some(addr) = peer_addr {
                        update_peer_mapped(&node, sid, addr).await;
                    }
                    let ack = format!("{} {}", ACK_PREFIX, sid);
                    if let Err(e) = sock.send_to(ack.as_bytes(), src).await {
                        eprintln!("[!] 回发ACK失败: {}", e);
                    } else {
                        println!("[*] 收到HOLEPUNCH sid={} from {}, 回ACK", sid, src);
                    }
                }
            }
        }
    }
}

/// 收到 HOLEPUNCH-ACK：将匹配会话标记为 Connected 并广播结果事件
async fn mark_connected(node: &Mutex<P2PNode>, sid: u64) {
    let n = node.lock().await;
    let mut sessions = n.hole_sessions.lock().await;
    for sess in sessions.values_mut() {
        if sess.session_id == sid && sess.state == SessionState::Punching {
            sess.state = SessionState::Connected;
            println!("[+] 打洞成功! 会话={} 对方映射={}", sid, sess.peer_mapped);
            let event = BotEvent::HolePunchResult {
                user_id: sess.peer_user_id,
                success: true,
                peer_mapped: Some(sess.peer_mapped),
                detail: format!(
                    "🎉 UDP打洞成功!\n📍 对方映射地址: {}\n📍 我方映射地址: {}\n🔗 UDP直连已建立",
                    sess.peer_mapped, sess.my_mapped
                ),
            };
            n.send_event(event).await;
            return;
        }
    }
    println!("[*] 收到ACK sid={}, 无匹配会话", sid);
}

/// 解析探测包剩余部分 " <session_id> <user_id> [<my_mapped>]"
/// 返回 (session_id, user_id, 对端携带的最新映射地址[可选])
fn parse_probe(rest: &str) -> Option<(u64, u64, Option<SocketAddr>)> {
    let parts: Vec<&str> = rest.split_whitespace().collect();
    let sid = parts.first()?.parse::<u64>().ok()?;
    let uid = parts.get(1)?.parse::<u64>().ok()?;
    let addr = parts.get(2).and_then(|s| s.parse::<SocketAddr>().ok());
    Some((sid, uid, addr))
}

/// 收到对方探测包携带的最新映射地址时, 更新匹配会话的对端映射。
/// 仅更新正在打洞(Punching)的会话, 且地址确实变化时才打印。
async fn update_peer_mapped(node: &Mutex<P2PNode>, sid: u64, new_addr: SocketAddr) {
    let n = node.lock().await;
    let mut sessions = n.hole_sessions.lock().await;
    for sess in sessions.values_mut() {
        if sess.session_id == sid
            && sess.state == SessionState::Punching
            && sess.peer_mapped != new_addr
        {
            println!(
                "[*] 会话 {} 对端映射更新: {} -> {}",
                sid, sess.peer_mapped, new_addr
            );
            sess.peer_mapped = new_addr;
        }
    }
}

/// 后台保活循环：周期性向 STUN 发送 Binding Request，维持 NAT 映射不超时。
/// 出站包本身即可重置 NAT 映射计时器；若响应被 UDP 监听循环抢收则静默跳过。
/// 每轮查询成功时刷新 `node.my_mapped` 缓存并打印地址是否变化。
pub async fn keepalive_loop(node: Arc<Mutex<P2PNode>>, interval: Duration) {
    if interval.is_zero() {
        return;
    }
    loop {
        tokio::time::sleep(interval).await;
        let (sock, stun_server) = {
            let n = node.lock().await;
            (n.udp_sock.clone(), n.stun_server)
        };
        let (Some(sock), Some(stun_server)) = (sock, stun_server) else {
            return;
        };
        match query_mapped_addr(&sock, stun_server).await {
            Ok(m) => {
                let changed = {
                    let mut n = node.lock().await;
                    let prev = n.my_mapped;
                    n.my_mapped = Some(m);
                    prev != Some(m)
                };
                if changed {
                    println!("[+] 保活: 映射地址已变化 -> {}", m);
                } else {
                    println!("[*] 保活: 映射地址不变 ({})", m);
                }
            }
            Err(_) => {
                // 静默: 出站 Binding Request 已达成保活目的, 响应可能被监听循环收走
            }
        }
    }
}

/// 启动一次 UDP 打洞会话（TCP 直连失败时自动触发）。
/// 单轮打洞: 一轮未连通即判定失败并发结果事件(QQ 信令模式)。
pub async fn start_hole_punch(
    node: Arc<Mutex<P2PNode>>,
    peer_user_id: u64,
    peer_mapped: SocketAddr,
) {
    start_hole_punch_retry(node, peer_user_id, peer_mapped, 1).await;
}

/// 持续打洞直到连通或达到重试轮数上限。
/// - `max_rounds = 0` 表示无限重试直到连通（命令行 `holepunch` 使用）
/// - 每轮: 刷新 STUN 映射 → 更新会话 → 发包约 10 秒; 未连通 → 等 `RETRY_INTERVAL` 后下一轮
/// - 防重入: 同一对端已有 Punching/Connected 会话时直接跳过
pub async fn start_hole_punch_retry(
    node: Arc<Mutex<P2PNode>>,
    peer_user_id: u64,
    peer_mapped: SocketAddr,
    max_rounds: u32,
) {
    // 防重入
    {
        let n = node.lock().await;
        let sessions = n.hole_sessions.lock().await;
        if let Some(sess) = sessions.get(&peer_user_id) {
            if matches!(sess.state, SessionState::Punching | SessionState::Connected) {
                println!(
                    "[*] 对端 {} 已有打洞会话(sid={}), 跳过重复启动",
                    peer_user_id, sess.session_id
                );
                return;
            }
        }
    }

    // 取共享 socket / STUN 服务器 / 本机 QQ 号
    let (sock, stun_server, my_uid) = {
        let n = node.lock().await;
        (n.udp_sock.clone(), n.stun_server, n.user_id)
    };
    let (Some(sock), Some(stun_server), _my_uid) = (sock, stun_server, my_uid) else {
        println!("[!] UDP 打洞未启用(STUN socket 未初始化), 跳过打洞");
        return;
    };

    // 生成会话 ID 并记录会话(映射占位, 每轮刷新后更新)
    let session_id = rand::random::<u64>();
    {
        let n = node.lock().await;
        let mut sessions = n.hole_sessions.lock().await;
        sessions.insert(
            peer_user_id,
            HolePunchSession {
                session_id,
                peer_user_id,
                my_mapped: peer_mapped,
                peer_mapped,
                state: SessionState::Punching,
            },
        );
    }

    let mut round: u32 = 1;
    loop {
        // 每轮先刷新本机映射(同一 socket, 映射一致性; 失败降级缓存)
        let my_mapped = match query_mapped_addr_retry(&sock, stun_server).await {
            Ok(m) => m,
            Err(e) => {
                let cached = {
                    let n = node.lock().await;
                    n.my_mapped
                };
                match cached {
                    Some(m) => {
                        eprintln!("[!] 第{}轮 STUN 刷新失败({}), 降级使用缓存映射 {}", round, e, m);
                        m
                    }
                    None => {
                        eprintln!("[!] 第{}轮 STUN 查询失败, 打洞终止: {}", round, e);
                        send_result_event(
                            &node,
                            peer_user_id,
                            false,
                            Some(peer_mapped),
                            format!("❌ UDP打洞失败: STUN 查询失败 ({})", e),
                        )
                        .await;
                        return;
                    }
                }
            }
        };
        // 刷新缓存 + 更新会话(我方/对端映射, 状态复位)
        {
            let mut n = node.lock().await;
            n.my_mapped = Some(my_mapped);
            let mut sessions = n.hole_sessions.lock().await;
            if let Some(sess) = sessions.get_mut(&peer_user_id) {
                sess.my_mapped = my_mapped;
                sess.peer_mapped = peer_mapped;
                sess.state = SessionState::Punching;
            }
        }
        println!(
            "[+] 第{}轮打洞: 对端={} 我方映射={} 对方映射={} 会话={}",
            round, peer_user_id, my_mapped, peer_mapped, session_id
        );

        // 发包约10秒, 期间对端ACK到达(监听循环标记 Connected)则提前成功
        let node2 = Arc::clone(&node);
        let connected = probe_loop(node2, peer_user_id, session_id, my_uid).await;
        if connected {
            println!("[+] 第{}轮打洞成功! 会话={}", round, session_id);
            return;
        }

        // 未连通: 达到轮数上限 → 判定失败并发事件
        if max_rounds > 0 && round >= max_rounds {
            {
                let n = node.lock().await;
                let mut sessions = n.hole_sessions.lock().await;
                if let Some(sess) = sessions.get_mut(&peer_user_id) {
                    sess.state = SessionState::Failed;
                }
            }
            println!(
                "[!] 打洞失败(已尝试{}轮): 对端={} 映射={}",
                round, peer_user_id, peer_mapped
            );
            let detail = format!(
                "❌ UDP打洞失败(已尝试{}轮): 可能为对称NAT或UDP被拦截\n📍 对方映射: {}\n💡 可回退TCP直连或后续中继兜底",
                round, peer_mapped
            );
            send_result_event(&node, peer_user_id, false, Some(peer_mapped), detail).await;
            return;
        }

        // 进入下一轮前等待
        println!(
            "[*] 第{}轮未连通, {}秒后重试(上限{})...",
            round,
            RETRY_INTERVAL.as_secs(),
            if max_rounds == 0 {
                "∞".to_string()
            } else {
                max_rounds.to_string()
            }
        );
        tokio::time::sleep(RETRY_INTERVAL).await;
        round += 1;
    }
}

/// 打洞发包循环：每 200ms 向对方映射地址发探测包，最多 50 次（约 10 秒）。
/// 探测包携带本机最新映射地址(`HOLEPUNCH <sid> <uid> <my_mapped>`), 对端可动态更新;
/// 发送目标实时从会话读取(对端探测包可更新我方持有的对端映射, 地址变化后自动改打新地址)。
/// 期间收到 ACK（由监听循环标记 Connected）则提前退出。
/// 返回 true 表示已连通。
async fn probe_loop(
    node: Arc<Mutex<P2PNode>>,
    peer_user_id: u64,
    session_id: u64,
    my_uid: u64,
) -> bool {
    let sock = {
        let n = node.lock().await;
        n.udp_sock.clone()
    };
    let Some(sock) = sock else {
        return false;
    };

    for i in 0..MAX_PROBES {
        // 每次发包前读取: 会话状态 / 对端最新映射 / 本机最新映射
        let (connected, peer_mapped, my_mapped) = {
            let n = node.lock().await;
            let sessions = n.hole_sessions.lock().await;
            let sess = sessions.get(&peer_user_id);
            (
                matches!(sess.map(|s| &s.state), Some(SessionState::Connected)),
                sess.map(|s| s.peer_mapped),
                n.my_mapped,
            )
        };
        if connected {
            println!("[*] 会话 {} 已连通, 停止发包", session_id);
            return true;
        }
        let Some(peer_mapped) = peer_mapped else {
            tokio::time::sleep(PROBE_INTERVAL).await;
            continue;
        };
        // 携带本机最新映射(保活/上轮刷新后可能变化), 供对端动态更新
        let probe = match my_mapped {
            Some(m) => format!("{} {} {} {}", PROBE_PREFIX, session_id, my_uid, m),
            None => format!("{} {} {}", PROBE_PREFIX, session_id, my_uid),
        };
        if let Err(e) = sock.send_to(probe.as_bytes(), peer_mapped).await {
            eprintln!("[!] 发送探测包失败({}/{}): {}", i + 1, MAX_PROBES, e);
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }

    false
}

/// 发送打洞结果事件（由消息处理层回 QQ）
async fn send_result_event(
    node: &Mutex<P2PNode>,
    user_id: u64,
    success: bool,
    peer_mapped: Option<SocketAddr>,
    detail: String,
) {
    let n = node.lock().await;
    n.send_event(BotEvent::HolePunchResult {
        user_id,
        success,
        peer_mapped,
        detail,
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_response(attr_type: u16, attr_value: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&STUN_MSG_BINDING_RESPONSE.to_be_bytes());
        // STUN 头 Length 字段 = 所有属性总长（含 4 字节属性头）
        buf.extend_from_slice(&((attr_value.len() + 4) as u16).to_be_bytes());
        buf.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&[0u8; 12]); // tx_id
        buf.extend_from_slice(&attr_type.to_be_bytes());
        buf.extend_from_slice(&(attr_value.len() as u16).to_be_bytes());
        buf.extend_from_slice(attr_value);
        buf
    }

    /// 构造 XOR-MAPPED-ADDRESS 属性值（8 字节）
    fn xor_mapped_value(ip: [u8; 4], port: u16) -> Vec<u8> {
        let mut v = Vec::with_capacity(8);
        v.push(0); // reserved
        v.push(0x01); // family = IPv4
        v.extend_from_slice(&(port ^ (STUN_MAGIC_COOKIE >> 16) as u16).to_be_bytes());
        v.push(ip[0] ^ (STUN_MAGIC_COOKIE >> 24) as u8);
        v.push(ip[1] ^ (STUN_MAGIC_COOKIE >> 16) as u8);
        v.push(ip[2] ^ (STUN_MAGIC_COOKIE >> 8) as u8);
        v.push(ip[3] ^ STUN_MAGIC_COOKIE as u8);
        v
    }

    #[test]
    fn test_parse_stun_xor_mapped_address() {
        let buf = build_test_response(ATTR_XOR_MAPPED_ADDRESS, &xor_mapped_value([1, 2, 3, 4], 54321));
        let addr = parse_stun_response(&buf).unwrap();
        assert_eq!(addr, SocketAddr::new("1.2.3.4".parse().unwrap(), 54321));
    }

    #[test]
    fn test_parse_stun_xor_mapped_address_0x2112() {
        // 端口 0x2112 ^ 0x2112 = 0，考验 XOR 混淆边界
        let buf = build_test_response(ATTR_XOR_MAPPED_ADDRESS, &xor_mapped_value([8, 8, 8, 8], 0x2112));
        let addr = parse_stun_response(&buf).unwrap();
        assert_eq!(addr, SocketAddr::new("8.8.8.8".parse().unwrap(), 0x2112));
    }

    #[test]
    fn test_parse_stun_mapped_address_compat() {
        // 旧版服务器：MAPPED-ADDRESS 不混淆
        let mut v = Vec::with_capacity(8);
        v.push(0);
        v.push(0x01);
        v.extend_from_slice(&8080u16.to_be_bytes());
        v.extend_from_slice(&[9, 9, 9, 9]);
        let buf = build_test_response(ATTR_MAPPED_ADDRESS, &v);
        let addr = parse_stun_response(&buf).unwrap();
        assert_eq!(addr, SocketAddr::new("9.9.9.9".parse().unwrap(), 8080));
    }

    #[test]
    fn test_parse_stun_response_invalid() {
        // 非 Binding Response（如错误响应 0x0111）
        let mut buf = vec![0x01, 0x11];
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&[0u8; 12]);
        assert!(parse_stun_response(&buf).is_err());
    }

    #[test]
    fn test_extract_holepunch_info() {
        let msg = "🌐 我的P2P节点信息:\n📍 公网IP: 54.251.93.8\n🔌 端口: 8080\n📡 P2P打洞: udp://9.8.7.6:54321 会话=123456789";
        let (addr, sid) = extract_holepunch_info(msg).unwrap();
        assert_eq!(addr, SocketAddr::new("9.8.7.6".parse().unwrap(), 54321));
        assert_eq!(sid, 123456789);
    }

    #[test]
    fn test_extract_holepunch_info_no_udp_line() {
        let msg = "🌐 我的P2P节点信息:\n📍 公网IP: 54.251.93.8\n🔌 端口: 8080";
        assert!(extract_holepunch_info(msg).is_none());
    }

    #[test]
    fn test_extract_holepunch_info_invalid() {
        // 打洞行存在但端口非法
        let msg = "📡 P2P打洞: udp://9.8.7.6:abc 会话=1";
        assert!(extract_holepunch_info(msg).is_none());
    }

    #[test]
    fn test_parse_probe_with_addr() {
        let (sid, uid, addr) = parse_probe(" 123456 10001 1.2.3.4:54321").unwrap();
        assert_eq!(sid, 123456);
        assert_eq!(uid, 10001);
        assert_eq!(addr, Some(SocketAddr::new("1.2.3.4".parse().unwrap(), 54321)));
    }

    #[test]
    fn test_parse_probe_without_addr() {
        let (sid, uid, addr) = parse_probe(" 123456 10001").unwrap();
        assert_eq!(sid, 123456);
        assert_eq!(uid, 10001);
        assert_eq!(addr, None);
    }

    #[test]
    fn test_parse_probe_invalid() {
        assert!(parse_probe(" abc 10001").is_none());
        assert!(parse_probe(" 123456").is_none());
    }
}
