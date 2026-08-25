//! libp2p 传输层端点
//!
//! 负责：Endpoint 启动、监听、PeerId 打印
//! 遵循文档：/tun/1.0.0 协议家族
//!
//! N1 阶段：QUIC (UDP) 传输，DCUtR 打洞主方案

use libp2p::swarm::Swarm;
use libp2p::{identify, request_response, SwarmBuilder};

use crate::p2p::protocol::{HelloMsg, JoinAckMsg, TUN_PROTOCOL};

// -----------------------------------------------------------
// 核心类型重导出
// -----------------------------------------------------------

/// libp2p PeerId 类型
pub use libp2p::PeerId;

/// libp2p Multiaddr 类型
pub use libp2p::Multiaddr;

// -----------------------------------------------------------
// 组合网络行为
// -----------------------------------------------------------

/// 组合行为事件枚举（手动定义，供 derive 宏 `to_swarm` 引用）
///
/// `Identify` 变体较大，使用 `Box` 装箱以减小 enum 整体体积，
/// 避免触发 `clippy::large_enum_variant` 警告。
#[derive(Debug)]
pub enum TunBehaviourEvent {
    Identify(Box<identify::Event>),
    Tun(request_response::Event<HelloMsg, JoinAckMsg>),
}

impl From<identify::Event> for TunBehaviourEvent {
    fn from(e: identify::Event) -> Self {
        TunBehaviourEvent::Identify(Box::new(e))
    }
}

impl From<request_response::Event<HelloMsg, JoinAckMsg>> for TunBehaviourEvent {
    fn from(e: request_response::Event<HelloMsg, JoinAckMsg>) -> Self {
        TunBehaviourEvent::Tun(e)
    }
}

/// 组合行为：identify + /tun/1.0.0 request-response
#[derive(libp2p::swarm::NetworkBehaviour)]
#[behaviour(to_swarm = "TunBehaviourEvent")]
pub struct TunBehaviour {
    /// identify 协议：连接自检、交换 PeerId/地址信息
    pub identify: identify::Behaviour,
    /// /tun/1.0.0 请求-响应：HELLO → JOIN_ACK 交换
    pub tun: request_response::json::Behaviour<HelloMsg, JoinAckMsg>,
}

// -----------------------------------------------------------
// 传输层启动函数
// -----------------------------------------------------------

/// 启动 libp2p 传输层端点并开始监听
///
/// # 参数
/// * `listen_port` - 监听端口号
///
/// # 返回
/// * `(Swarm<TunBehaviour>, Multiaddr, PeerId)` - Swarm 实例、监听多地址、本地 PeerId
pub async fn start_transport(
    listen_port: u16,
) -> anyhow::Result<(Swarm<TunBehaviour>, Multiaddr, PeerId)> {
    // 1. 通过 SwarmBuilder 生成身份（Ed25519 密钥对）并装配 QUIC + 组合行为
    let mut swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_quic()
        .with_behaviour(|key| {
            let identify = identify::Behaviour::new(identify::Config::new(
                "/tun/1.0.0".to_string(),
                key.public(),
            ));
            let tun = request_response::json::Behaviour::new(
                [(TUN_PROTOCOL, request_response::ProtocolSupport::Full)],
                request_response::Config::default(),
            );
            TunBehaviour { identify, tun }
        })?
        .build();

    let peer_id = *swarm.local_peer_id();
    println!("[*] 本地 PeerId: {}", peer_id);

    // 2. 监听 QUIC (UDP) 端口，协议后缀为 quic-v1
    let listen_addr: Multiaddr = format!("/ip4/0.0.0.0/udp/{}/quic-v1", listen_port).parse()?;

    swarm
        .listen_on(listen_addr.clone())
        .map_err(|e| anyhow::anyhow!("监听 QUIC 地址失败: {e}"))?;

    println!("[*] 正在监听 QUIC: {}", listen_addr);

    Ok((swarm, listen_addr, peer_id))
}

/// 将多地址转换为字符串便于打印
#[allow(dead_code)]
pub fn format_multiaddr(addr: &Multiaddr) -> String {
    addr.to_string()
}
