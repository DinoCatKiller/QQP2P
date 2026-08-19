use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde::{Deserialize, Serialize};
use reqwest::Client;
use anyhow::{Result, Context};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub user_id: u64,
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BotState {
    Idle,
    WaitingForPeerIP,
    WaitingForOwnIP,
    Connecting,
    Connected,
}

#[derive(Debug, Clone)]
pub enum BotEvent {
    PrivateMessage { user_id: u64, message: String },
    GroupMessage { user_id: u64, group_id: u64, message: String, raw_message: String },
    Connected { user_id: u64 },
    Disconnected { user_id: u64 },
}

#[derive(Debug)]
pub struct P2PNode {
    pub user_id: u64,
    pub ip: String,
    pub port: u16,
    pub peers: Arc<Mutex<HashMap<u64, PeerInfo>>>,
    pub states: Arc<Mutex<HashMap<u64, BotState>>>,
    pub peer_ips: Arc<Mutex<HashMap<u64, String>>>,
    event_tx: Arc<Mutex<broadcast::Sender<BotEvent>>>,
}

impl P2PNode {
    pub async fn new(user_id: u64) -> Result<(Self, broadcast::Receiver<BotEvent>)> {
        let ip = Self::get_public_ip().await?;
        let (event_tx, event_rx) = broadcast::channel(100);
        
        Ok((Self {
            user_id,
            ip: ip.clone(),
            port: 0,
            peers: Arc::new(Mutex::new(HashMap::new())),
            states: Arc::new(Mutex::new(HashMap::new())),
            peer_ips: Arc::new(Mutex::new(HashMap::new())),
            event_tx: Arc::new(Mutex::new(event_tx)),
        }, event_rx))
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
        format!("🌐 我的P2P节点信息:\n📍 公网IP: {}\n🔌 端口: {}\n\n请把你的信息告诉我",
            self.ip, self.port)
    }

    pub async fn send_event(&self, event: BotEvent) {
        let tx = self.event_tx.lock().await;
        let _ = tx.send(event);
    }

    pub async fn add_peer(&self, peer: PeerInfo) {
        let mut peers = self.peers.lock().await;
        peers.insert(peer.user_id, peer.clone());
        println!("[+] 新增Peer: {:?}", peer);
        let _ = self.send_event(BotEvent::Connected { user_id: peer.user_id }).await;
    }

    pub async fn remove_peer(&self, user_id: u64) {
        let mut peers = self.peers.lock().await;
        peers.remove(&user_id);
        println!("[*] 移除Peer: {}", user_id);
        let _ = self.send_event(BotEvent::Disconnected { user_id }).await;
    }

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
                    
                    if reply.starts_with("PONG ") {
                        let peer_ip = &reply[5..];
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
        // 统一规范化: 全角冒号→半角, "公网IP"→"IP"(兼容大小写)
        let normalized = message
            .replace('：', ":")
            .replace("公网IP", "IP")
            .replace("公网ip", "IP");

        let mut found_ip: Option<String> = None;
        let mut found_port: Option<u16> = None;

        // 1) 连续格式 "a.b.c.d:port"
        for part in normalized.split_whitespace() {
            let segs: Vec<&str> = part.split(':').collect();
            if segs.len() == 2 && Self::is_valid_ipv4(segs[0]) {
                if let Ok(port) = segs[1].parse::<u16>() {
                    found_ip = Some(segs[0].to_string());
                    found_port = Some(port);
                }
            }
        }

        // 2) 分行格式 "IP: x.x.x.x" + "端口: xxxx" (同款机器人互发的节点信息)
        if found_ip.is_none() || found_port.is_none() {
            let mut ip: Option<String> = None;
            let mut port: Option<u16> = None;
            for line in normalized.lines() {
                let line = line.trim();
                if let Some(idx) = line.find("IP:") {
                    let candidate = line[idx + 3..].trim();
                    if Self::is_valid_ipv4(candidate) {
                        ip = Some(candidate.to_string());
                    }
                } else if let Some(idx) = line.find("端口:") {
                    if let Ok(p) = line[idx + 3..].trim().parse::<u16>() {
                        port = Some(p);
                    }
                }
            }
            if let (Some(i), Some(p)) = (ip, port) {
                found_ip = Some(i);
                found_port = Some(p);
            }
        }

        // 3) /connect IP:PORT 格式
        if found_ip.is_none() {
            if let Some(idx) = normalized.find("/connect") {
                let rest = normalized[idx + "/connect".len()..].trim();
                if let Some(first) = rest.split_whitespace().next() {
                    let segs: Vec<&str> = first.split(':').collect();
                    if segs.len() == 2 && Self::is_valid_ipv4(segs[0]) {
                        if let Ok(port) = segs[1].parse::<u16>() {
                            found_ip = Some(segs[0].to_string());
                            found_port = Some(port);
                        }
                    }
                }
            }
        }

        if let (Some(ip), Some(port)) = (found_ip, found_port) {
            let mut peer_ips = self.peer_ips.lock().await;
            peer_ips.insert(user_id, format!("{}:{}", ip, port));

            println!("[+] 记录对方节点: {} -> {}:{}", user_id, ip, port);

            return Some(format!("🟢 已记录对方节点: {}:{}\n🔄 正在尝试连接...", ip, port));
        }

        None
    }

    /// 简单校验 IPv4 地址: 以数字开头, 仅含数字和点
    fn is_valid_ipv4(s: &str) -> bool {
        !s.is_empty()
            && s.starts_with(|c: char| c.is_ascii_digit())
            && s.chars().all(|c| c.is_ascii_digit() || c == '.')
    }

    pub async fn try_auto_connect(&self, user_id: u64) {
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
                        }
                        Err(e) => {
                            println!("[!] 自动连接失败: {}", e);
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NapCatConfig {
    pub http_host: String,
    pub http_port: u16,
    pub ws_host: String,
    pub ws_port: u16,
    pub token: Option<String>,
}

impl Default for NapCatConfig {
    fn default() -> Self {
        Self {
            http_host: "127.0.0.1".to_string(),
            http_port: 3000,
            ws_host: "127.0.0.1".to_string(),
            ws_port: 3001,
            token: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct NapCatResponse<T> {
    pub status: String,
    pub retcode: i32,
    pub data: Option<T>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UserInfo {
    #[serde(rename = "user_id")]
    pub user_id: u64,
    pub nickname: String,
}

#[derive(Debug, Deserialize)]
pub struct FriendInfo {
    pub user_id: u64,
    pub nickname: String,
}

#[derive(Debug, Deserialize)]
pub struct GroupInfo {
    pub group_id: u64,
    pub group_name: String,
}

#[derive(Debug, Clone)]
pub struct NapCatClient {
    client: Client,
    pub config: NapCatConfig,
}

impl NapCatClient {
    pub fn new(config: NapCatConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    pub async fn new_default() -> Result<Self> {
        Ok(Self::new(NapCatConfig::default()))
    }

    fn base_url(&self) -> String {
        format!("http://{}:{}", self.config.http_host, self.config.http_port)
    }

    pub async fn get_friends(&self) -> Result<Vec<FriendInfo>> {
        let url = format!("{}/get_friend_list", self.base_url());
        let resp: NapCatResponse<Vec<FriendInfo>> = self.client.get(&url).send().await?
            .json().await?;
        
        if resp.retcode != 0 {
            anyhow::bail!("获取好友列表失败: {:?}", resp.message);
        }
        
        Ok(resp.data.unwrap_or_default())
    }

    pub async fn get_groups(&self) -> Result<Vec<GroupInfo>> {
        let url = format!("{}/get_group_list", self.base_url());
        let resp: NapCatResponse<Vec<GroupInfo>> = self.client.get(&url).send().await?
            .json().await?;
        
        if resp.retcode != 0 {
            anyhow::bail!("获取群列表失败: {:?}", resp.message);
        }
        
        Ok(resp.data.unwrap_or_default())
    }

    pub async fn send_private_message(&self, user_id: u64, message: &str) -> Result<i64> {
        let url = format!("{}/send_private_msg", self.base_url());
        let params = serde_json::json!({
            "user_id": user_id,
            "message": message
        });
        
        let resp: NapCatResponse<serde_json::Value> = self.client.post(&url)
            .json(&params)
            .send()
            .await?
            .json()
            .await?;
        
        if resp.retcode != 0 {
            anyhow::bail!("发送私聊消息失败: {:?}", resp.message);
        }
        
        Ok(0)
    }

    pub async fn send_group_message(&self, group_id: u64, message: &str) -> Result<i64> {
        let url = format!("{}/send_group_msg", self.base_url());
        let params = serde_json::json!({
            "group_id": group_id,
            "message": message
        });
        
        let resp: NapCatResponse<serde_json::Value> = self.client.post(&url)
            .json(&params)
            .send()
            .await?
            .json()
            .await?;
        
        if resp.retcode != 0 {
            anyhow::bail!("发送群消息失败: {:?}", resp.message);
        }
        
        Ok(0)
    }

    pub async fn check_online(&self) -> Result<bool> {
        let url = format!("{}/get_login_info", self.base_url());
        Ok(self.client.get(&url).send().await.is_ok())
    }

    pub async fn get_login_info(&self) -> Result<UserInfo> {
        let url = format!("{}/get_login_info", self.base_url());
        let resp: NapCatResponse<UserInfo> = self.client.get(&url).send().await?.json().await?;

        if resp.retcode != 0 {
            anyhow::bail!("获取登录信息失败: {:?}", resp.message);
        }

        resp.data.context("登录信息为空")
    }
}
