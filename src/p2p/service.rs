//! P2P 服务层
//!
//! 负责：传输层 POC 的核心逻辑协调
//! - 启动/停止 libp2p Swarm
//! - /tun/1.0.0 协议处理（HELLO / JOIN_ACK 交换）
//! - 对等节点连接与发现
//! - 消息发送（DATA 帧）

#![allow(dead_code)]

use futures_util::StreamExt;
use libp2p::swarm::Swarm;
use libp2p::{Multiaddr, PeerId};
use std::collections::HashMap;

use crate::p2p::protocol::{HelloMsg, JoinAckMsg, MemberInfo, TunProtocolHandler};
use crate::p2p::transport::{TunBehaviour, TunBehaviourEvent};

/// N1 传输层 POC 服务
///
/// 协调 libp2p 传输层与 /tun/1.0.0 协议的交互。
/// 提供简化的 API 供上层（如 main.rs 或 CLI）使用。
pub struct P2pService {
    /// libp2p Swarm - 核心事件循环和连接管理
    swarm: Swarm<TunBehaviour>,
    /// /tun/1.0.0 协议处理器（帧编解码辅助）
    protocol: TunProtocolHandler,
    /// 本机虚拟 IP，用于 HELLO 消息宣告
    local_virtual_ip: String,
    /// 监听端口，用于构造可拨号地址
    listen_port: u16,
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
            listen_port,
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

    /// 通过多地址字符串拨号（内部解析，便于 CLI 直接传字符串）
    pub fn dial_addr(&mut self, addr_str: &str) {
        match addr_str.parse::<Multiaddr>() {
            Ok(addr) => match self.swarm.dial(addr.clone()) {
                Ok(_) => println!("[*] 已发起拨号: {}", addr),
                Err(e) => eprintln!("[!] 拨号失败: {}", e),
            },
            Err(e) => eprintln!("[!] 无效的多地址 '{}': {}", addr_str, e),
        }
    }

    /// 返回本机可拨号地址（回环，用于同机双进程联调）
    pub fn dialable_addr(&self) -> String {
        format!("/ip4/127.0.0.1/udp/{}/quic-v1/p2p/{}", self.listen_port, self.peer_id())
    }

    /// 发送 HELLO 请求 (通过 /tun/1.0.0 request-response)
    ///
    /// 连接建立后调用此方法向对端发送 HELLO，对端会回复 JOIN_ACK
    pub fn send_hello(&mut self, target_peer_id: &PeerId) {
        let hello = HelloMsg {
            peer_id: self.protocol.local_peer_id().to_base58(),
            virtual_ip: self.local_virtual_ip.clone(),
            features: 0b11, // 位 0 = DCUtR 支持, 位 1 = QUIC 支持
        };
        let req_id = self.swarm.behaviour_mut().tun.send_request(target_peer_id, hello);
        println!("[*] 已发送 HELLO 请求 (id={})", req_id);
    }

    /// 处理下一个 Swarm 事件
    pub async fn poll_events(&mut self) {
        use libp2p::swarm::SwarmEvent;

        if let Some(event) = self.swarm.next().await {
            match event {
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                    println!("[+] 连接已建立: {} (endpoint: {:?})", peer_id, endpoint);
                    // 连接建立后自动发送 HELLO
                    self.send_hello(&peer_id);
                }
                SwarmEvent::ConnectionClosed { peer_id, endpoint, .. } => {
                    println!("[-] 连接已关闭: {} (endpoint: {:?})", peer_id, endpoint);
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    println!("[*] 新的监听地址: {}", address);
                }
                SwarmEvent::IncomingConnection { send_back_addr, .. } => {
                    println!("[*] 收到入站连接: {}", send_back_addr);
                }
                SwarmEvent::Behaviour(event) => {
                    self.handle_behaviour_event(event);
                }
                SwarmEvent::ListenerError { listener_id, error } => {
                    eprintln!("[!] 监听器错误 {:?}: {}", listener_id, error);
                }
                _ => {}
            }
        }
    }

    /// 处理组合行为事件 (identify + request_response)
    fn handle_behaviour_event(&mut self, event: TunBehaviourEvent) {
        match event {
            TunBehaviourEvent::Identify(identify_event) => {
                use libp2p::identify::Event as IdentifyEvent;
                match identify_event {
                    IdentifyEvent::Received { peer_id, info, .. } => {
                        println!(
                            "[*] Identify 收到对端信息: {} (agent={:?}, protocols={:?})",
                            peer_id, info.agent_version, info.protocols
                        );
                    }
                    IdentifyEvent::Sent { peer_id, .. } => {
                        println!("[*] Identify 已发送本机信息给: {}", peer_id);
                    }
                    IdentifyEvent::Pushed { .. } => {}
                    IdentifyEvent::Error { peer_id, error, .. } => {
                        eprintln!("[!] Identify 错误 (peer={}): {}", peer_id, error);
                    }
                }
            }
            TunBehaviourEvent::Tun(rr_event) => {
                use libp2p::request_response::{Event as RREvent, Message as RRMessage};
                match rr_event {
                    RREvent::Message { peer, message, .. } => match message {
                        RRMessage::Request { request, channel, .. } => {
                            // 收到 HELLO 请求，回复 JOIN_ACK
                            println!(
                                "[+] 收到 HELLO: peer_id={}, virtual_ip={}, features={:#010b}",
                                request.peer_id, request.virtual_ip, request.features
                            );

                            // 记录对端到已知节点表
                            let addr: Multiaddr = format!("/p2p/{}", peer)
                                .parse()
                                .unwrap_or_else(|_| "/ip4/0.0.0.0/tcp/0".parse().unwrap());
                            self.known_peers.insert(
                                peer,
                                (addr, request.virtual_ip.clone()),
                            );

                            // 构造 JOIN_ACK 响应（成员表包含自己 + 对端）
                            let mut members = vec![MemberInfo {
                                peer_id: self.protocol.local_peer_id().to_base58(),
                                virtual_ip: self.local_virtual_ip.clone(),
                            }];
                            members.push(MemberInfo {
                                peer_id: request.peer_id.clone(),
                                virtual_ip: request.virtual_ip.clone(),
                            });
                            let join_ack = JoinAckMsg { members };

                            match self
                                .swarm
                                .behaviour_mut()
                                .tun
                                .send_response(channel, join_ack)
                            {
                                Ok(_) => println!("[*] 已回复 JOIN_ACK 给: {}", peer),
                                Err(_) => eprintln!("[!] 回复 JOIN_ACK 失败: {}", peer),
                            }
                        }
                        RRMessage::Response { response, .. } => {
                            // 收到 JOIN_ACK 响应
                            println!("[+] 收到 JOIN_ACK: 成员数={}", response.members.len());
                            for m in &response.members {
                                println!(
                                    "    • peer_id={}, virtual_ip={}",
                                    m.peer_id, m.virtual_ip
                                );
                            }
                        }
                    },
                    RREvent::ResponseSent { peer, .. } => {
                        println!("[*] JOIN_ACK 已送达: {}", peer);
                    }
                    RREvent::OutboundFailure { peer, error, .. } => {
                        eprintln!("[!] 出站请求失败 (peer={}): {}", peer, error);
                    }
                    RREvent::InboundFailure { peer, error, .. } => {
                        eprintln!("[!] 入站请求失败 (peer={}): {}", peer, error);
                    }
                }
            }
        }
    }

    /// 发送 JOIN_ACK 消息（静态方法，保留兼容）
    pub fn send_join_ack(members: &[MemberInfo]) -> Vec<u8> {
        TunProtocolHandler::encode_join_ack(members)
    }
}
