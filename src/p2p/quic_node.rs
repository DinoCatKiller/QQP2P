//! QUIC 节点：STUN + UDP 打洞 + QUIC 握手
//!
//! 绕开 libp2p 的 QUIC 封装，直接管理 UDP socket：
//! 1. 绑定 UDP socket → STUN 查映射
//! 2. 双方同时 UDP 打洞（互开 NAT 洞）
//! 3. 在打洞后的 socket 上创建 quinn Endpoint
//! 4. 双方同时 dial + accept → QUIC 握手（TLS 1.3 加密）
//! 5. 在 QUIC 连接上交换 HELLO/JOIN_ACK

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use quinn::{ClientConfig, Endpoint, EndpointConfig, ServerConfig};
use rand::RngCore;

use crate::p2p::holepunch::{
    build_binding_request, hole_punch, query_mapped_addr_retry, resolve_stun_server,
};

/// 默认 STUN 服务器
const DEFAULT_STUN: &str = "stun.l.google.com:19302";

// -----------------------------------------------------------
// TLS 自签名证书 + 跳过验证
// -----------------------------------------------------------

/// 跳过服务器证书验证（P2P 自签名证书场景）
#[derive(Debug)]
struct SkipVerification;

impl rustls::client::danger::ServerCertVerifier for SkipVerification {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

/// 生成自签名证书 + 创建 quinn Endpoint
fn create_endpoint(std_sock: std::net::UdpSocket) -> Result<Endpoint> {
    // 1. 生成自签名证书
    let cert = rcgen::generate_simple_self_signed(vec!["p2p".to_string()])?;
    let cert_der = cert.cert.der().clone();
    let key_der = cert.key_pair.serialize_der();

    // 2. Server config（自签名证书）
    let server_crypto = rustls::server::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert_der],
            rustls::pki_types::PrivateKeyDer::Pkcs8(key_der.into()),
        )?;
    let quic_server = quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?;
    let server_config = ServerConfig::with_crypto(Arc::new(quic_server));

    // 3. Client config（跳过证书验证，P2P 场景）
    let client_crypto = rustls::client::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipVerification))
        .with_no_client_auth();
    let quic_client = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)?;
    let client_config = ClientConfig::new(Arc::new(quic_client));

    // 4. 创建 quinn Endpoint（复用已打洞的 socket）
    let runtime = Arc::new(quinn::TokioRuntime);
    let mut endpoint = Endpoint::new(
        EndpointConfig::default(),
        Some(server_config),
        std_sock,
        runtime,
    )?;
    endpoint.set_default_client_config(client_config);

    Ok(endpoint)
}

// -----------------------------------------------------------
// P2P 节点主逻辑
// -----------------------------------------------------------

/// 运行 P2P 节点（STUN + 打洞 + QUIC 握手 + 消息交换）
pub async fn run_p2p_node(port: u16, stun_server: Option<&str>) -> Result<()> {
    let stun = stun_server.unwrap_or(DEFAULT_STUN);

    println!("[*] ═══════════════════════════════════════════");
    println!("[*]  P2P 节点 (STUN + UDP打洞 + QUIC)");
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
            let mut buf = [0u8; 1500];
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                keep_sock.recv_from(&mut buf),
            ).await;
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
    }).await?;

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

    // 7. 把 socket 转回 std，交给 quinn
    let std_sock = tokio_sock.into_std()?;

    // 8. 创建 quinn Endpoint
    println!("[*] 创建 QUIC Endpoint...");
    let endpoint = create_endpoint(std_sock)?;
    println!("[+] QUIC Endpoint 已创建");

    // 9. 双方同时 dial + accept
    println!("[*] 正在建立 QUIC 连接（双方同时 dial + accept）...");

    let dial_endpoint = endpoint.clone();
    let dial_fut = async {
        let conn = dial_endpoint
            .connect(peer_mapped, "p2p")
            .map_err(|e| anyhow::anyhow!("connect: {}", e))?
            .await
            .map_err(|e| anyhow::anyhow!("connection: {}", e))?;
        Ok::<quinn::Connection, anyhow::Error>(conn)
    };

    let accept_endpoint = endpoint.clone();
    let accept_fut = async {
        let incoming = accept_endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("endpoint closed"))?;
        let conn = incoming
            .await
            .map_err(|e| anyhow::anyhow!("accept: {}", e))?;
        Ok::<quinn::Connection, anyhow::Error>(conn)
    };

    let conn = tokio::select! {
        res = dial_fut => {
            println!("[+] 通过 dial 建立连接");
            res?
        }
        res = accept_fut => {
            println!("[+] 通过 accept 收到连接");
            res?
        }
    };

    println!("[+] QUIC 连接已建立!");
    println!("[+] 对端地址: {}", conn.remote_address());
    println!("[+] 加密: TLS 1.3 (QUIC 内置)");
    println!();

    // 10. 在 QUIC 连接上交换 HELLO/JOIN_ACK
    println!("[*] ── 交换 HELLO/JOIN_ACK ──");

    let (mut send, mut recv) = conn.open_bi().await?;

    // 发送 HELLO
    let hello = b"HELLO peer_id=local virtual_ip=10.0.0.1 features=3";
    send.write_all(hello).await?;
    println!("[+] 已发送 HELLO");

    // 接收对方 HELLO
    let mut buf = vec![0u8; 1024];
    if let Some(n) = recv.read(&mut buf).await? {
        println!("[+] 收到: {}", String::from_utf8_lossy(&buf[..n]));
    }

    // 回复 JOIN_ACK
    let ack = b"JOIN_ACK members=2 peer_id=local virtual_ip=10.0.0.1 peer_id=remote virtual_ip=10.0.0.1";
    send.write_all(ack).await?;
    println!("[+] 已回复 JOIN_ACK");

    // 接收对方 JOIN_ACK
    if let Some(n) = recv.read(&mut buf).await? {
        println!("[+] 收到: {}", String::from_utf8_lossy(&buf[..n]));
    }

    println!();
    println!("[*] ═══════════════════════════════════════════");
    println!("[*]  N1 验收：QUIC 连接 + 消息互通 + 加密");
    println!("[*] ═══════════════════════════════════════════");
    println!("[*] 按 Ctrl+C 退出");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
