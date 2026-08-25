//! P2P 节点：基于 libp2p 传输层 + 自研 /tun/1.0.0 协议
//!
//! N1 里程碑：传输层 POC
//! - libp2p 传输层（QUIC/UDP）
//! - DCUtR 打洞
//! - /tun/1.0.0 自定义协议（TUN 包 + 控制消息复用）
//! - 兼容旧版 TCP P2PNode（通过 legacy_p2p 模块重导出）

// 重导出旧版 P2PNode 相关类型，保持向后兼容
pub use crate::legacy_p2p::P2PNode;
pub use crate::legacy_p2p::BotEvent;
pub use crate::legacy_p2p::HANDSHAKE_CONFIRM_REPLY;
pub use crate::legacy_p2p::PeerInfo;

// Module declarations
mod transport;
mod protocol;
mod service;
pub mod holepunch;
pub mod quic_node;

pub use transport::start_transport;
pub use service::P2pService;
pub use quic_node::run_p2p_node;
