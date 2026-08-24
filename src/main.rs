//! QQP2P 入口：CLI 参数解析 + 各子命令的任务装配。
//!
//! 模块划分：
//! - `napcat`：NapCat HTTP API 客户端
//! - `p2p`  ：P2P 节点（既有 TCP 传输，也有 libp2p 传输 POC）
//! - `ws`   ：WebSocket 事件监听与消息分发
//! - `app`  ：BotApp 消息处理

mod app;
mod napcat;
mod p2p;
mod ws;

use std::sync::Arc;
use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::app::BotApp;
use crate::napcat::NapCatClient;
use crate::p2p::{P2pService, start_transport};  // 新的 libp2p P2P Service
use crate::legacy_p2p::P2PNode;      // 旧的 TCP P2PNode (兼容)

// 兼容旧文件名（已重命名为 legacy_p2p.rs，保留向后兼忽）
#[allow(dead_code)]
mod legacy_p2p;

#[derive(Parser, Debug)]
#[command(name = "qqp2p")]
#[command(about = "QQ P2P连接机器人 - 通过@消息自自动建立P2P连接")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 启动QQ P2P 机器人（监听QQ消息）
    Start {
        #[arg(short, long, default_value = "12345")]
        user_id: u64,
        #[arg(short, long, default_value = "8080")]
        port: u16,
    },
    /// 查询本机IP
    Ip {
        #[arg(short, long, default_value = "12345")]
        user_id: u64,
    },
    /// 连接到对端 (使用传统 TCP 方式)
    Connect {
        #[arg(short, long)]
        target: String,
        #[arg(short, long, default_value = "12345")]
        user_id: u64,
    },
    /// 查看连接状态
    Status {
        #[arg(short, long, default_value = "12345")]
        user_id: u64,
    },
    /// 发送私聊消息
    Send {
        #[arg(short, long)]
        to: u64,
        #[arg(short, long)]
        msg: String,
    },
    /// 发送群消息
    SendGroup {
        #[arg(short, long)]
        group: u64,
        #[arg(short, long)]
        msg: String,
    },
    /// 查看好友列表
    Friends,
    /// 查看群列表
    Groups,
    /// 检查登录状态
    Online,
    /// 启动 libp2p 传输层 POC (N1 里程碑)
    P2pStart {
        #[arg(short, long, default_value = "30303")]
        port: u16,
    },
    /// 通过 PeerId 连接 (N1 联调子命令)
    P2pConnect {
        #[arg(short, long)]
        peer_id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { user_id, port } => {
            println!("[*] 启动 QQ P2P 机器人...");
            println!("[*] 用户ID: {}", user_id);
            println!("[*] TCP端口: {}", port);
            println!();

            let (app, event_rx) = BotApp::new(user_id).await?;

            let ip = P2PNode::get_public_ip().await?;
            println!("[+] 公网IP: {}", ip);
            println!("[+] 机器人昵称: {}", app.my_name);
            println!("[*] 机器人已就绪!");
            println!("[*] 私聊直接回复; 群聊需 @{} 或互发P2P节点信息", app.my_name);
            println!();
            println!("[*] 使用说明:");
            println!("[*]   • 对方在QQ中 @你 发送: /ip");
            println!("[*]   • 机器人回复你的IP信息");
            println!("[*]   • 对方发送: /connect IP:PORT");
            println!("[*]   • 双方自动建立P2P连接");
            println!();

            // N1: 同时启动 libp2p 传输层 (后台)，TCP 服务器由 libp2p 处理
            let _p2p_handle = tokio::spawn(async move {
                let _ = start_transport(port).await;
            });

            let _app_clone = Arc::clone(&app.node);
            let tcp_handle = tokio::spawn(async move {
                // P2PNode::start_tcp_server(app_clone, port).await
                println!("[*] TCP 服务器由 libp2p 传输层处理 (N1 阶段)");
            });

            let app_clone = app.clone();
            let msg_handle = tokio::spawn(async move {
                ws::run_message_handler(app_clone, event_rx).await
            });

            let ws_app = app.clone();
            tokio::spawn(async move {
                let _ = ws::websocket_listener(ws_app).await;
            });

            println!("[*] 等待QQ消息...");
            println!("[*] 按 Ctrl+C 退出");

            drop(tcp_handle);
            let _ = msg_handle.await?;

            Ok(())
        }

        Commands::Ip { user_id } => {
            let (node, _) = P2PNode::new(user_id).await?;
            println!("{}", node.get_ip_info().await);
            Ok(())
        }

        Commands::Connect { target, user_id } => {
            println!("[*] 连接到: {}", target);

            let parts: Vec<&str> = target.split(':').collect();
            if parts.len() != 2 {
                eprintln!("[!] 无效格式，请使用 IP:PORT");
                std::process::exit(1);
            }

            let ip = parts[0];
            let port: u16 = parts[1].parse()?;

            let (node, _) = P2PNode::new(user_id).await?;

            match node.connect_to_peer(ip, port).await {
                Ok(result) => println!("{}", result),
                Err(e) => eprintln!("[!] 连接失败: {}", e),
            }
            Ok(())
        }

        Commands::Status { user_id } => {
            let (node, _) = P2PNode::new(user_id).await?;
            let peers = node.peers.lock().await;

            println!("📊 P2P状态:");
            println!("  本机IP: {}", node.ip);
            println!("  端口: {}", node.port);
            println!("  已连接Peer: {}", peers.len());

            if peers.is_empty() {
                println!("\n💡 提示: 让对方发送 /ip 获取其IP，然后你发送 /connect IP:PORT");
            } else {
                for (uid, peer) in peers.iter() {
                    println!("  • {} -> {}:{}", uid, peer.ip, peer.port);
                }
            }
            Ok(())
        }

        Commands::Send { to, msg } => {
            let napcat = NapCatClient::new_default().await?;
            napcat.send_private_message(to, &msg).await?;
            println!("[+] 消息已发送至: {}", to);
            Ok(())
        }

        Commands::SendGroup { group, msg } => {
            let napcat = NapCatClient::new_default().await?;
            napcat.send_group_message(group, &msg).await?;
            println!("[+] 消息已发送至群: {}", group);
            Ok(())
        }

        Commands::Friends => {
            let napcat = NapCatClient::new_default().await?;
            match napcat.get_friends().await {
                Ok(friends) => {
                    println!("[*] 好友列表:");
                    for f in friends {
                        println!("  • {} ({})", f.nickname, f.user_id);
                    }
                }
                Err(e) => println!("[!] 获取好友列表失败: {}", e),
            }
            Ok(())
        }

        Commands::Groups => {
            let napcat = NapCatClient::new_default().await?;
            match napcat.get_groups().await {
                Ok(groups) => {
                    println!("[*] 群列表:");
                    for g in groups {
                        println!("  • {} ({})", g.group_name, g.group_id);
                    }
                }
                Err(e) => println!("[!] 获取群列表失败: {}", e),
            }
            Ok(())
        }

        Commands::Online => {
            let napcat = NapCatClient::new_default().await?;
            match napcat.check_online().await {
                Ok(true) => println!("[+] 机器人已在线"),
                Ok(false) => println!("[!] 机器人未在线，请检查 NapCat 是否启动"),
                Err(e) => println!("[!] 检查失败: {}", e),
            }
            Ok(())
        }

        // N1: 启动 libp2p 传输层 POC
        Commands::P2pStart { port } => {
            println!("[*] 启动 libp2p 传输层 POC (N1 里程碑)...");
            println!("[*] 监听端口: {}", port);

            // 初始化 libp2p 传输层
            let (_swarm, listen_multiaddr, peer_id) = start_transport(port).await?;

            println!("[*] 本地 PeerId: {}", peer_id);
            println!("[*] 监听地址: {}", listen_multiaddr);

            // 创建 P2PService 实例
            let local_virtual_ip = "10.0.0.1".to_string(); // 示例虚拟 IP
            let p2p_service = P2pService::new(port, local_virtual_ip).await?;

            println!("[*] P2PService 创建完成");

            // 主循环：处理 swarm 事件
            let mut service = p2p_service;
            loop {
                service.poll_events().await;
            }
        }

        // N1: 通过 PeerId 连接 (手动联调)
        Commands::P2pConnect { peer_id } => {
            println!("[*] 通过 PeerId 连接: {}", peer_id);
            // TODO: 实现通过 PeerId 连接的逻辑
            // 这将需要解析对等节点的多地址并调用 P2pService::connect_to_peer
            println!("[!] 此功能将在 N1.4 联调中实现");
            Ok(())
        }
    }
}