//! Noise 节点：STUN + UDP 打洞 + Noise 握手
//!
//! 绕开 libp2p 的 QUIC 封装，直接管理 UDP socket：
//! 1. 绑定 UDP socket → STUN 查映射
//! 2. 双方同时 UDP 打洞（互开 NAT 洞）
//! 3. 在打洞后的 socket 上做 Noise_XX 握手（ChaCha20-Poly1305 加密）
//! 4. 在加密通道上交换 HELLO/JOIN_ACK

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rand::RngCore;
use snow::Builder as NoiseBuilder;
use tokio::net::UdpSocket;

use crate::p2p::holepunch::{
    build_binding_request, hole_punch, query_mapped_addr_retry, resolve_stun_server,
};

/// 默认 STUN 服务器
const DEFAULT_STUN: &str = "stun.l.google.com:19302";

/// Noise 协议参数：XX 模式 + Curve25519 + ChaChaPoly + BLAKE2s
const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Noise 握手单步超时
const NOISE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// UDP 收发缓冲区大小：用足 UDP 理论上限，避免 Windows WSAEMSGSIZE (os error 10040)
/// —— 应用提供的 recv 缓冲区小于传入数据报时，Windows 会直接丢弃并返回 10040
const UDP_RECV_BUF: usize = 65535;

// -----------------------------------------------------------
// P2P 节点主逻辑
// -----------------------------------------------------------

/// 运行 P2P 节点（STUN + 打洞 + Noise 握手 + 消息交换）
pub async fn run_p2p_node(port: u16, stun_server: Option<&str>) -> Result<()> {
    let stun = stun_server.unwrap_or(DEFAULT_STUN);

    println!("[*] ═══════════════════════════════════════════");
    println!("[*]  P2P 节点 (STUN + UDP打洞 + Noise)");
    println!("[*] ═══════════════════════════════════════════");
    println!("[*] 监听端口: {}", port);
    println!();

    // 1. 创建 UDP socket
    let std_sock = std::net::UdpSocket::bind(format!("0.0.0.0:{}", port))?;
    println!("[*] UDP socket 绑定: 0.0.0.0:{}", port);

    // 必须设为非阻塞，否则 tokio::net::UdpSocket::from_std 在 Linux/Android 上会 panic
    std_sock.set_nonblocking(true)?;

    let tokio_sock = tokio::net::UdpSocket::from_std(std_sock)?;

    // 2. STUN 查映射
    let stun_addr = resolve_stun_server(stun).await?;
    println!("[*] STUN 服务器: {}", stun_addr);

    // 包成 Arc 便于 keep-alive 后台任务共享
    let tokio_sock = Arc::new(tokio_sock);

    let my_mapped = query_mapped_addr_retry(&tokio_sock, stun_addr).await?;
    println!("[*] 本机映射地址: {}", my_mapped);
    println!("[*] 虚拟IP: 10.0.0.1");
    println!();

    // 3. 启动 keep-alive 后台任务: 每 10s 发 STUN 包刷新 NAT 映射
    //    避免用户输入慢导致 NAT 映射过期
    println!("[*] [保活] 等待输入期间每 10s 发 STUN 包保持 NAT 映射");
    let keep_sock = Arc::clone(&tokio_sock);
    let keep_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.tick().await; // 跳过首次立即触发
        loop {
            interval.tick().await;
            // 发 STUN 包保活 (同时也能检测 NAT 是否还活着)
            let mut tx_id = [0u8; 12];
            rand::thread_rng().fill_bytes(&mut tx_id);
            let req = build_binding_request(tx_id);
            if keep_sock.send_to(&req, stun_addr).await.is_err() {
                continue;
            }
            // 收响应避免 socket buf 堆积, 500ms 超时即可
            let mut buf = [0u8; UDP_RECV_BUF];
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                keep_sock.recv_from(&mut buf),
            )
            .await;
        }
    });

    // 4. 等待用户输入对方地址 (用 spawn_blocking 避免 tokio runtime 阻塞)
    println!("[*] ── 操作说明 ──");
    println!("[*] 1. 把上面的「映射地址」发给对方");
    println!("[*] 2. 输入对方给你的映射地址并回车");
    println!("[*] 3. 双方都输入后同时开始打洞");
    println!();
    print!("[*] 请输入对方映射地址 (ip:port): ");
    use std::io::Write;
    std::io::stdout().flush().ok();

    let input = tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        s
    })
    .await?;

    // 5. 取消 keep-alive, 拆出 Arc 取回原始 socket 用于打洞
    keep_handle.abort();
    let _ = keep_handle.await; // 等 task 真正退出, Arc 才能被 try_unwrap
    let tokio_sock = Arc::try_unwrap(tokio_sock)
        .map_err(|_| anyhow::anyhow!("keep-alive task 未释放 socket, 内部错误"))?;

    let input = input.trim();
    if input.is_empty() {
        anyhow::bail!("未输入对方地址");
    }
    let peer_mapped: SocketAddr = input.parse()?;

    // 6. 双方同时 UDP 打洞
    println!();
    println!("[*] ── 开始 UDP 打洞 ──");
    hole_punch(&tokio_sock, peer_mapped, 1, 30).await?;
    println!("[+] 打洞成功! NAT 洞已打开");
    println!();

    // 7. 清空打洞阶段残留的 UDP 包（避免干扰 Noise 握手）
    //    注意：Windows 上若残留包 > recv buf，recv_from 返回 Err(WSAEMSGSIZE) 且 OS 已丢弃该包。
    //    旧逻辑 `while let Ok(Ok(_))` 遇到这种 Err 会立即退出循环，残留包没真正清干净。
    //    正确做法：无论 Ok/Err 都继续，只在超时（无包可读）时退出。
    println!("[*] 清空打洞残留包...");
    let mut drain_buf = [0u8; UDP_RECV_BUF];
    loop {
        match tokio::time::timeout(Duration::from_millis(200), tokio_sock.recv_from(&mut drain_buf)).await {
            Ok(Ok(_)) => continue,   // 已消费一个残留包
            Ok(Err(_)) => continue,  // 如 WSAEMSGSIZE: 包已被 OS 丢弃，继续清下一个
            Err(_) => break,         // 超时 → 队列已清空
        }
    }
    println!("[+] 残留包已清空");
    println!();

    // 8. Noise_XX 握手
    println!("[*] ── Noise 握手 ──");

    // 生成静态密钥对（X25519）
    let builder = NoiseBuilder::new(NOISE_PATTERN.parse()?);
    let keypair = builder.generate_keypair()?;
    println!("[*] 本地公钥: {}", hex_encode(&keypair.public));

    // 确定角色：映射地址较小的一方为 Initiator（避免双方同时发送导致冲突）
    // 极端情况下地址相同（不可能），双方都是 Responder 会导致死锁
    let is_initiator = my_mapped < peer_mapped;
    println!(
        "[*] 角色: {}",
        if is_initiator { "Initiator" } else { "Responder" }
    );

    let mut transport =
        run_noise_handshake(&tokio_sock, peer_mapped, is_initiator, &keypair.private).await?;

    println!("[+] Noise 握手完成!");
    println!("[+] 加密: Noise Protocol (XX 模式, ChaCha20-Poly1305, Curve25519)");
    println!();

    // 9. 在加密通道上交换 HELLO/JOIN_ACK
    println!("[*] ── 交换 HELLO/JOIN_ACK ──");

    // 发送 HELLO + JOIN_ACK（用换行分隔，一次加密发送）
    let payload = "HELLO peer_id=local virtual_ip=10.0.0.1 features=3\n\
                   JOIN_ACK members=2 peer_id=local virtual_ip=10.0.0.1 peer_id=remote virtual_ip=10.0.0.1";
    let mut send_buf = vec![0u8; UDP_RECV_BUF];
    let send_len = transport.write_message(payload.as_bytes(), &mut send_buf)?;
    tokio_sock.send_to(&send_buf[..send_len], peer_mapped).await?;
    println!("[+] 已发送 HELLO + JOIN_ACK (加密)");

    // 接收对方的 HELLO + JOIN_ACK
    let mut recv_buf = vec![0u8; UDP_RECV_BUF];
    let (n, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_sock.recv_from(&mut recv_buf),
    )
    .await
    .map_err(|_| anyhow::anyhow!("等待对方消息超时"))?
    .context("recv_from 失败")?;

    let mut pt = vec![0u8; UDP_RECV_BUF];
    let pt_len = transport.read_message(&recv_buf[..n], &mut pt)?;
    let received = String::from_utf8_lossy(&pt[..pt_len]);
    for line in received.lines() {
        println!("[+] 收到: {}", line);
    }

    println!();
    println!("[*] ═══════════════════════════════════════════");
    println!("[*]  N1 验收：Noise 连接 + 消息互通 + 加密");
    println!("[*] ═══════════════════════════════════════════");
    println!("[*] 按 Ctrl+C 退出");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

// -----------------------------------------------------------
// Noise_XX 握手
// -----------------------------------------------------------

/// 执行 Noise_XX 握手，返回传输模式状态
///
/// 双方根据映射地址大小确定角色：
/// - Initiator（地址较小）: send → recv → send
/// - Responder（地址较大）: recv → send → recv
///
/// 握手完成后进入传输模式，可加密/解密消息
async fn run_noise_handshake(
    sock: &UdpSocket,
    peer: SocketAddr,
    is_initiator: bool,
    private_key: &[u8],
) -> Result<snow::TransportState> {
    let builder = NoiseBuilder::new(NOISE_PATTERN.parse()?).local_private_key(private_key)?;

    let mut hs = if is_initiator {
        builder.build_initiator()?
    } else {
        builder.build_responder()?
    };

    let mut buf = vec![0u8; UDP_RECV_BUF];
    let mut recv_buf = vec![0u8; UDP_RECV_BUF];
    let mut payload = vec![0u8; UDP_RECV_BUF];

    if is_initiator {
        // Initiator: send → recv → send
        // msg 1: -> e
        let len = hs.write_message(&[], &mut buf)?;
        sock.send_to(&buf[..len], peer).await?;
        println!("[*] [noise] 已发送 e ({} 字节)", len);

        // msg 2: <- e, ee, s, es
        let (n, _) = tokio::time::timeout(NOISE_HANDSHAKE_TIMEOUT, sock.recv_from(&mut recv_buf))
            .await
            .map_err(|_| anyhow::anyhow!("Noise 握手超时: 等待 msg 2"))?
            .context("recv_from 失败")?;
        hs.read_message(&recv_buf[..n], &mut payload)?;
        if let Some(remote) = hs.get_remote_static() {
            println!("[*] [noise] 对端公钥: {}", hex_encode(remote));
        }

        // msg 3: -> s, se
        let len = hs.write_message(&[], &mut buf)?;
        sock.send_to(&buf[..len], peer).await?;
        println!("[*] [noise] 已发送 s ({} 字节)", len);
    } else {
        // Responder: recv → send → recv
        // msg 1: <- e
        let (n, _) = tokio::time::timeout(NOISE_HANDSHAKE_TIMEOUT, sock.recv_from(&mut recv_buf))
            .await
            .map_err(|_| anyhow::anyhow!("Noise 握手超时: 等待 msg 1"))?
            .context("recv_from 失败")?;
        hs.read_message(&recv_buf[..n], &mut payload)?;

        // msg 2: -> e, ee, s, es
        let len = hs.write_message(&[], &mut buf)?;
        sock.send_to(&buf[..len], peer).await?;
        println!("[*] [noise] 已发送 e + s ({} 字节)", len);

        // msg 3: <- s, se
        let (n, _) = tokio::time::timeout(NOISE_HANDSHAKE_TIMEOUT, sock.recv_from(&mut recv_buf))
            .await
            .map_err(|_| anyhow::anyhow!("Noise 握手超时: 等待 msg 3"))?
            .context("recv_from 失败")?;
        hs.read_message(&recv_buf[..n], &mut payload)?;
        if let Some(remote) = hs.get_remote_static() {
            println!("[*] [noise] 对端公钥: {}", hex_encode(remote));
        }
    }

    Ok(hs.into_transport_mode()?)
}

// -----------------------------------------------------------
// 辅助函数
// -----------------------------------------------------------

/// 十六进制编码（用于打印公钥）
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
