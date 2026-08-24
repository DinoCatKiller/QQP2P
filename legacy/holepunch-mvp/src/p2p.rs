//! P2P 节点（打洞 MVP 支撑版）：仅保留 UDP 打洞所需的最小状态与接口。
//!
//! 归档自 QQP2P 主工程 `src/p2p.rs`（2026-08-24）。
//! 正式项目已转向 libp2p 方案（见 `docs/plan/P2P_INFRA_PLAN.md`），
//! 本文件仅作为 MVP（双方 UDP 打洞）独立联调工具的支撑，只维护不演进。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, broadcast};
use anyhow::{Context, Result};

/// 机器人内部事件：打洞结果（供 CLI 回显）
#[derive(Debug, Clone)]
pub enum BotEvent {
    /// UDP 打洞结果：会话结束（成功/失败）时发出
    HolePunchResult {
        #[allow(dead_code)]
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
    /// 对端标识（命令行 `--peer-uid`，仅日志用）
    pub peer_user_id: u64,
    /// 本机 NAT 映射地址
    pub my_mapped: SocketAddr,
    /// 对方 NAT 映射地址
    pub peer_mapped: SocketAddr,
    pub state: SessionState,
}

/// P2P 节点（打洞 MVP 支撑版）
#[derive(Debug)]
pub struct P2PNode {
    /// 本机标识（命令行 `--peer-uid`）
    pub user_id: u64,
    /// UDP 打洞 socket（STUN 查询与打洞发包共用，保证映射一致性）
    pub udp_sock: Option<Arc<UdpSocket>>,
    /// UDP 打洞监听端口
    pub udp_port: u16,
    /// STUN 服务器地址
    pub stun_server: Option<SocketAddr>,
    /// 本机 NAT 映射地址缓存（STUN 查询结果）
    pub my_mapped: Option<SocketAddr>,
    /// 打洞会话表（key = 对端标识）
    pub hole_sessions: Arc<Mutex<HashMap<u64, HolePunchSession>>>,
    event_tx: Arc<Mutex<broadcast::Sender<BotEvent>>>,
}

impl P2PNode {
    /// 构造节点（无需 NapCat，不查询公网 IP；仅需 UDP 打洞的命令使用）
    pub async fn new_offline(user_id: u64) -> Result<(Self, broadcast::Receiver<BotEvent>)> {
        let (event_tx, event_rx) = broadcast::channel(100);
        Ok((
            Self {
                user_id,
                udp_sock: None,
                udp_port: 0,
                stun_server: None,
                my_mapped: None,
                hole_sessions: Arc::new(Mutex::new(HashMap::new())),
                event_tx: Arc::new(Mutex::new(event_tx)),
            },
            event_rx,
        ))
    }

    pub async fn send_event(&self, event: BotEvent) {
        let tx = self.event_tx.lock().await;
        let _ = tx.send(event);
    }

    /// 启动 UDP 打洞服务：绑定 UDP socket、查询 STUN 缓存映射地址、监听打洞报文。
    /// socket 全程共享，STUN 查询与打洞发包使用同一 socket（映射一致性）。
    /// `keepalive_interval` 为后台保活周期（周期性向 STUN 发包维持 NAT 映射），0 表示禁用。
    pub async fn start_udp_server(
        node: Arc<Mutex<P2PNode>>,
        port: u16,
        stun_server: SocketAddr,
        keepalive_interval: Duration,
    ) -> Result<()> {
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

        // 启动时查询一次 STUN（带重试），缓存本机映射
        match crate::holepunch::query_mapped_addr_retry(&sock, stun_server).await {
            Ok(m) => {
                println!("[+] 本机STUN映射地址: {}", m);
                let mut n = node.lock().await;
                n.my_mapped = Some(m);
            }
            Err(e) => {
                eprintln!("[!] STUN 查询失败: {}", e);
            }
        }

        // UDP 监听循环
        let node2 = Arc::clone(&node);
        tokio::spawn(async move {
            if let Err(e) = crate::holepunch::run_udp_listener(node2).await {
                eprintln!("[!] UDP监听循环退出: {}", e);
            }
        });

        // 后台保活：周期性向 STUN 发包，维持 NAT 映射不超时
        if !keepalive_interval.is_zero() {
            let node3 = Arc::clone(&node);
            tokio::spawn(async move {
                crate::holepunch::keepalive_loop(node3, keepalive_interval).await;
            });
        }

        Ok(())
    }
}
