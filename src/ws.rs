//! WebSocket 层：连接 NapCat 的 OneBot v11 WS 通道，
//! 监听 QQ 消息事件 → 过滤（@机器人 / P2P 协议消息）→ 转成内部 `BotEvent`。

use anyhow::Result;
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use futures_util::StreamExt;

use crate::app::BotApp;
use crate::napcat::NapCatConfig;
use crate::p2p::BotEvent;

/// NapCat WebSocket 推送的事件（只取本项目关心的字段）
#[derive(Debug, Deserialize)]
pub struct NapCatEvent {
    #[serde(rename = "post_type")]
    post_type: String,
    #[serde(rename = "message_type")]
    message_type: Option<String>,
    /// serde 反序列化字段（保留供消息细分类型判断）
    #[allow(dead_code)]
    #[serde(rename = "sub_type")]
    sub_type: Option<String>,
    #[serde(rename = "user_id")]
    user_id: u64,
    #[serde(rename = "group_id")]
    group_id: Option<u64>,
    /// 原始消息全文（保留，供后续协议解析使用）
    #[allow(dead_code)]
    #[serde(rename = "raw_message")]
    raw_message: Option<String>,
    #[serde(rename = "message")]
    message: Option<serde_json::Value>,
}

/// 从 `event.message` 中提取纯文本内容（兼容字符串与分段数组两种格式）
fn extract_message_text(value: &Option<serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(segs)) => {
            segs.iter()
                .filter_map(|seg| {
                    if seg.get("type").and_then(|t| t.as_str()) == Some("text") {
                        seg.pointer("/data/text").and_then(|t| t.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        }
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// 判断消息是否 @ 了机器人: at 段的 QQ 号或昵称与登录账号匹配, 或纯文本 "@昵称"
fn is_mentioned_bot(event: &NapCatEvent, my_user_id: u64, my_name: &str) -> bool {
    let Some(message) = &event.message else {
        return false;
    };
    let serde_json::Value::Array(segs) = message else {
        return false;
    };

    let my_id_str = my_user_id.to_string();
    for seg in segs {
        let seg_type = seg.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match seg_type {
            "at" => {
                // 按 @ 的 QQ 号匹配(最可靠)
                if let Some(qq) = seg.pointer("/data/qq").and_then(|v| v.as_str()) {
                    if qq == my_id_str {
                        return true;
                    }
                }
                // 按 @ 段携带的昵称匹配
                if !my_name.is_empty() {
                    if let Some(name) = seg.pointer("/data/name").and_then(|v| v.as_str()) {
                        if name == my_name {
                            return true;
                        }
                    }
                }
            }
            "text"
                // 纯文本 "@昵称" 形式(私聊或客户端不产生 at 段时)
                if !my_name.is_empty() => {
                    if let Some(text) = seg.pointer("/data/text").and_then(|v| v.as_str()) {
                        if text.contains(&format!("@{my_name}")) {
                            return true;
                        }
                    }
                }
            _ => {}
        }
    }
    false
}

/// P2P 协议握手消息特征: 含 "P2P节点信息", 或同时含 "公网IP" 与 "端口"
/// 这类消息是双方机器人互发节点信息的握手协议, 允许不 @ 也能自动处理
fn is_protocol_message(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("p2p节点信息")
        || (lower.contains("公网ip") && lower.contains("端口"))
}

/// 消费 `BotEvent`：调用消息处理器，并把回复发回 QQ
pub async fn run_message_handler(app: BotApp, mut event_rx: tokio::sync::broadcast::Receiver<BotEvent>) -> Result<()> {
    loop {
        match event_rx.recv().await {
            Ok(BotEvent::PrivateMessage { user_id, message }) => {
                if user_id == app.my_user_id {
                    continue;
                }
                println!("[*] 收到私聊消息: {} - {}", user_id, message);

                if let Some(reply) = app.handle_message(user_id, &message).await {
                    if let Err(e) = app.send_reply(user_id, &reply).await {
                        eprintln!("[!] 发送私聊消息失败: {}", e);
                    }
                }
            }
            Ok(BotEvent::GroupMessage { user_id, group_id, message, raw_message: _ }) => {
                if user_id == app.my_user_id {
                    continue;
                }
                println!("[*] 收到群消息: {} - {}", user_id, message);

                if let Some(reply) = app.handle_message(user_id, &message).await {
                    if let Err(e) = app.send_group_reply(group_id, &reply).await {
                        eprintln!("[!] 发送群消息失败: {}", e);
                    }
                }
            }
            Ok(BotEvent::Connected { user_id }) => {
                println!("[+] {} 已建立P2P连接", user_id);
            }
            Ok(BotEvent::Disconnected { user_id }) => {
                println!("[!] {} 已断开P2P连接", user_id);
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                println!("[*] 丢失 {} 个事件", n);
            }
            Err(e) => {
                eprintln!("[!] 事件接收错误: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }
    }
}

/// 连接 NapCat WS 并监听消息（断线自动重连）
pub async fn websocket_listener(app: BotApp) -> Result<()> {
    let config = {
        let napcat = app.napcat.lock().await;
        napcat.config.clone()
    };

    let ws_url = format!("ws://{}:{}/onebot/v11/ws", config.ws_host, config.ws_port);
    println!("[*] 正在连接WebSocket: {}", ws_url);

    loop {
        let ws = match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((ws, _)) => ws,
            Err(e) => {
                eprintln!("[!] WebSocket连接失败: {}, 5秒后重试...", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
        };
        println!("[+] WebSocket已连接");

        let mut ws_read = ws;

        loop {
            tokio::select! {
                msg = ws_read.next() => {
                    match msg {
                        Some(Ok(WsMessage::Text(text))) => {
                            println!("[WS] 收到文本帧: {}", text);
                            match serde_json::from_str::<NapCatEvent>(&text) {
                                Ok(event) => {
                                    println!("[WS] 解析成功 post_type={}", event.post_type);
                                    if event.post_type == "message" {
                                        let message_type = event.message_type.as_deref().unwrap_or("");
                                        let user_id = event.user_id;
                                        let msg = extract_message_text(&event.message);

                                        // 只有当 @机器人(昵称取自登录账号信息) 时才触发回信
                                        // P2P 协议握手消息(含对方节点信息)除外: 允许同款机器人互发节点信息
                                        if !is_mentioned_bot(&event, app.my_user_id, &app.my_name) && !is_protocol_message(&msg) {
                                            println!("[*] 忽略非@消息({}): {} - {}", message_type, user_id, msg);
                                            continue;
                                        }

                                        if message_type == "private" {
                                            let _ = app.send_event(BotEvent::PrivateMessage { user_id, message: msg.clone() }).await;
                                        } else if message_type == "group" {
                                            if let Some(group_id) = event.group_id {
                                                let _ = app.send_event(BotEvent::GroupMessage { user_id, group_id, message: msg.clone(), raw_message: msg }).await;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    println!("[WS] 解析失败: {}", e);
                                }
                            }
                        }
                        Some(Ok(WsMessage::Ping(_))) => {
                            println!("[WS] 收到Ping帧");
                        }
                        Some(Ok(WsMessage::Pong(_))) => {
                            println!("[WS] 收到Pong帧");
                        }
                        Some(Ok(WsMessage::Binary(b))) => {
                            println!("[WS] 收到二进制帧: {} 字节", b.len());
                        }
                        Some(Ok(WsMessage::Close(_))) | None => {
                            println!("[*] WebSocket关闭，1秒后重连...");
                            break;
                        }
                        Some(Err(e)) => {
                            eprintln!("[!] WebSocket错误: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

/// 面向未来打洞/中继扩展的 WS 客户端封装（当前未接入主流程，保留供后续使用）
#[allow(dead_code)]
#[derive(Debug)]
pub struct BotWebSocket {
    ws_url: String,
}

#[allow(dead_code)]
impl BotWebSocket {
    pub fn new(config: &NapCatConfig) -> Self {
        Self {
            ws_url: format!("ws://{}:{}", config.ws_host, config.ws_port),
        }
    }

    pub async fn connect(&self) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>> {
        let (ws, _) = tokio_tungstenite::connect_async(&self.ws_url).await?;
        Ok(ws)
    }
}
