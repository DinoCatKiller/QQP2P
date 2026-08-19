use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde::{Deserialize, Serialize};
use reqwest::Client;
use anyhow::{Result, Context};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use futures_util::{StreamExt, SinkExt};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub user_id: u64,
    pub ip: String,
    pub port: u16,
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
            event_tx: Arc::new(Mutex::new(event_tx)),
        }, event_rx))
    }

    pub async fn get_public_ip() -> Result<String> {
        let client = Client::new();
        
        let services = [
            "https://api.ipify.org?format=text",
            "https://ifconfig.me/ip",
            "https://icanhazip.com/",
            "https://ipinfo.io/ip",
            "https://checkip.amazonaws.com/",
        ];
        
        for service in &services {
            match client.get(*service).send().await {
                Ok(resp) => {
                    if let Ok(ip) = resp.text().await {
                        let ip = ip.trim().to_string();
                        if !ip.is_empty() && ip.chars().all(|c| c.is_ascii_digit() || c == '.') {
                            return Ok(ip);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[!] 获取公网IP失败 ({}) : {}", service, e);
                }
            }
        }
        
        anyhow::bail!("无法获取公网IP，所有服务均不可用")
    }

    pub async fn get_ip_info(&self) -> String {
        format!("🌐 P2P节点信息:\n📍 公网IP: {}\n🔌 端口: {}\n💡 发送此信息给对方，对方输入\"/连接 {} {}\"即可建立P2P",
            self.ip, self.port, self.ip, self.port)
    }

    pub async fn send_event(&self, event: BotEvent) {
        let tx = self.event_tx.lock().await;
        let _ = tx.send(event);
    }

    pub async fn add_peer(&self, peer: PeerInfo) {
        let mut peers = self.peers.lock().await;
        peers.insert(peer.user_id, peer.clone());
        println!("[+] 新增Peer: {:?}", peer);
        
        // 发送连接成功事件
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

    pub async fn start_tcp_server(&mut self, port: u16) -> Result<()> {
        let addr = format!("0.0.0.0:{}", port);
        let listener = TcpListener::bind(&addr)
            .await
            .context("绑定TCP端口失败")?;
        
        self.port = port;
        println!("[*] P2P服务监听在 {}:{}", self.ip, port);
        
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    println!("[*] 新连接: {}", addr);
                    self.handle_connection(stream).await;
                }
                Err(e) => {
                    eprintln!("[!] 连接错误: {}", e);
                }
            }
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream) {
        let mut buf = [0; 4096];
        match stream.read(&mut buf).await {
            Ok(n) if n > 0 => {
                let msg = String::from_utf8_lossy(&buf[..n]);
                println!("[*] 收到数据: {}", msg.trim());
                
                // 解析连接信息
                if msg.starts_with("PING ") {
                    let peer_ip = &msg[5..];
                    let reply = format!("PONG {}", self.ip);
                    if let Err(e) = stream.write_all(reply.as_bytes()).await {
                        eprintln!("[!] 发送失败: {}", e);
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
                    
                    // 解析对方的IP
                    if reply.starts_with("PONG ") {
                        let peer_ip = &reply[5..];
                        return Ok(format!("✅ P2P连接成功!\n📍 对方IP: {}\n🔗 连接已建立", peer_ip));
                    }
                }
                
                Ok("✅ 连接成功，但未能解析对方IP".to_string())
            }
            Err(e) => {
                eprintln!("[!] 连接失败: {}", e);
                Err(e).context("无法连接到对端")
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
    pub ret_code: i32,
    pub data: Option<T>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UserInfo {
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

#[derive(Debug, Deserialize)]
pub struct Event {
    #[serde(rename = "post_type")]
    pub post_type: String,
    #[serde(rename = "message_type")]
    pub message_type: Option<String>,
    #[serde(rename = "sub_type")]
    pub sub_type: Option<String>,
    #[serde(rename = "user_id")]
    pub user_id: u64,
    #[serde(rename = "group_id")]
    pub group_id: Option<u64>,
    #[serde(rename = "raw_message")]
    pub raw_message: Option<String>,
    #[serde(rename = "message")]
    pub message: Option<String>,
    #[serde(rename = "sender")]
    pub sender: Option<Sender>,
}

#[derive(Debug, Deserialize)]
pub struct Sender {
    #[serde(rename = "user_id")]
    pub user_id: u64,
    #[serde(rename = "nickname")]
    pub nickname: String,
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

    pub async fn get_login_info(&self) -> Result<UserInfo> {
        let url = format!("{}/get_login_info", self.base_url());
        let resp: NapCatResponse<UserInfo> = self.client.get(&url).send().await?
            .json().await?;
        
        if resp.ret_code != 0 {
            anyhow::bail!("获取登录信息失败: {:?}", resp.message);
        }
        
        Ok(resp.data.context("数据为空")?)
    }

    pub async fn get_friends(&self) -> Result<Vec<FriendInfo>> {
        let url = format!("{}/get_friends", self.base_url());
        let resp: NapCatResponse<Vec<FriendInfo>> = self.client.get(&url).send().await?
            .json().await?;
        
        if resp.ret_code != 0 {
            anyhow::bail!("获取好友列表失败: {:?}", resp.message);
        }
        
        Ok(resp.data.unwrap_or_default())
    }

    pub async fn get_groups(&self) -> Result<Vec<GroupInfo>> {
        let url = format!("{}/get_groups", self.base_url());
        let resp: NapCatResponse<Vec<GroupInfo>> = self.client.get(&url).send().await?
            .json().await?;
        
        if resp.ret_code != 0 {
            anyhow::bail!("获取群列表失败: {:?}", resp.message);
        }
        
        Ok(resp.data.unwrap_or_default())
    }

    pub async fn send_private_message(&self, user_id: u64, message: &str) -> Result<i64> {
        let url = format!("{}/send_private_message", self.base_url());
        let params = serde_json::json!({
            "user_id": user_id,
            "message": message
        });
        
        let resp: NapCatResponse<()> = self.client.post(&url)
            .json(&params)
            .send()
            .await?
            .json()
            .await?;
        
        if resp.ret_code != 0 {
            anyhow::bail!("发送私聊消息失败: {:?}", resp.message);
        }
        
        Ok(0)
    }

    pub async fn send_group_message(&self, group_id: u64, message: &str) -> Result<i64> {
        let url = format!("{}/send_group_message", self.base_url());
        let params = serde_json::json!({
            "group_id": group_id,
            "message": message
        });
        
        let resp: NapCatResponse<()> = self.client.post(&url)
            .json(&params)
            .send()
            .await?
            .json()
            .await?;
        
        if resp.ret_code != 0 {
            anyhow::bail!("发送群消息失败: {:?}", resp.message);
        }
        
        Ok(0)
    }

    pub async fn check_online(&self) -> Result<bool> {
        let url = format!("{}/get_login_info", self.base_url());
        Ok(self.client.get(&url).send().await.is_ok())
    }
}

#[derive(Debug)]
pub struct BotWebSocket {
    ws_url: String,
}

impl BotWebSocket {
    pub fn new(config: &NapCatConfig) -> Self {
        Self {
            ws_url: format!("ws://{}:{}", config.ws_host, config.ws_port),
        }
    }

    pub async fn connect(&self) -> Result<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>> {
        let url: Url = self.ws_url.parse()?;
        let (ws, _) = tokio_tungstenite::connect_async(url).await?;
        Ok(ws)
    }
}
