//! P2P 节点：节点信息交换、TCP 直连服务、连接管理
//!
//! 预留：NAT 打洞（STUN + UDP Hole Punching）将在后续版本加入本模块，
//! 方案见项目根目录 `P2P_HOLE_PUNCHING.md`（M1 计划）。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde::{Deserialize, Serialize};
use reqwest::Client;
use anyhow::{Result, Context};

/// 握手确认回复: 收到与已记录完全相同的节点信息(ip:port)时回复此内容。
/// 该内容不含节点信息关键词, 对方收到后不会再次触发握手, 从而终止无限互发。
pub const HANDSHAKE_CONFIRM_REPLY: &str = "✅ 握手已确认，节点已记录，无需重复发送。";

/// 对端节点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub user_id: u64,
    pub ip: String,
    pub port: u16,
}

/// 机器人内部事件：由 P2P 节点 / WebSocket 监听层产生，消息处理层消费
#[derive(Debug, Clone)]
pub enum BotEvent {
    PrivateMessage { user_id: u64, message: String },
    GroupMessage { user_id: u64, group_id: u64, message: String, #[allow(dead_code)] raw_message: String },
    Connected { user_id: u64 },
    Disconnected { user_id: u64 },
    /// UDP 打洞结果：会话结束（成功/失败）时发出，由消息处理层回 QQ
    HolePunchResult {
        user_id: u64,
        success: bool,
        #[allow(dead_code)]
        peer_mapped: Option<SocketAddr>,
        detail: String,
    },
}

/// 打洞会话状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// 正在发包打洞
    Punching,
    /// 已连通
    Connected,
    /// 失败（超时 / STUN 错误）
    Failed,
}

/// 一次 UDP 打洞会话
#[derive(Debug, Clone)]
pub struct HolePunchSession {
    /// 本机会话 ID（探测包携带，ACK 据此对号入座）
    pub session_id: u64,
    /// 对端 QQ 号
    pub peer_user_id: u64,
    /// 本机 NAT 映射地址
    pub my_mapped: SocketAddr,
    /// 对方 NAT 映射地址
    pub peer_mapped: SocketAddr,
    pub state: SessionState,
}

/// P2P 节点：维护本机节点信息、对端 Peer 表与连接状态
#[derive(Debug)]
pub struct P2PNode {
    /// 本机 QQ 号（预留：打洞会话校验对端身份时使用）
    #[allow(dead_code)]
    pub user_id: u64,
    pub ip: String,
    pub port: u16,
    pub peers: Arc<Mutex<HashMap<u64, PeerInfo>>>,
    pub peer_ips: Arc<Mutex<HashMap<u64, String>>>,
    /// UDP 打洞 socket（STUN 查询与打洞发包共用，保证映射一致性）
    pub udp_sock: Option<Arc<UdpSocket>>,
    /// UDP 打洞监听端口
    pub udp_port: u16,
    /// STUN 服务器地址
    pub stun_server: Option<SocketAddr>,
    /// 本机 NAT 映射地址缓存（STUN 查询结果，供节点信息附带打洞行）
    pub my_mapped: Option<SocketAddr>,
    /// 打洞会话表（key = 对端 QQ 号）
    pub hole_sessions: Arc<Mutex<HashMap<u64, HolePunchSession>>>,
    event_tx: Arc<Mutex<broadcast::Sender<BotEvent>>>,
}

impl P2PNode {
    pub async fn new(user_id: u64) -> Result<(Self, broadcast::Receiver<BotEvent>)> {
        let ip = Self::get_public_ip().await?;
        Ok(Self::new_with_ip(user_id, ip))
    }

    /// 构造节点但不查询公网 IP（供 `holepunch` 等仅需 UDP 打洞、不依赖外网 IP 的命令使用）
    pub async fn new_offline(user_id: u64) -> Result<(Self, broadcast::Receiver<BotEvent>)> {
        Ok(Self::new_with_ip(user_id, "0.0.0.0".to_string()))
    }

    fn new_with_ip(user_id: u64, ip: String) -> (Self, broadcast::Receiver<BotEvent>) {
        let (event_tx, event_rx) = broadcast::channel(100);

        (Self {
            user_id,
            ip,
            port: 0,
            peers: Arc::new(Mutex::new(HashMap::new())),
            peer_ips: Arc::new(Mutex::new(HashMap::new())),
            udp_sock: None,
            udp_port: 0,
            stun_server: None,
            my_mapped: None,
            hole_sessions: Arc::new(Mutex::new(HashMap::new())),
            event_tx: Arc::new(Mutex::new(event_tx)),
        }, event_rx)
    }

    pub async fn get_public_ip() -> Result<String> {
        let client = Client::new();

        let ip = client.get("https://api.ipify.org?format=text")
            .send()
            .await?
            .text()
            .await?;

        Ok(ip.trim().to_string())
    }

    pub async fn get_ip_info(&self) -> String {
        // 打洞映射行：仅当 UDP 打洞已启用且 STUN 查询成功时附带（会话 ID 用于日志追踪）
        match self.my_mapped {
            Some(m) => format!(
                "🌐 我的P2P节点信息:\n📍 公网IP: {}\n🔌 端口: {}\n📡 P2P打洞: udp://{} 会话={}",
                self.ip,
                self.port,
                m,
                rand::random::<u64>()
            ),
            None => format!("🌐 我的P2P节点信息:\n📍 公网IP: {}\n🔌 端口: {}", self.ip, self.port),
        }
    }

    pub async fn send_event(&self, event: BotEvent) {
        let tx = self.event_tx.lock().await;
        let _ = tx.send(event);
    }

    /// 预留 API：P2P 连接管理（打洞成功 / 断开时由后续功能调用）
    #[allow(dead_code)]
    pub async fn add_peer(&self, peer: PeerInfo) {
        let mut peers = self.peers.lock().await;
        peers.insert(peer.user_id, peer.clone());
        println!("[+] 新增Peer: {:?}", peer);
        let _ = self.send_event(BotEvent::Connected { user_id: peer.user_id }).await;
    }

    #[allow(dead_code)]
    pub async fn remove_peer(&self, user_id: u64) {
        let mut peers = self.peers.lock().await;
        peers.remove(&user_id);
        println!("[*] 移除Peer: {}", user_id);
        let _ = self.send_event(BotEvent::Disconnected { user_id }).await;
    }

    #[allow(dead_code)]
    pub async fn get_peer_info(&self, user_id: u64) -> Option<String> {
        let peers = self.peers.lock().await;
        peers.get(&user_id).map(|p| format!("{}:{}", p.ip, p.port))
    }

    pub async fn start_tcp_server(node: Arc<Mutex<P2PNode>>, port: u16) -> Result<()> {
        let addr = format!("0.0.0.0:{}", port);
        let listener = TcpListener::bind(&addr)
            .await
            .context("绑定TCP端口失败")?;

        // 短暂加锁设置端口，然后立即释放（避免长期持有锁导致死锁）
        {
            let mut n = node.lock().await;
            n.port = port;
        }
        println!("[*] P2P服务监听在 0.0.0.0:{}", port);

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    println!("[*] 新连接: {}", addr);
                    // 仅在需要时短暂加锁读取IP，避免长期持有锁
                    let ip = {
                        let n = node.lock().await;
                        n.ip.clone()
                    };
                    Self::handle_connection(&ip, stream).await;
                }
                Err(e) => {
                    eprintln!("[!] 连接错误: {}", e);
                }
            }
        }
    }

    /// 启动 UDP 打洞服务：绑定 UDP socket、查询 STUN 缓存映射地址、监听打洞报文。
    /// socket 全程共享，STUN 查询与打洞发包使用同一 socket（映射一致性）。
    pub async fn start_udp_server(node: Arc<Mutex<P2PNode>>, port: u16, stun_server: SocketAddr) -> Result<()> {
        let sock = Arc::new(
            UdpSocket::bind(("0.0.0.0", port))
                .await
                .context("绑定UDP端口失败")?,
        );
        println!("[*] UDP打洞监听在 0.0.0.0:{} (STUN: {})", port, stun_server);

        {
            let mut n = node.lock().await;
            n.udp_sock = Some(Arc::clone(&sock));
            n.udp_port = port;
            n.stun_server = Some(stun_server);
        }

        // 启动时查询一次 STUN(带重试), 缓存本机映射(供 get_ip_info 附带打洞行)
        match crate::holepunch::query_mapped_addr_retry(&sock, stun_server).await {
            Ok(m) => {
                println!("[+] 本机STUN映射地址: {}", m);
                let mut n = node.lock().await;
                n.my_mapped = Some(m);
            }
            Err(e) => {
                eprintln!("[!] STUN 查询失败(节点信息将不附带打洞行): {}", e);
            }
        }

        // UDP 监听循环
        let node2 = Arc::clone(&node);
        tokio::spawn(async move {
            if let Err(e) = crate::holepunch::run_udp_listener(node2).await {
                eprintln!("[!] UDP监听循环退出: {}", e);
            }
        });

        Ok(())
    }

    async fn handle_connection(ip: &str, mut stream: TcpStream) {
        let mut buf = [0; 4096];
        match stream.read(&mut buf).await {
            Ok(n) if n > 0 => {
                let msg = String::from_utf8_lossy(&buf[..n]);
                println!("[*] 收到数据: {}", msg.trim());

                if msg.starts_with("PING ") {
                    let reply = format!("PONG {}", ip);
                    if let Err(e) = stream.write_all(reply.as_bytes()).await {
                        eprintln!("[!] 连接错误: {}", e);
                    }
                }
            }
            Ok(_) => {
                println!("[*] 连接关闭");
            }
            Err(e) => {
                eprintln!("[!] 读取错误: {}", e);
            }
        }
    }

    pub async fn connect_to_peer(&self, ip: &str, port: u16) -> Result<String> {
        println!("[*] 尝试连接 {}:{}", ip, port);

        match TcpStream::connect((ip, port)).await {
            Ok(mut stream) => {
                println!("[+] 连接成功!");

                let msg = format!("PING {}", self.ip);
                stream.write_all(msg.as_bytes()).await?;

                let mut buf = [0; 4096];
                if let Ok(n) = stream.read(&mut buf).await {
                    let reply = String::from_utf8_lossy(&buf[..n]);
                    println!("[*] 收到响应: {}", reply.trim());

                    if let Some(peer_ip) = reply.strip_prefix("PONG ") {
                        return Ok(format!("🟢 P2P连接成功!\n📍 对方IP: {}\n🔗 连接已建立", peer_ip));
                    }
                }

                Ok("🟢 连接成功".to_string())
            }
            Err(e) => {
                eprintln!("[!] 连接失败: {}", e);
                Err(e).context("无法连接到对端")
            }
        }
    }

    pub async fn parse_and_store_peer_ip(&self, user_id: u64, message: &str) -> Option<String> {
        let normalized = normalize_node_message(message);
        println!("[DBG] parse_and_store_peer_ip: from={} normalized={:?}", user_id, normalized);

        let mut found: Option<(String, u16)> = None;

        // 1) 前缀格式 "IP: x.x.x.x" + "端口: xxxx" (同款机器人互发的节点信息)
        //    extract_ip_port 在整段文本上定位, 不依赖换行, 兼容真实换行/字面"\n"/单行挤压
        if found.is_none() {
            found = extract_ip_port(&normalized);
        }

        // 2) 连续格式 "a.b.c.d:port"
        if found.is_none() {
            for part in normalized.split_whitespace() {
                let segs: Vec<&str> = part.split(':').collect();
                if segs.len() == 2 && is_valid_ipv4(segs[0]) {
                    if let Ok(port) = segs[1].parse::<u16>() {
                        found = Some((segs[0].to_string(), port));
                        break;
                    }
                }
            }
        }

        // 3) /connect IP:PORT 格式
        if found.is_none() {
            if let Some(idx) = normalized.find("/connect") {
                let rest = normalized[idx + "/connect".len()..].trim();
                if let Some(first) = rest.split_whitespace().next() {
                    let segs: Vec<&str> = first.split(':').collect();
                    if segs.len() == 2 && is_valid_ipv4(segs[0]) {
                        if let Ok(port) = segs[1].parse::<u16>() {
                            found = Some((segs[0].to_string(), port));
                        }
                    }
                }
            }
        }

        if let Some((ip, port)) = found {
            let new_key = format!("{}:{}", ip, port);

            // 防无限循环: 已记录过完全相同的节点(ip:port) → 视为握手已完成
            // 回复内容"稍微不同"的确认消息(不含节点信息), 对方收到后不会再触发握手
            let mut peer_ips = self.peer_ips.lock().await;
            if peer_ips.get(&user_id) == Some(&new_key) {
                println!("[*] 收到重复节点信息({}), 握手已完成, 回复确认(防循环)", new_key);
                return Some(HANDSHAKE_CONFIRM_REPLY.to_string());
            }

            peer_ips.insert(user_id, new_key);
            println!("[+] 记录对方节点: {} -> {}:{}", user_id, ip, port);

            return Some(format!("🟢 已记录对方节点: {}:{}\n🔄 正在尝试连接...", ip, port));
        }

        println!("[!] 无法从消息中解析出IP和端口: from={} msg={:?}", user_id, message);
        None
    }

    /// 尝试自动连接对端（TCP）。返回是否连接成功，供调用方决定是否转 UDP 打洞。
    pub async fn try_auto_connect(&self, user_id: u64) -> bool {
        let peer_ips = self.peer_ips.lock().await;
        if let Some(peer_ip_port) = peer_ips.get(&user_id) {
            let parts: Vec<&str> = peer_ip_port.split(':').collect();
            if parts.len() == 2 {
                let ip = parts[0].to_string();
                if let Ok(port) = parts[1].parse::<u16>() {
                    drop(peer_ips);

                    match self.connect_to_peer(&ip, port).await {
                        Ok(_result) => {
                            println!("[+] 自动连接对方: {}:{}", ip, port);
                            let mut peers = self.peers.lock().await;
                            peers.insert(user_id, PeerInfo {
                                user_id,
                                ip: ip.clone(),
                                port,
                            });
                            return true;
                        }
                        Err(e) => {
                            println!("[!] 自动连接失败: {}", e);
                            return false;
                        }
                    }
                }
            }
        }
        false
    }
}

/// 规范化节点信息消息: 全角冒号→半角, "公网IP"/"公网ip"→"IP", 字面"\n"→真实换行
fn normalize_node_message(message: &str) -> String {
    message
        .replace('：', ":")
        .replace("\\n", "\n")
        .replace("公网IP", "IP")
        .replace("公网ip", "IP")
}

/// 简单校验 IPv4 地址: 以数字开头, 仅含数字和点
fn is_valid_ipv4(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// 在整段文本上定位 "IP:" 与 "端口:", 提取其后的连续数字(和点)。
/// 不依赖换行: 真实换行 / 字面 "\n" / 单行挤压 均可解析。
/// 端口来源: 优先 "端口: xxxx", 其次 "IP: x.x.x.x:port" 中 IP 后紧跟的 ":port"。
fn extract_ip_port(message: &str) -> Option<(String, u16)> {
    let normalized = normalize_node_message(message);

    // 提取 "IP:" 后的 IPv4, 并记录 IP 之后的剩余文本(用于 "IP: x.x.x.x:port" 兜底)
    let (ip, after_ip) = normalized.find("IP:").and_then(|idx| {
        let rest = normalized[idx + "IP:".len()..].trim_start();
        let candidate: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if is_valid_ipv4(&candidate) {
            // candidate 全 ASCII, len() 即字节数; rest 以 candidate 开头
            Some((candidate.clone(), &rest[candidate.len()..]))
        } else {
            None
        }
    })?;

    // 1) "端口: xxxx" —— 注意前缀是中文("端口:"=10字节), 不能用 idx+3
    let port = normalized.find("端口:").and_then(|idx| {
        let digits: String = normalized[idx + "端口:".len()..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<u16>().ok()
    });

    // 2) IP 后紧跟 ":port" (如 "IP: 1.2.3.4:9000")
    let port = port.or_else(|| {
        let rest = after_ip.trim_start().strip_prefix(':')?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u16>().ok()
    });

    port.map(|p| (ip, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 复现实际日志中的消息(字面 "\n", 被 JSON 转义后到达程序)
    #[test]
    fn test_extract_from_real_log_literal_newline() {
        let msg = "🌐 我的P2P节点信息:\\n📍 公网IP: 54.251.93.8\\n🔌 端口: 8080";
        let normalized = normalize_node_message(msg);
        eprintln!("[DBG-T] normalized={:?}", normalized);
        eprintln!("[DBG-T] find(IP:)={:?} find(端口:)={:?}", normalized.find("IP:"), normalized.find("端口:"));
        let got = extract_ip_port(msg);
        assert_eq!(got, Some(("54.251.93.8".to_string(), 8080)));
    }

    /// 真实换行版本(对方程序直接拼接 \n)
    #[test]
    fn test_extract_from_real_log_real_newline() {
        let msg = "🌐 我的P2P节点信息:\n📍 公网IP: 54.251.93.8\n🔌 端口: 8080";
        let got = extract_ip_port(msg);
        assert_eq!(got, Some(("54.251.93.8".to_string(), 8080)));
    }

    /// 单行挤压格式
    #[test]
    fn test_extract_single_line() {
        let msg = "我的P2P节点信息: 公网IP: 1.2.3.4 端口: 9000 请把你的信息告诉我";
        let got = extract_ip_port(msg);
        assert_eq!(got, Some(("1.2.3.4".to_string(), 9000)));
    }

    /// 连续格式 "IP:PORT"
    #[test]
    fn test_extract_compact() {
        let msg = "IP: 1.2.3.4:9000";
        let got = extract_ip_port(msg);
        assert_eq!(got, Some(("1.2.3.4".to_string(), 9000)));
    }

    /// 缺少端口时应返回 None
    #[test]
    fn test_extract_missing_port() {
        let msg = "IP: 1.2.3.4 没有端口";
        let got = extract_ip_port(msg);
        assert_eq!(got, None);
    }

    /// 防无限循环: 相同节点信息(ip:port)重复收到时, 回复内容"不同"的确认消息,
    /// 不再触发对方继续回复; 端口变化(如重启后端口改变)仍允许重新握手
    #[tokio::test]
    async fn test_duplicate_peer_info_skips_reply() {
        let (event_tx, _) = broadcast::channel(10);
        let node = P2PNode {
            user_id: 1,
            ip: "1.2.3.4".to_string(),
            port: 9000,
            peers: Arc::new(Mutex::new(HashMap::new())),
            peer_ips: Arc::new(Mutex::new(HashMap::new())),
            udp_sock: None,
            udp_port: 0,
            stun_server: None,
            my_mapped: None,
            hole_sessions: Arc::new(Mutex::new(HashMap::new())),
            event_tx: Arc::new(Mutex::new(event_tx)),
        };
        let msg = "🌐 我的P2P节点信息:\n📍 公网IP: 8.8.8.8\n🔌 端口: 8080";

        // 第一次: 新节点, 记录并回复节点信息(非确认消息)
        let first = node.parse_and_store_peer_ip(100, msg).await;
        assert!(first.is_some());
        assert_ne!(first.as_deref(), Some(HANDSHAKE_CONFIRM_REPLY));

        // 第二次相同内容: 视为重复 → 回复确认消息(内容不同), 防无限互发
        let second = node.parse_and_store_peer_ip(100, msg).await;
        assert_eq!(second.as_deref(), Some(HANDSHAKE_CONFIRM_REPLY));

        // 端口变化: 视为新节点, 允许重新握手
        let msg2 = "🌐 我的P2P节点信息:\n📍 公网IP: 8.8.8.8\n🔌 端口: 8081\n\n请把你的信息告诉我";
        let third = node.parse_and_store_peer_ip(100, msg2).await;
        assert!(third.is_some());
        assert_ne!(third.as_deref(), Some(HANDSHAKE_CONFIRM_REPLY));
    }
}
