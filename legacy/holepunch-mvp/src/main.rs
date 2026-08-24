//! holepunch-mvp：命令行 UDP NAT 穿越打洞联调工具
//!
//! 归档自 QQP2P 主工程（2026-08-24）。MVP（双方 UDP 打洞）为可行性验证产物，
//! 正式项目已按 `docs/plan/P2P_INFRA_PLAN.md` 转向 libp2p 方案。
//! 本工程仅保留作为"上帝视角"打洞联调工具，只维护不演进。

mod holepunch;
mod p2p;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::io::AsyncBufReadExt;
use tokio::sync::Mutex;

use crate::p2p::{BotEvent, P2PNode};

#[derive(Parser, Debug)]
#[command(name = "holepunch-mvp")]
#[command(about = "UDP NAT 穿越打洞联调工具(MVP 归档)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 命令行UDP打洞（上帝视角联调，不依赖 QQ 信令）
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Holepunch {
            port,
            peer_uid,
            peer_mapped,
            stun,
            retry,
            keepalive,
        } => {
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
    }
}
