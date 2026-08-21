//! QQP2P 入口：CLI 参数解析 + 各子命令的任务装配。
//!
//! 模块划分：
//! - `napcat`：NapCat HTTP API 客户端
//! - `p2p`  ：P2P 节点（TCP 直连、节点信息交换）
//! - `ws`   ：WebSocket 事件监听与消息分发
//! - `app`  ：BotApp 消息处理

mod app;
mod holepunch;
mod napcat;
mod p2p;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::io::AsyncBufReadExt;
use tokio::sync::Mutex;

use crate::app::BotApp;
use crate::napcat::NapCatClient;
use crate::p2p::{BotEvent, P2PNode};

#[derive(Parser, Debug)]
#[command(name = "qqp2p")]
#[command(about = "QQ P2P连接机器人 - 通过@消息自动建立P2P连接")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 启动P2P机器人（监听QQ消息）
    Start {
        #[arg(short, long, default_value = "12345")]
        user_id: u64,
        #[arg(short, long, default_value = "8080")]
        port: u16,
        /// UDP 打洞监听端口（默认与 TCP 端口相同）
        #[arg(long)]
        udp_port: Option<u16>,
        /// STUN 服务器地址（NAT 映射查询）
        #[arg(long, default_value = "stun.l.google.com:19302")]
        stun: String,
    },
    /// 查询本机IP
    Ip {
        #[arg(short, long, default_value = "12345")]
        user_id: u64,
    },
    /// 连接到对端
    Connect {
        #[arg(short, long)]
        target: String,
        #[arg(short, long, default_value = "12345")]
        user_id: u64,
    },
    /// 命令行UDP打洞（绕开QQ信令，上帝视角联调用）
    Holepunch {
        /// UDP打洞监听端口（两台机器/进程需用不同端口）
        #[arg(short, long)]
        port: u16,
        /// 对端标识（本机会话表key，可任意填，仅用于日志）
        #[arg(long, default_value = "2")]
        peer_uid: u64,
        /// 对方NAT映射地址 (ip:port)。作为交互输入的默认值, 启动后回车即采用, 也可输入新地址覆盖
        #[arg(long)]
        peer_mapped: Option<String>,
        /// STUN服务器地址
        #[arg(long, default_value = "stun.l.google.com:19302")]
        stun: String,
        /// 打洞重试轮数上限(0=无限重试直到连通)
        #[arg(long, default_value = "0")]
        retry: u32,
        /// 映射保活间隔(秒), 周期性向STUN发包维持NAT映射不超时(0=禁用保活)
        #[arg(long, default_value = "20")]
        keepalive: u64,
    },
    /// 仅查询本机NAT映射地址(公网IP:端口), 查完即退
    Mapped {
        /// 本地UDP端口(0=系统自动分配)
        #[arg(short, long, default_value = "0")]
        port: u16,
        /// STUN服务器地址
        #[arg(long, default_value = "stun.l.google.com:19302")]
        stun: String,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { user_id, port, udp_port, stun } => {
            println!("[*] 启动 QQ P2P 机器人...");
            println!("[*] 用户ID: {}", user_id);
            println!("[*] TCP端口: {}", port);
            let udp_port = udp_port.unwrap_or(port);
            println!("[*] UDP打洞端口: {}", udp_port);
            println!("[*] STUN服务器: {}", stun);
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

            let app_clone = Arc::clone(&app.node);
            let tcp_handle = tokio::spawn(async move {
                P2PNode::start_tcp_server(app_clone, port).await
            });

            // UDP 打洞服务：解析 STUN 地址 → 绑定 UDP → 查询映射 → 监听打洞报文 + 后台保活
            let udp_app = Arc::clone(&app.node);
            tokio::spawn(async move {
                match crate::holepunch::resolve_stun_server(&stun).await {
                    Ok(srv) => {
                        if let Err(e) = P2PNode::start_udp_server(
                            udp_app,
                            udp_port,
                            srv,
                            Duration::from_secs(20),
                        )
                        .await
                        {
                            eprintln!("[!] UDP打洞服务启动失败: {}", e);
                        }
                    }
                    Err(e) => eprintln!("[!] STUN 地址解析失败: {}", e),
                }
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

        Commands::Holepunch { port, peer_uid, peer_mapped, stun, retry, keepalive } => {
            println!("[*] 命令行UDP打洞 (绕开QQ信令)");
            println!("[*] UDP端口: {}", port);
            println!("[*] 对端标识: {}", peer_uid);

            // 创建节点（无需 NapCat，不查公网IP）：UDP socket + STUN + 事件通道
            let (node, mut event_rx) = P2PNode::new_offline(peer_uid).await?;
            let node = Arc::new(Mutex::new(node));

            // 解析 STUN → 绑定 UDP → 查询映射 → 后台监听打洞报文 + 后台保活(维持映射不超时)
            let stun_addr = crate::holepunch::resolve_stun_server(&stun).await?;
            println!("[*] STUN服务器: {}", stun_addr);
            P2PNode::start_udp_server(
                Arc::clone(&node),
                port,
                stun_addr,
                Duration::from_secs(keepalive),
            )
            .await?;
            if keepalive > 0 {
                println!("[*] 保活已启动(每 {} 秒刷新映射, 等待/重试期间地址长期有效)", keepalive);
            }

            // 打印我方映射地址
            let my_mapped = {
                let n = node.lock().await;
                n.my_mapped
            };
            match my_mapped {
                Some(m) => println!("[+] 我方映射地址: {}", m),
                None => eprintln!("[!] 我方映射地址获取失败"),
            }

            // 对方地址来源：--peer-mapped 作为默认值; 启动后无限期等待 stdin 输入(不输入则一直保活)
            let peer_addr: SocketAddr = loop {
                let hint = match &peer_mapped {
                    Some(pm) => format!("(默认 {}, 直接回车采用)", pm),
                    None => String::from("(留空则继续等待)"),
                };
                println!("[*] 请输入对方映射地址(ip:port)后回车 {}:", hint);
                let mut input = String::new();
                let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
                stdin
                    .read_line(&mut input)
                    .await
                    .context("读取输入失败")?;
                let s = input.trim();
                if s.is_empty() {
                    if let Some(pm) = &peer_mapped {
                        println!("[*] 采用默认对方映射地址: {}", pm);
                        break pm.parse().map_err(|_| {
                            anyhow::anyhow!("对方映射地址格式应为 ip:port, 收到: {}", pm)
                        })?;
                    }
                    println!("[*] 输入为空, 继续等待(保活中, Ctrl+C 退出)...");
                    continue;
                }
                match s.parse::<SocketAddr>() {
                    Ok(addr) => {
                        println!("[*] 对方映射地址: {}", addr);
                        break addr;
                    }
                    Err(_) => {
                        eprintln!("[!] 地址格式应为 ip:port, 收到: {}", s);
                        continue;
                    }
                }
            };
            println!(
                "[*] 开始打洞(每轮约10秒, 未连通自动重试{}轮, Ctrl+C 退出)...",
                if retry == 0 {
                    "∞".to_string()
                } else {
                    format!("上限{}", retry)
                }
            );

            // 后台持续打洞: 0 表示无限重试直到连通(每轮刷新映射, 探测包携带最新地址)
            let punch_node = Arc::clone(&node);
            tokio::spawn(async move {
                crate::holepunch::start_hole_punch_retry(punch_node, peer_uid, peer_addr, retry)
                    .await;
            });

            // 事件循环: 打洞成功→保持监听(等对端也完成); 失败(重试轮数用尽)→退出; Ctrl+C→退出
            loop {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        println!("\n[*] 手动退出");
                        break;
                    }
                    ev = event_rx.recv() => {
                        match ev {
                            Ok(BotEvent::HolePunchResult { detail, success, .. }) => {
                                println!("{}", detail);
                                if success {
                                    println!("[*] 已连通, 保持监听中(Ctrl+C 退出)...");
                                } else {
                                    println!("[!] 打洞失败, 退出");
                                    break;
                                }
                            }
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                }
            }
            Ok(())
        }

        Commands::Mapped { port, stun } => {
            // 绑定 UDP → 查一次 STUN → 打印 ip:port → 直接退出（不做任何打洞动作）
            let stun_addr = crate::holepunch::resolve_stun_server(&stun).await?;
            let sock = tokio::net::UdpSocket::bind(("0.0.0.0", port)).await?;
            let mapped = crate::holepunch::query_mapped_addr_retry(&sock, stun_addr).await?;
            println!("{}", mapped);
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
    }
}
