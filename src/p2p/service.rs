//! P2P 服务层
//!
//! 负责：传输层 POC 的核心逻辑协调
//! - 启动/停止 libp2p Swarm
//! - /tun/1.0.0 协议处理（HELLO / JOIN_ACK 交换）
//! - 对等节点连接与发现
//! - 消息发送（DATA 帧）

use futures_util::StreamExt;
use libp2p::swarm::Swarm;
use libp2p::{Multiaddr, PeerId};
use std::collections::HashMap;

use crate::p2p::protocol::{MemberInfo, TunProtocolHandler};

/// N1 传输层 POC 服务
///
/// 协调 libp2p 传输层与 /tun/1.0.0 协议的交互。
/// 提供简化的 API 供上层（如 main.rs 或 CLI）使用。
pub struct P2pService {
    /// libp2p Swarm - 核心事件循环和连接管理
    swarm: Swarm<libp2p::identify::Identify>,
    /// /tun/1.0.0 协议处理器
    protocol: TunProtocolHandler,
    /// 本机虚拟 IP，用于 HELLO 消息宣告
    local_virtual_ip: String,
    /// 已知对等节点：PeerId -> (Multiaddr, 虚拟IP)
    known_peers: HashMap<PeerId, (Multiaddr, String)>,
}

impl P2pService {
    /// 创建新的 P2P 服务实例
    ///
    /// # 参数
    /// * `listen_port` - 监听的 TCP/UDP 端口
    /// * `local_virtual_ip` - 本机分配的虚拟 IP (如 "10.0.0.1")
    pub async fn new(listen_port: u16, local_virtual_ip: String) -> Result<Self, anyhow::Error> {
        // 启动传输层端点
        let (swarm, listen_multiaddr, peer_id) = crate::p2p::transport::start_transport(listen_port).await
            .map_err(|e| anyhow::anyhow!("启动传输层失败: {}", e))?;

        let protocol = TunProtocolHandler::new(peer_id, local_virtual_ip.clone());

        println!("[*] P2PService 创建完成");
        println!("[*] 监听地址: {}", listen_multiaddr);

        Ok(Self {
            swarm,
            protocol,
            local_virtual_ip,
            known_peers: HashMap::new(),
        })
    }

    /// 获取本机 PeerId
    pub fn peer_id(&self) -> &PeerId {
        self.protocol.local_peer_id()
    }

    /// 获取本机虚拟 IP
    pub fn local_virtual_ip(&self) -> &str {
        &self.local_virtual_ip
    }

    /// 连接到远程对等节点
    ///
    /// # 参数
    /// * `peer_id` - 远程对等节点的 PeerId
    /// * `multiaddr` - 远程多地址 (通过 QQ 信令获取)
    pub async fn connect_to_peer(&mut self, peer_id: PeerId, multiaddr: Multiaddr) {
        // 向 swarm 发起连接请求
        let _ = self.swarm.dial(multiaddr.clone());
        println!("[*] 正在连接对等节点: {}", peer_id);

        // 将对等节点记录到本地表
        self.known_peers.insert(peer_id, (multiaddr, String::new()));
    }

    /// 发送 HELLO 消息 (启动 /tun/1.0.0 协议交换)
    pub fn send_hello(&self, _target_peer_id: &PeerId) -> Vec<u8> {
        self.protocol.build_hello_frame()
    }

    /// 发送 JOIN_ACK 消息
    pub fn send_join_ack(members: &[MemberInfo]) -> Vec<u8> {
        TunProtocolHandler::encode_join_ack(members)
    }

    /// 处理下一个 Swarm 事件
    pub async fn poll_events(&mut self) {
        use libp2p::swarm::SwarmEvent;
        if let Some(event) = self.swarm.next().await {
            match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    println!("[*] 连接已建立: {}", peer_id);
                }
                SwarmEvent::Behaviour(_) => {
                    // 行为事件由协议处理器内部处理
                }
                SwarmEvent::ListenerError { listener_id, error } => {
                    eprintln!("[!] 监听器错误 {:?}: {}", listener_id, error);
                }
                _ => {
                    // 其他事件静默忽略
                }
            }
        }
    }
}
