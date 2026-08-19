use std::sync::Arc;
use tokio::sync::Mutex;
use anyhow::Result;

use crate::napcat::NapCatClient;
use crate::napcat::P2PNode;

#[derive(Clone)]
pub struct BotApp {
    pub node: Arc<Mutex<P2PNode>>,
    pub napcat: Arc<Mutex<NapCatClient>>,
    pub my_user_id: u64,
    pub my_name: String,
}

impl BotApp {
    pub async fn new(user_id: u64) -> Result<(Self, tokio::sync::broadcast::Receiver<crate::napcat::BotEvent>)> {
        let (node, event_rx) = P2PNode::new(user_id).await?;
        let napcat = NapCatClient::new_default().await?;

        // 从登录账号信息里拿机器人昵称(用于 @ 匹配); 失败时降级为空, 不影响启动
        let my_name = match napcat.get_login_info().await {
            Ok(info) => info.nickname,
            Err(e) => {
                eprintln!("[!] 获取登录昵称失败(仅按QQ号匹配@): {e}");
                String::new()
            }
        };

        Ok((Self {
            node: Arc::new(Mutex::new(node)),
            napcat: Arc::new(Mutex::new(napcat)),
            my_user_id: user_id,
            my_name,
        }, event_rx))
    }

    pub async fn send_event(&self, event: crate::napcat::BotEvent) {
        let node = self.node.lock().await;
        node.send_event(event).await;
    }

    pub async fn handle_message(&self, sender_id: u64, raw_message: &str) -> Option<String> {
        let msg = raw_message.trim().to_lowercase();
        
        // 检查是否是@机器人的消息（群组中）
        // 这里简化处理，直接解析命令
        
        // 流程1: 对方发送 "给我一个p2p" 或类似
        if msg.contains("给我") && msg.contains("p2p") || msg.contains("/ip") || msg == "p2p" {
            let node = self.node.lock().await;
            return Some(node.get_ip_info().await);
        }
        
        // 流程2: 对方发送 "请你跟我这样做" 或类似
        if msg.contains("请你") && msg.contains("跟我") || msg.contains("/请你") {
            let _node = self.node.lock().await;
            return Some("👋 好的，请告诉我你的P2P信息（IP:端口）\n例如: 1.2.3.4:8080".to_string());
        }
        
        // 流程3: 对方发送自己的IP信息
        if msg.contains("ip") && msg.contains(":") || msg.contains("公网") || msg.contains("端口") {
            let node = self.node.lock().await;
            
            // 尝试解析并存储对方IP
            if let Some(reply) = node.parse_and_store_peer_ip(sender_id, raw_message).await {
                // 存储对方的user_id到peers（临时）
                drop(node);
                
                // 自动尝试连接
                let node = self.node.lock().await;
                node.try_auto_connect(sender_id).await;
                
                return Some(reply);
            }
        }
        
        // 流程4: 手动连接命令
        if msg.contains("/connect") || msg.contains("连接") {
            let parts: Vec<&str> = msg.split_whitespace().collect();
            if parts.len() >= 2 {
                let target = parts[1];
                let ip_parts: Vec<&str> = target.split(':').collect();
                if ip_parts.len() == 2 {
                    let ip = ip_parts[0];
                    if let Ok(port) = ip_parts[1].parse::<u16>() {
                        let node = self.node.lock().await;
                        match node.connect_to_peer(ip, port).await {
                            Ok(result) => {
                                // 添加到peers
                                let mut peers = node.peers.lock().await;
                                peers.insert(sender_id, crate::napcat::PeerInfo {
                                    user_id: sender_id,
                                    ip: ip.to_string(),
                                    port,
                                });
                                return Some(result);
                            }
                            Err(e) => return Some(format!("❌ 连接失败: {}", e)),
                        }
                    }
                }
            }
            return Some("❌ 请使用 /connect IP:PORT 格式".to_string());
        }
        
        // 流程5: 查看状态
        if msg.contains("/status") || msg == "状态" {
            let node = self.node.lock().await;
            let peers = node.peers.lock().await;
            let peer_count = peers.len();
            
            if peer_count == 0 {
                return Some(format!("📊 当前状态:\n✅ 本机IP: {}\n🔌 端口: {}\n👥 已连接Peer: 0\n\n💡 让对方发送 /ip 获取其IP并告诉你", node.ip, node.port));
            } else {
                let peer_list: Vec<String> = peers.values()
                    .map(|p| format!("  • {} -> {}:{}", p.user_id, p.ip, p.port))
                    .collect();
                return Some(format!("📊 当前状态:\n✅ 本机IP: {}\n🔌 端口: {}\n👥 已连接Peer: {}\n{}", node.ip, node.port, peer_count, peer_list.join("\n")));
            }
        }
        
        // 流程6: 帮助
        if msg.contains("/help") || msg == "帮助" {
            return Some("📖 P2P机器人帮助:\n\n".to_string()
                + "🔹 发送「给我一个p2p」- 获取本机IP信息\n"
                + "🔹 发送「请你跟我这样做」- 询问对方IP\n"
                + "🔹 发送你的IP:端口 - 自动连接对方\n"
                + "🔹 /connect IP:PORT - 手动连接\n"
                + "🔹 /status - 查看状态\n"
                + "🔹 /help - 显示此帮助\n\n"
                + "💡 完整流程:\n"
                + "1. 对方发送「给我一个p2p」获取你的IP\n"
                + "2. 对方发送「请你跟我这样做」询问你的IP\n"
                + "3. 你发送你的IP:端口给对方\n"
                + "4. 对方自动连接你");
        }
        
        None
    }

    pub async fn send_reply(&self, user_id: u64, message: &str) -> Result<()> {
        let napcat = self.napcat.lock().await;
        napcat.send_private_message(user_id, message).await?;
        Ok(())
    }

    pub async fn send_group_reply(&self, group_id: u64, message: &str) -> Result<()> {
        let napcat = self.napcat.lock().await;
        napcat.send_group_message(group_id, message).await?;
        Ok(())
    }
}
