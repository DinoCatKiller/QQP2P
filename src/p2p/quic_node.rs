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
    build_binding_request, detect_nat_type, hole_punch, query_mapped_addr_retry,
    resolve_stun_server, NatType,
};

/// 默认 STUN 服务器
const DEFAULT_STUN: &str = "stun.l.google.com:19302";

/// 备用 STUN 服务器 (NAT 类型探测用, 必须与主 STUN 不同)
const SECONDARY_STUN: &str = "stun1.l.google.com:19302";

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
        recv_noise_msg(sock, &mut hs, &mut recv_buf, &mut payload, "msg 2").await?;
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
        recv_noise_msg(sock, &mut hs, &mut recv_buf, &mut payload, "msg 1").await?;

        // msg 2: -> e, ee, s, es
        let len = hs.write_message(&[], &mut buf)?;
        sock.send_to(&buf[..len], peer).await?;
        println!("[*] [noise] 已发送 e + s ({} 字节)", len);

        // msg 3: <- s, se
        recv_noise_msg(sock, &mut hs, &mut recv_buf, &mut payload, "msg 3").await?;
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

/// Unix 秒 → "YYYYMMDD_HHMMSS" (本地时区, 用 Windows API / Unix localtime)
/// 为避免引入 chrono 依赖, 用简单算法算 UTC+8
fn format_timestamp(unix_secs: u64) -> String {
    // 简化: 用 UTC+8 (北京时间) 算本地时间, 避免引入 chrono
    let secs = unix_secs + 8 * 3600;
    let days = secs / 86400;
    let remainder = secs % 86400;
    let hour = remainder / 3600;
    let min = (remainder % 3600) / 60;
    let sec = remainder % 60;

    // 从 1970-01-01 算日期
    let mut y = 1970u32;
    let mut d = days as u32;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
        let yd = if leap { 366 } else { 365 };
        if d < yd { break; }
        d -= yd;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0u32;
    for &dm in &mdays {
        if d < dm { break; }
        d -= dm;
        m += 1;
    }
    m += 1;
    d += 1;

    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", y, m, d, hour, min, sec)
}

/// Noise 握手 recv: 循环 recv 直到 read_message 成功或超时。
///
/// 必要性: 打洞结束后对方可能仍在 interval tick 发 HOLEPUNCH 探测包,
/// drain 的 200ms 窗口漏掉的包会进入 Noise recv, 被当 msg 解析 -> input error。
/// read_message 失败即跳过该包继续等, 自动过滤 HOLEPUNCH/STUN 残留。
async fn recv_noise_msg(
    sock: &UdpSocket,
    hs: &mut snow::HandshakeState,
    recv_buf: &mut [u8],
    payload: &mut [u8],
    label: &str,
) -> Result<()> {
    let deadline = tokio::time::sleep(NOISE_HANDSHAKE_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => anyhow::bail!("Noise 握手超时: 等待 {}", label),
            res = sock.recv_from(recv_buf) => {
                let (n, src) = res.context("recv_from 失败")?;
                if n < 16 {
                    println!("[*] [noise] 跳过短包 ({} 字节) from {}", n, src);
                    continue;
                }
                match hs.read_message(&recv_buf[..n], payload) {
                    Ok(_) => return Ok(()),
                    Err(e) => {
                        println!("[*] [noise] 跳过非 Noise 包 ({} 字节) from {}: {}", n, src, e);
                        continue;
                    }
                }
            }
        }
    }
}

// -----------------------------------------------------------
// P2P 打洞实测 (p2p-bench 命令)
// -----------------------------------------------------------

/// 单轮测试结果
struct BenchRound {
    success: bool,
    elapsed: Duration,
    /// 失败阶段: "打洞" / "Noise握手" / "消息交换" / None(成功)
    fail_stage: Option<&'static str>,
    fail_reason: Option<String>,
}

impl BenchRound {
    fn ok(elapsed: Duration) -> Self {
        Self { success: true, elapsed, fail_stage: None, fail_reason: None }
    }
    fn fail(elapsed: Duration, stage: &'static str, reason: String) -> Self {
        Self { success: false, elapsed, fail_stage: Some(stage), fail_reason: Some(reason) }
    }
}

/// 运行 P2P 打洞实测
///
/// 流程:
/// 1. 绑定 socket + NAT 类型探测 (查两次 STUN 比对端口)
/// 2. 用户输入对方映射地址
/// 3. 循环 N 轮, 每轮:
///    打洞 → drain → Noise 握手 → HELLO/JOIN_ACK 交换
///    口径3: 三步全过才算成功
/// 4. 打印统计表 + 写 md 报告
pub async fn run_p2p_bench(
    port: u16,
    stun_server: Option<&str>,
    rounds: u32,
    interval_secs: u64,
    hole_timeout_secs: u64,
) -> Result<()> {
    let stun = stun_server.unwrap_or(DEFAULT_STUN);
    // 时间戳用 std 生成 (避免引入 chrono 依赖)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let ts = format_timestamp(now);

    println!("[*] ═══════════════════════════════════════════");
    println!("[*]  P2P 打洞实测 ({} 轮)", rounds);
    println!("[*] ═══════════════════════════════════════════");
    println!("[*] 监听端口: {}", port);
    println!("[*] STUN 服务器: {}", stun);
    println!("[*] 单轮打洞超时: {}s", hole_timeout_secs);
    println!("[*] 轮间间隔: {}s", interval_secs);
    println!();

    // 1. 创建 socket
    let std_sock = std::net::UdpSocket::bind(format!("0.0.0.0:{}", port))?;
    std_sock.set_nonblocking(true)?;
    let tokio_sock = tokio::net::UdpSocket::from_std(std_sock)?;

    let stun_addr = resolve_stun_server(stun).await?;
    let stun_secondary_addr = resolve_stun_server(SECONDARY_STUN).await?;

    // 2. NAT 类型探测
    println!("[*] ── NAT 类型探测 ──");
    let nat_type = detect_nat_type(&tokio_sock, stun_addr, stun_secondary_addr).await;
    let (my_mapped, nat_label) = match &nat_type {
        NatType::Cone { mapped } => {
            println!("[*] 主 STUN  → {}", mapped);
            println!("[*] 备 STUN  → (端口一致)");
            println!("[*] NAT 类型: {}", nat_type.label());
            (*mapped, "Cone")
        }
        NatType::Symmetric { mapped1, mapped2 } => {
            println!("[*] 主 STUN  → {} (端口 {})", mapped1, mapped1.port());
            println!("[*] 备 STUN  → {} (端口 {})", mapped2, mapped2.port());
            println!("[*] NAT 类型: {}", nat_type.label());
            println!("[!] 警告: 本机是 Symmetric NAT, 纯 UDP 打洞大概率失败");
            (*mapped1, "Symmetric")
        }
        NatType::Unknown { reason } => {
            println!("[!] NAT 探测失败: {}", reason);
            anyhow::bail!("NAT 探测失败, 无法继续");
        }
    };
    println!("[*] 本机映射地址: {}", my_mapped);
    println!();

    // 3. 输入对方地址
    println!("[*] ── 操作说明 ──");
    println!("[*] 1. 把上面的「映射地址」发给对方");
    println!("[*] 2. 输入对方给你的映射地址并回车");
    println!("[*] 3. 双方都输入后同时开始 {} 轮测试", rounds);
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

    let input = input.trim();
    if input.is_empty() {
        anyhow::bail!("未输入对方地址");
    }
    let peer_mapped: SocketAddr = input.parse()?;
    println!();
    println!("[*] 对端: {}", peer_mapped);
    println!("[*] 开始 {} 轮测试", rounds);
    println!();

    // 4. 循环测试
    let mut results: Vec<BenchRound> = Vec::with_capacity(rounds as usize);
    println!("轮次 | 结果 | 耗时  | 失败阶段");
    println!("-----|------|-------|----------------");

    for round in 1..=rounds {
        let t0 = std::time::Instant::now();
        let result = run_one_bench_round(
            port, my_mapped, peer_mapped, stun_addr, hole_timeout_secs,
        )
        .await;
        let elapsed = t0.elapsed();

        let r = match result {
            Ok(()) => BenchRound::ok(elapsed),
            Err((stage, e)) => BenchRound::fail(elapsed, stage, e.to_string()),
        };

        // 打印该轮结果
        let mark = if r.success { "✓" } else { "✗" };
        let stage = r.fail_stage.unwrap_or("-");
        let reason = r.fail_reason.as_deref().unwrap_or("");
        println!(
            "  {:>2} | {}    | {:>5.1}s | {} {}",
            round, mark, elapsed.as_secs_f64(), stage,
            if reason.is_empty() { String::new() } else { format!("({})", reason) }
        );

        results.push(r);

        // 轮间间隔 (最后一轮不用等)
        if round < rounds {
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
        }
    }

    // 5. 统计
    let success_count = results.iter().filter(|r| r.success).count();
    let fail_count = results.len() - success_count;
    let success_rate = success_count as f64 / results.len() as f64 * 100.0;
    let avg_success_elapsed: f64 = {
        let successes: Vec<f64> = results.iter()
            .filter(|r| r.success)
            .map(|r| r.elapsed.as_secs_f64())
            .collect();
        if successes.is_empty() { 0.0 } else { successes.iter().sum::<f64>() / successes.len() as f64 }
    };

    // 失败阶段分布
    let mut fail_stages: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for r in &results {
        if let Some(stage) = r.fail_stage {
            *fail_stages.entry(stage).or_insert(0) += 1;
        }
    }

    println!();
    println!("═══════════════════════════════════════════════");
    println!(" 实测统计");
    println!("═══════════════════════════════════════════════");
    println!("总轮数:      {}", results.len());
    println!("成功:         {}", success_count);
    println!("失败:         {}", fail_count);
    println!("通过率:    {:.1}%", success_rate);
    println!("成功平均耗时: {:.2}s", avg_success_elapsed);
    println!("本机 NAT:  {}", nat_label);
    if !fail_stages.is_empty() {
        println!("失败阶段分布:");
        for (stage, count) in &fail_stages {
            println!("  - {}: {} 次", stage, count);
        }
    }
    println!();

    // 6. 写 md 报告
    let report_name = format!("bench_report_{}.md", ts);
    let report_path = std::path::Path::new(&report_name);
    let report = format_bench_report(
        &ts,
        my_mapped, peer_mapped, nat_label,
        rounds, &results,
        success_rate, avg_success_elapsed, &fail_stages,
    );
    std::fs::write(&report_path, report)?;
    println!("[+] 报告已写入: {}", report_path.display());

    Ok(())
}

/// 跑一轮完整测试: 打洞 → drain → Noise 握手 → HELLO 交换
///
/// 注意: 每轮用新 socket, 因为旧 socket 的 NAT 映射可能已过期/被回收,
///       新 socket 会触发 NAT 重新分配端口 (更真实的模拟)
async fn run_one_bench_round(
    port: u16,
    _my_mapped: SocketAddr,
    peer_mapped: SocketAddr,
    stun_addr: SocketAddr,
    hole_timeout: u64,
) -> std::result::Result<(), (&'static str, anyhow::Error)> {
    // 新 socket (模拟真实场景: 每次连接都是新的)
    let std_sock = std::net::UdpSocket::bind(format!("0.0.0.0:{}", port))
        .map_err(|e| ("打洞", anyhow::anyhow!("bind 失败: {}", e)))?;
    std_sock.set_nonblocking(true)
        .map_err(|e| ("打洞", anyhow::anyhow!("set_nonblocking 失败: {}", e)))?;
    let sock = tokio::net::UdpSocket::from_std(std_sock)
        .map_err(|e| ("打洞", anyhow::anyhow!("from_std 失败: {}", e)))?;

    // 重新查本机映射 (NAT 可能给了新端口)
    let my_mapped = query_mapped_addr_retry(&sock, stun_addr).await
        .map_err(|e| ("打洞", anyhow::anyhow!("STUN 查询失败: {}", e)))?;

    // === 阶段1: 打洞 ===
    hole_punch(&sock, peer_mapped, 1, hole_timeout).await
        .map_err(|e| ("打洞", e))?;

    // === drain 残留包 ===
    let mut drain_buf = [0u8; UDP_RECV_BUF];
    loop {
        match tokio::time::timeout(Duration::from_millis(200), sock.recv_from(&mut drain_buf)).await {
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => continue,
            Err(_) => break,
        }
    }

    // === 阶段2: Noise 握手 ===
    let builder = NoiseBuilder::new(NOISE_PATTERN.parse().unwrap());
    let keypair = builder.generate_keypair()
        .map_err(|e| ("Noise握手", anyhow::anyhow!("生成密钥失败: {}", e)))?;
    let is_initiator = my_mapped < peer_mapped;
    let mut transport = run_noise_handshake(&sock, peer_mapped, is_initiator, &keypair.private).await
        .map_err(|e| ("Noise握手", e))?;

    // === 阶段3: 消息交换 (HELLO + JOIN_ACK) ===
    let payload = "HELLO peer_id=local virtual_ip=10.0.0.1 features=3\n\
                   JOIN_ACK members=2 peer_id=local virtual_ip=10.0.0.1 peer_id=remote virtual_ip=10.0.0.1";
    let mut send_buf = vec![0u8; UDP_RECV_BUF];
    let send_len = transport.write_message(payload.as_bytes(), &mut send_buf)
        .map_err(|e| ("消息交换", anyhow::anyhow!("加密失败: {}", e)))?;
    sock.send_to(&send_buf[..send_len], peer_mapped).await
        .map_err(|e| ("消息交换", anyhow::anyhow!("send_to 失败: {}", e)))?;

    let mut recv_buf = vec![0u8; UDP_RECV_BUF];
    let (n, _) = tokio::time::timeout(Duration::from_secs(10), sock.recv_from(&mut recv_buf))
        .await
        .map_err(|_| ("消息交换", anyhow::anyhow!("等待对方消息超时")))?
        .map_err(|e| ("消息交换", anyhow::anyhow!("recv_from 失败: {}", e)))?;

    let mut pt = vec![0u8; UDP_RECV_BUF];
    let pt_len = transport.read_message(&recv_buf[..n], &mut pt)
        .map_err(|e| ("消息交换", anyhow::anyhow!("解密失败: {}", e)))?;

    // 验证收到的确实是 HELLO
    let received = String::from_utf8_lossy(&pt[..pt_len]);
    if !received.contains("HELLO") {
        return Err(("消息交换", anyhow::anyhow!("收到的消息不是 HELLO: {}", received)));
    }

    Ok(())
}

/// 格式化 md 报告 (对应 N1.5 实测记录)
fn format_bench_report(
    ts: &str,
    my_mapped: SocketAddr,
    peer_mapped: SocketAddr,
    nat_label: &str,
    rounds: u32,
    results: &[BenchRound],
    success_rate: f64,
    avg_success_elapsed: f64,
    fail_stages: &std::collections::HashMap<&str, u32>,
) -> String {
    let success_count = results.iter().filter(|r| r.success).count();
    let fail_count = results.len() - success_count;

    let mut md = String::new();
    md.push_str("# P2P 打洞实测报告\n\n");
    md.push_str(&format!("- 时间: {}\n", ts));
    md.push_str(&format!("- 本机: {} ({})\n", my_mapped, nat_label));
    md.push_str(&format!("- 对端: {}\n", peer_mapped));
    md.push_str("- 场景: 宽带 ↔ 手机热点 (或其它, 请手动标注)\n\n");

    md.push_str("## 结果\n\n");
    md.push_str(&format!("- 总轮数: {}\n", rounds));
    md.push_str(&format!("- 成功: {}\n", success_count));
    md.push_str(&format!("- 失败: {}\n", fail_count));
    md.push_str(&format!("- 通过率: {:.1}%\n", success_rate));
    md.push_str(&format!("- 成功平均耗时: {:.2}s\n", avg_success_elapsed));
    md.push_str(&format!("- 本机 NAT: {}\n", nat_label));

    if !fail_stages.is_empty() {
        md.push_str("\n## 失败阶段分布\n\n");
        for (stage, count) in fail_stages {
            md.push_str(&format!("- {}: {} 次\n", stage, count));
        }
    }

    md.push_str("\n## 逐轮明细\n\n");
    md.push_str("| 轮次 | 结果 | 耗时(s) | 失败阶段 | 失败原因 |\n");
    md.push_str("|------|------|---------|----------|----------|\n");
    for (i, r) in results.iter().enumerate() {
        let mark = if r.success { "✓" } else { "✗" };
        let stage = r.fail_stage.unwrap_or("-");
        let reason = r.fail_reason.as_deref().unwrap_or("");
        md.push_str(&format!(
            "| {} | {} | {:.2} | {} | {} |\n",
            i + 1, mark, r.elapsed.as_secs_f64(), stage, reason
        ));
    }

    md.push_str("\n## 结论\n\n");
    md.push_str("<!-- 请手动填写 -->\n");
    md.push_str(&format!("- 通过率 {}% ", success_rate));
    if success_rate >= 100.0 {
        md.push_str("≥ 100% 阈值, 直连方案达标\n");
        md.push_str("- 建议: 可推进 N2 编排层\n");
    } else if success_rate >= 50.0 {
        md.push_str("部分达标, 存在失败场景\n");
        md.push_str("- 建议: 评估失败原因, 考虑 Symmetric NAT 端口预测 或 N4 中继兜底\n");
    } else {
        md.push_str("未达标\n");
        md.push_str("- 建议: 启动 N4 中继兜底, 或更换网络方案\n");
    }

    md
}
