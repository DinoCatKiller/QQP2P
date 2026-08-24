# QQP2P 代码主要对象结构概述

本文件先按**源文件分类**列出各模块中的主要对象/结构体/管理器，再在每个文件内部用**调用结构**说明它们如何被创建、被谁调用、又调用了谁。

---

## 1. Cargo.toml
依赖配置（非代码对象，但决定运行时的类型可用范围）

| 对象/配置 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `libp2p 0.43` (features: tcp, noise, yamux, dcutr, identify, ping) | 提供 P2P 传输层基础类型：`Swarm`、`PeerId`、`Multiaddr`、`Transport`、`Behaviour` 等 | `Swarm`、`PeerId`、`Multiaddr`、`TcpTransport`、`NoiseKeypair`、`YamuxConfig`、`DcutrConfig`、`Identify`、`Ping` | 被 `transport.rs`、`service.rs`、`protocol.rs`、`mod.rs` 全部间接引用 |

---

## 2. src/p2p/mod.rs
模块声明与类型重导出的“胶水层”

| 对象/结构 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `mod transport / protocol / service` | 声明子模块 | 无 | 编译期模块系统 |
| `pub use crate::legacy_p2p::{P2PNode, BotEvent, HANDSHAKE_CONFIRM_REPLY, PeerInfo}` | 重导出旧版类型以保持向后兼容 | 无 | main.rs、app.rs 通过 `crate::p2p::` 路径访问 |
| `pub use transport::start_transport` | 暴露传输层启动入口 | 无 | main.rs (`Commands::P2pStart`) |
| `pub use protocol::{MessageType, TunProtocolHandler, HelloMsg, JoinAckMsg, MemberInfo, DataMsg, encode_frame, decode_frame}` | 暴露协议类型与编解码 | 无 | service.rs、main.rs 间接使用 |
| `pub use service::P2pService` | 暴露核心服务 | 无 | main.rs (`Commands::P2pStart`) |

---

## 3. src/p2p/transport.rs
libp2p 传输层端点

### 文件内对象
| 对象/结构 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `start_transport(listen_port)` | **N1 核心**：生成 ED25519 密钥对与 PeerId；构建 TCP+Noise+Yamux+DCUtR 传输；配置 Identify+Ping 行为；创建 Swarm 并开始监听；返回 `(Swarm, Multiaddr, PeerId)` | `local_keypair`、`peer_id`、`noise_keypair`、`tcp_transport`、`dcutr_config`、`identify_config`、`ping_config`、`behaviour`、`swarm`、`listen_multiaddr` | 被 `service.rs::P2pService::new` 调用；被 `main.rs::Commands::P2pStart` 直接调用 |
| `handle_swarm_events(swarm)` | 监听并打印 Swarm 基础事件（连接建立、错误等） | `swarm: &mut Swarm` | 由上层主循环调用（当前 main.rs 中未被 active 调用） |
| `format_multiaddr(addr)` | 多地址转字符串便于打印 | `Multiaddr` | 工具函数，按需调用 |

### 调用结构（transport.rs 视角）
```
main.rs::P2pStart ──► start_transport()
                         ├── identity::Keypair::generate_ed25519() → PeerId
                         ├── libp2p::tcp::Transport::new()          → TcpTransport
                         ├── libp2p::dcutr::Transport::new()        → DCUtR 传输
                         ├── Identify::new() + Ping::new()          → behaviour
                         └── Swarm::new()                          → Swarm
service.rs::P2pService::new ──► start_transport()  (同上)
```

---

## 4. src/p2p/protocol.rs
`/tun/1.0.0` 自定义协议

### 文件内对象
| 对象/结构 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `MessageType` (enum: HELLO=0x01, JOIN_ACK=0x02, LEAVE=0x03, IP_CONFLICT=0x04, DATA=0x05) | 协议消息类型枚举；提供 `as_byte()` / `from_byte()` | 无（纯枚举） | 被 `TunProtocolHandler`、`decode_frame`/`encode_frame` 使用 |
| `HelloMsg` / `JoinAckMsg` / `MemberInfo` / `LeaveMsg` / `IpConflictMsg` / `DataMsg` | 各类消息载荷的 serde 结构体 | 各自的字段（peer_id、virtual_ip、features、members、raw 等） | 被 `TunProtocolHandler` 的编解码方法构造/解析 |
| `encode_frame(type_, payload)` → `Vec<u8>` | 编码单帧：`[1B type][4B len][payload]`（大端序） | 无持久状态 | 被 `TunProtocolHandler::encode_hello` / `encode_join_ack` 调用 |
| `decode_frame(data)` → `Option<(MessageType, Vec<u8>)>` | 解码单帧 | 无持久状态 | 被 `TunProtocolHandler::handle_incoming` 与 `handle()` 调用 |
| `TunProtocolHandler` | **协议处理器**：实现 `StreamHandler`，处理收到的帧，分发 HELLO/JOIN_ACK/DATA 等；提供 `build_hello_frame()`、`parse_hello_payload()`、`parse_join_ack_payload()` | `local_peer_id: PeerId`<br>`local_virtual_ip: String` | 被 `service.rs::P2pService` 持有并初始化；其 `handle()` 由 libp2p Swarm behaviour 在收到流消息时回调 |

### 调用结构（protocol.rs 视角）
```
service.rs::P2pService
   └── 持有 TunProtocolHandler
          ├── new(peer_id, virtual_ip)  → 创建
          ├── build_hello_frame()       → encode_hello() → encode_frame()
          ├── handle_incoming(data)     → decode_frame()
          └── impl StreamHandler::handle()
                 ├── decode_frame()
                 ├── parse_hello_payload()   → serde_json::from_slice::<HelloMsg>()
                 └── parse_join_ack_payload() → serde_json::from_slice::<JoinAckMsg>()
```

---

## 5. src/p2p/service.rs
P2pService 核心服务层

### 文件内对象
| 对象/结构 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `P2pService` | **N1 服务层**：协调 Swarm 与 `/tun/1.0.0` 协议；提供 `connect_to_peer`、`send_hello`、`send_join_ack`、`poll_events` 等 API | `swarm: Swarm<libp2p::SwarmHandler>`<br>`protocol: TunProtocolHandler`<br>`local_virtual_ip: String`<br>`known_peers: HashMap<PeerId, (Multiaddr, String)>` | 被 `main.rs::Commands::P2pStart` 创建并进入 `loop { poll_events() }` |
| `P2pService::new(listen_port, virtual_ip)` | 调用 `start_transport` 创建 Swarm，再用其返回的 peer_id 初始化 `TunProtocolHandler` | 见上 | main.rs |
| `connect_to_peer(peer_id, multiaddr)` | 向 Swarm 发起拨号并登记到 `known_peers` | 调用 `swarm.dial_addr()` | 上层 CLI（尚未在 main.rs 中接线） |
| `send_hello(target)` / `send_join_ack(members)` | 返回待发送帧的字节 | 委托 `protocol` 编解码 | 上层联调调用 |
| `poll_events()` / `process_event(event)` | 驱动 `swarm.next()` 并处理事件 | 调用 `swarm.next()` | main.rs 主循环 |

### 调用结构（service.rs 视角）
```
main.rs::Commands::P2pStart
   └── P2pService::new(port, virtual_ip)
          ├── transport::start_transport(port)  → (swarm, listen_addr, peer_id)
          └── TunProtocolHandler::new(peer_id, virtual_ip)
   └── loop { service.poll_events() }
          └── swarm.next() → process_event()
                 ├── NewConnection / ConnectionEstablished → 打印
                 └── Behaviour(_) → 协议已在 handler 内部处理
```

---

## 6. src/legacy_p2p.rs
旧版 TCP P2P 节点（向后兼容）

### 文件内对象
| 对象/结构 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `HANDSHAKE_CONFIRM_REPLY` (const &str) | 握手确认回复文案（防无限互发） | 无 | `parse_and_store_peer_ip`；`app.rs` 用于比对 |
| `PeerInfo` | 对端节点信息 | `user_id: u64, ip: String, port: u16` | `P2PNode`、`app.rs` 添加到 peers 表 |
| `BotEvent` (enum) | 机器人内部事件：PrivateMessage / GroupMessage / Connected / Disconnected | 各变体字段 | `P2PNode::send_event` 发出；`ws.rs` 接收处理 |
| `P2PNode` | 管理本机节点信息、TCP 服务器、连接管理、打洞、IP 解析 | `user_id`、`ip`、`port`<br>`peers: Arc<Mutex<HashMap<u64, PeerInfo>>>`<br>`peer_ips: Arc<Mutex<HashMap<u64, String>>>`<br>`event_tx: Arc<Mutex<broadcast::Sender<BotEvent>>>` | 被 `app.rs::BotApp::new` 创建；被 `main.rs::Start/Connect/Status` 调用；被 `app.rs::handle_message` 调用 |

### 调用结构（legacy_p2p.rs 视角）
```
app.rs::BotApp::new ──► P2PNode::new(user_id) → (node, event_rx)
main.rs::Commands::Start ──► P2PNode::new() / get_public_ip()
app.rs::handle_message ──► node.parse_and_store_peer_ip()
                                ├── extract_ip_port() / is_valid_ipv4()
                                └── 命中重复 → 返回 HANDSHAKE_CONFIRM_REPLY
                          node.try_auto_connect() → node.connect_to_peer()
                          node.send_event(BotEvent) → event_tx 广播
ws.rs::run_message_handler ──► 接收 BotEvent 广播
```

---

## 7. src/napcat.rs
NapCat HTTP API 客户端

### 文件内对象
| 对象/结构 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `NapCatConfig` | 连接配置（HTTP/WS host、port、token） | `http_host, http_port, ws_host, ws_port, token` | `NapCatClient` 内部持有；`ws.rs` 读取 config 连接 WS |
| `NapCatResponse<T>` | API 响应统一包装 | `status, retcode, data: Option<T>, message` | `NapCatClient` 所有方法的返回类型 |
| `UserInfo` / `FriendInfo` / `GroupInfo` | 数据结构体 | 各自字段 | `NapCatClient` 的查询方法返回 |
| `NapCatClient` | 封装发私聊/群消息、查好友/群/登录信息 | `client: reqwest::Client`<br>`config: NapCatConfig` | `app.rs::BotApp::new` 创建；`main.rs` 各子命令；`ws.rs` 通过 app 转发 |

### 调用结构（napcat.rs 视角）
```
app.rs::BotApp::new ──► NapCatClient::new_default()
app.rs::send_reply / send_group_reply ──► napcat.send_private_message / send_group_message
main.rs 子命令 (Friends/Groups/Online/Send/...) ──► NapCatClient 对应方法
ws.rs::websocket_listener ──► 读取 napcat.config 构造 WS URL
```

---

## 8. src/ws.rs
WebSocket 事件监听与消息分发

### 文件内对象
| 对象/结构 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `NapCatEvent` | WS 推送事件结构体（只取关心字段） | `post_type, message_type, self_id, user_id, group_id, raw_message, message` | `websocket_listener` 解析 JSON 时使用 |
| `BotWebSocket` (预留，未接入主流程) | WS 客户端封装 | `ws_url: String` | 当前未使用（保留扩展） |
| `websocket_listener(app)` | 连接 NapCat WS，监听消息，过滤（@机器人/协议消息），转成 `BotEvent` 发给 `app.send_event` | 内部：`WsStream`、连接状态 | `main.rs::Commands::Start` 中 `tokio::spawn` 启动 |
| `run_message_handler(app, event_rx)` | 消费 `BotEvent` 广播，调用 `app.handle_message` 并回复发回 QQ | 内部：`event_rx` 接收器 | `main.rs::Commands::Start` 中 `tokio::spawn` 启动 |

### 调用结构（ws.rs 视角）
```
main.rs::Commands::Start
   ├── tokio::spawn(websocket_listener(app_clone))
   │       └── 从 NapCat WS 收消息 → 过滤 → app.send_event(BotEvent)
   └── tokio::spawn(run_message_handler(app_clone, event_rx))
           └── 接收 BotEvent → app.handle_message() → app.send_reply/send_group_reply
                                                              └── NapCatClient 发消息
```

---

## 9. src/app.rs
BotApp 主应用与消息处理

### 文件内对象
| 对象/结构 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `BotApp` | 主应用结构体：聚合 P2P 节点与 NapCat 客户端，处理 QQ 消息与 P2P 握手流程 | `node: Arc<Mutex<P2PNode>>`<br>`napcat: Arc<Mutex<NapCatClient>>`<br>`my_user_id: u64`<br>`my_name: String` | `main.rs::Commands::Start` 创建；`ws.rs` 的两个任务持有其 clone |
| `BotApp::new(user_id)` | 创建 P2PNode 与 NapCatClient，并以 NapCat 登录信息覆盖 user_id/name | 见上 | main.rs |
| `handle_message(sender_id, raw)` | 6 种流程分发：给我p2p / 节点信息 / IP解析 / /connect / /status / /help | 调用 `node` 与 `napcat` | `ws.rs::run_message_handler` |
| `send_event / send_reply / send_group_reply` | 事件与消息收发封装 | 委托 `node` / `napcat` | ws.rs、main.rs |

### 调用结构（app.rs 视角）
```
main.rs::Commands::Start ──► BotApp::new(user_id)
                                ├── P2PNode::new()
                                └── NapCatClient::new_default()
ws.rs::run_message_handler ──► app.handle_message()
                                 ├── node.parse_and_store_peer_ip()
                                 ├── node.try_auto_connect()
                                 ├── node.get_ip_info()
                                 └── app.send_reply/group_reply → napcat.send_*
```

---

## 10. src/main.rs
CLI 入口与子命令装配

### 文件内对象
| 对象/结构 | 主要职责 | 持有的其他对象 | 被哪些模块调用 |
|---|---|---|---|
| `Cli` (clap Parser) | 顶层 CLI 结构 | `command: Commands` | `#[tokio::main] async fn main()` |
| `Commands` (enum) | 子命令：Start / Ip / Connect / Status / Send / SendGroup / Friends / Groups / Online / P2pStart / P2pConnect | 各变体字段 | `main()` 通过 `match cli.command` 分发 |
| `main()` | 程序入口：解析 CLI，按命令装配各模块任务 | 创建 `BotApp`、`P2pService`、`start_transport` 等 | 操作系统启动进程 |

### 调用结构（main.rs 视角）
```
main()
 ├── Commands::Start
 │     ├── BotApp::new() → P2PNode + NapCatClient
 │     ├── tokio::spawn(start_transport(port))         [N1 后台]
 │     ├── tokio::spawn(ws::websocket_listener(app))
 │     └── tokio::spawn(ws::run_message_handler(app, event_rx))
 ├── Commands::P2pStart { port }
 │     ├── start_transport(port) → (swarm, addr, peer_id)
 │     ├── P2pService::new(port, virtual_ip)
 │     └── loop { service.poll_events() }
 ├── Commands::P2pConnect { peer_id }   [TODO: 未实现]
 └── Commands::Connect/Ip/Status/Send/...  → 旧版 P2PNode / NapCatClient
```

---

## 外部依赖类型（libp2p）
| 类型 | 主要职责 | 被哪些模块调用 |
|---|---|---|
| `Swarm` | libp2p 事件循环与连接管理核心 | transport.rs（创建）、service.rs（poll_events）、main.rs（P2pStart） |
| `PeerId` | 节点唯一标识（源自 ED25519 公钥） | transport.rs（生成）、mod.rs（重导出）、service.rs、main.rs（打印） |
| `Multiaddr` | 网络多地址抽象 | transport.rs（listen）、service.rs（connect_to_peer）、mod.rs（重导出） |
| `TcpTransport` / `NoiseKeypair` / `YamuxConfig` / `DcutrConfig` / `Identify` / `Ping` | 传输层各组件 | transport.rs（组装） |
