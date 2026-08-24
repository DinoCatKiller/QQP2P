//! libp2p 传输层端点
//!
//! 负责：Endpoint 启动、监听、PeerId 打印
//! 遵循文档：/tun/1.0.0 协议家族
//!
//! N1 阶段：TCP + Noise + Yamux

use std::time::Duration;

use libp2p::identity;
use libp2p::swarm::Swarm;

// -----------------------------------------------------------
// 核心类型重导出
// -----------------------------------------------------------

/// libp2p PeerId 类型
pub use libp2p::PeerId;

/// libp2p Multiaddr 类型
pub use libp2p::Multiaddr;

// -----------------------------------------------------------
// 传输层启动函数
// -----------------------------------------------------------

/// 启动 libp2p 传输层端点并开始监听
///
/// # 参数
/// * `listen_port` - 监听端口号
///
/// # 返回
/// * `(Swarm, Multiaddr, PeerId)` - Swarm 实例、监听多地址、本地 PeerId
pub async fn start_transport(
    listen_port: u16,
) -> anyhow::Result<(Swarm<libp2p::identify::Identify>, Multiaddr, PeerId)> {
    // 1. 生成本地密钥对和 PeerId
    let local_keypair = identity::Keypair::generate_ed25519();
    let peer_id = PeerId::from(local_keypair.public());

    println!("[*] 本地 PeerId: {}", peer_id);

    // 2. 使用 development_transport 构建传输层 (TCP + Noise + Yamux)
    let transport = libp2p::development_transport(local_keypair.clone()).await?;

    // 3. Identify 协议 - 使用默认配置
    let behaviour = libp2p::identify::Identify::new(
        libp2p::identify::IdentifyConfig::new("/tun/1.0.0".into(), local_keypair.public())
            .with_interval(Duration::from_secs(15)),
    );

    // 4. 创建 Swarm
    let mut swarm = Swarm::new(transport, behaviour, peer_id);

    // 5. 监听端口
    let listen_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", listen_port).parse()?;

    swarm.listen_on(listen_addr.clone())
        .map_err(|e| anyhow::anyhow!("监听地址失败: {}", e))?;

    println!("[*] 正在监听: {}", listen_addr);

    Ok((swarm, listen_addr, peer_id))
}

/// 将多地址转换为字符串便于打印
#[allow(dead_code)]
pub fn format_multiaddr(addr: &Multiaddr) -> String {
    addr.to_string()
}
