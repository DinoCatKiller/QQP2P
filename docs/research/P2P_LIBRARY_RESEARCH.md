# P2P 候选库深度解析（iroh / rust-libp2p / webrtc-rs / quinn）

> 调研时间：2026-08-21
> 关联文档：`docs/P2P_OVERLAY_RESEARCH.md`、`docs/P2P_HOLE_PUNCHING.md`
> 关联代码：`src/holepunch.rs`、`src/p2p.rs`

---

## 一、先看地图：这四个库不在同一层

很多人把 quinn 和 iroh 放一起比，其实是错位比较。正确的分层：

```
应用层（你要的：P2P 通道）
├── iroh            ← 完整 P2P 框架（打洞+中继+加密+存储 全家桶）
├── rust-libp2p     ← 完整 P2P 框架（模块化、可插拔，全家桶但你要自己装）
└── webrtc-rs       ← 另类的全家桶（ICE打洞+DTLS加密+SCTP，但定位不是纯数据通道）
传输层（地基）
└── quinn           ← 只是 QUIC 协议实现，不知道 NAT 是什么东西
```

---

## 二、quinn —— 传输层地基

### 谁的库

- 最早是 **Benjamin Saunders** 的个人项目（曾在 Cloudflare 专注 QUIC），现在由 **quinn-rs 社区**维护。
- Rust 生态里最流行的 QUIC 实现。

### 解决什么问题

- QUIC = 基于 UDP 的"TCP 超进化体"：可靠传输、多路复用流（一条 UDP 上开 N 条 stream）、TLS 1.3 加密、0-RTT 快连、连接迁移。
- 解决"要现代可靠加密传输，但不想被 TCP 队头阻塞（head-of-line blocking）拖死"的问题。

### 利 / 弊

| 利 | 弊 |
|---|---|
| 纯 Rust、内存安全 | **只管传输**：不帮你打洞、不发现对端、不中继 |
| 性能接近 C 实现 | 给你一个 `UdpSocket` 就是全部，剩下全是你的事 |
| API 相对干净，`quinn-proto` 可无 IO 纯状态机运行（嵌入式友好） | |
| 配套 `h3` crate 直接上 HTTP/3 | |

### 竞争关系

同层对手：`quiche`（Cloudflare）、`s2n-quic`（AWS）、`neqo`（Mozilla/Firefox）、`picoquic`/`lsquic`（C）。quinn 在当中 API 最舒服、生态最活跃。

### 八卦

- `iroh` 的传输层就是 quinn；libp2p 的 QUIC transport 底层也是 quinn。
- 对应文档里的"M3 阶段用 QUIC 加密"，就是它——但注意它**默认你已经打通了洞**。

---

## 三、webrtc-rs —— 野路子出身的老牌移民

### 谁的库

- 纯 Rust 的 WebRTC 实现，**Pion**（Go 生态最有名的 WebRTC 库）的移植版。
- 主导者 **Martin Algesten**（Pion 团队成员）。Pion 作者 Sean DuBois 是"WebRTC 不该绑死在浏览器"运动的代表人物。

### 解决什么问题

- 让 Rust 开发者能在**没有浏览器、没有 C++ libwebrtc** 的情况下用 WebRTC 全家桶：ICE（STUN/TURN 打洞+中继）、DTLS、SRTP、SCTP、DataChannel、媒体轨道全都有。

### 利 / 弊

| 利 | 弊 |
|---|---|
| 纯 Rust、可编 WASM、能 headless 运行 | 性能/成熟度追不上 Google 的 C++ libwebrtc |
| API 对齐 Pion 生态 | **不带编解码器**（VP8/H264 要自己接 gstreamer/ffmpeg） |
| **浏览器参与场景的唯一解**（浏览器只能走 WebRTC/WS/TURN，塞不进 QUIC） | trickle ICE 等细节有坑，迭代速度一般 |

### 竞争关系

直接对手是 libwebrtc 的 Rust 绑定（`webrtc-sys`，LiveKit 客户端走这路子）。在纯数据 P2P 场景与 iroh/libp2p 抢地盘，但本质是"错位竞争"：它的核心价值是浏览器互通 + TURN 生态成熟。

> 注：WebRTC 的 DataChannel 本质就是"ICE打洞 + DTLS加密 + SCTP可靠流"，本身就是一个成熟 P2P 栈，只是通常没人纯当数据通道用它。

---

## 四、rust-libp2p —— P2P 界的 Linux 内核

### 谁的库

- **libp2p 项目由 Protocol Labs**（Juan Benet，IPFS 之父）发起，是 IPFS 的网络层。
- Rust 实现最初由 **Parity**（Polkadot）大力投入（Substrate 需要），现由 libp2p 基金会协调、跨公司社区维护（核心贡献者如 mxinden）。

### 解决什么问题

"节点如何互相找到并安全通信"的**模块化全家桶**：

- 传输可插拔：TCP / QUIC / WebRTC / WS / WSS
- 安全：noise / TLS
- 多路复用：yamux
- 协议协商：multistream-select
- 发现：mDNS / Kademlia DHT
- 广播：gossipsub
- NAT：relay + DCUtR 打洞

解决的是"做一个通用 P2P 网络层，任何项目都能用"。

### 利 / 弊

| 利 | 弊 |
|---|---|
| 生态最大：IPFS、Polkadot、Filecoin 生产环境验证过 | **概念爆炸**：Transport / Swarm / ConnectionHandler / NetworkBehaviour 学习曲线陡 |
| 多语言同构（Go/Rust/JS 协议互通） | 版本演进破坏性大（0.4x→0.5x 大改劝退不少人） |
| 想魔改每一层时它是唯一选项 | **打洞不是开箱即用**：relay + DCUtR 要自己拼，配置繁琐 |

### 竞争关系

与 iroh 同赛道直接对手；与 ZeroTier/Tailscale 在"虚拟局域网"需求上也竞争，只是形态不同。

---

## 五、iroh —— 从 IPFS 叛逃出来的人重写了"傻瓜版 libp2p"

### 谁的库

- **n0 team**，创始人 **Friedel Ziegelmair** 是 Protocol Labs/IPFS 核心老人（写过 bitswap 等 IPFS 底层）。
- 相当于 **IPFS 嫡系叛逃创业**，2026-06 发布 1.0 稳定版。

### 解决什么问题

一句话：**"给应用加 P2P"应该像调 API 一样简单，而不是像学框架。**

- 寻址不再用 IP:port，而是 `NodeId`（= Ed25519 公钥）。
- 三件套：
  - `iroh-net`：QUIC 连接 + DERP 中继 + 自动打洞 + 自动保活
  - `iroh-blobs`：内容寻址 CAS（IPFS 血统）
  - `iroh-gossip` / `iroh-docs`：订阅 + CRDT 文档同步

### 利 / 弊

| 利 | 弊 |
|---|---|
| API 极简：`endpoint.connect(node_id)` 完事 | 年轻，生态小（star 数与 libp2p 差一个量级） |
| **默认端到端加密** | 只走 QUIC（不吃 WebRTC/WS） |
| 打洞+中继+保活开箱即用（覆盖当前打洞的 9 条硬伤） | 公共 relay / STUN 在国外，**墙内大概率要自建 derper** |
| 免费公共 relay，可自建 derper | 1.0 之前 API 变动快 |
| 官方宣称穿透成功率约 70%，明显高于 libp2p 实测 | |

### 八卦

- 名字来自《降世神通》里的大叔 Iroh。
- 中继设计 **DERP**（Designated Encrypted Relay for Packets）直接借了 Tailscale 的方案。
- 传输层用的就是 quinn。
- 一句话：**n0 = IPFS 的经验 + Tailscale 的中继 + quinn 的传输，捏成"程序员友好版"。**

---

## 六、竞争关系总图

```
                 iroh ◄──────────► rust-libp2p     ← 同赛道正面刚
                  │ 依赖 quinn         │ 依赖 quinn
                  ▼                    ▼
                quinn ◄────► quiche / s2n-quic / neqo   ← 传输层内卷

    webrtc-rs ◄─────► libwebrtc 绑定（webrtc-sys）       ← 另一条赛道
        │
        └── 在"浏览器参与"场景是 iroh/libp2p 的补充而不是对手
```

几段恩怨值得记住：

1. **iroh 的本质是"对 libp2p 感到痛苦的人重写的更简单版本"**。n0 团队自己就从 libp2p 生态（IPFS）出来，公开对比过：同样的 P2P 场景，libp2p 的配置代码和心智负担是 iroh 的好几倍，而 iroh 用"牺牲一部分可定制性"换来了约 70% 的穿透成功率。
2. **quinn 是所有人的地基**，不参与上层竞争——iroh 和 libp2p 的 QUIC 传输都踩在它身上。
3. **webrtc-rs 是错位竞争**：核心竞争力不是性能，而是"浏览器互通 + TURN 生态成熟"。若需求变成"网页端也要连进来"，iroh 就得靠 webrtc-rs 或自建 WSS 中转补位。

---

## 七、落回 QQP2P 的结论

- **主线走 iroh**：它内置 NAT 类型探测 + 端口预测 + 多候选 + DERP 兜底，正好对应 `P2P_OVERLAY_RESEARCH.md` 里列的硬伤 1/2/3/4。自研补全这套 = 重复造轮子，可靠性还难追平。
- **国内网络必须自建 derper**：iroh 公共 relay 和 STUN 都在国外，国内大概率不稳。DERP 开源，一台便宜 VPS 就能跑；可顺手跑 coturn 双保险。
- **webrtc-rs 别急着否**：若以后加"浏览器控制面板"之类的需求，它是唯一不用另起炉灶的选项——iroh 不吃 WebRTC。
- **quinn 是最后的逃生通道**：如果 iroh 的抽象满足不了（比如要自定义握手协议），`quinn-proto` 让你在保住 QUIC 的前提下裸写自己的打洞逻辑——这是"自研"和"用框架"的中间态。

---

## 八、选型速查表

| 维度 | iroh | rust-libp2p | webrtc-rs | quinn |
|---|---|---|---|---|
| 定位 | 完整 P2P 框架 | 完整 P2P 框架（可魔改） | WebRTC 栈（偏媒体/浏览器） | QUIC 传输库 |
| 打洞 | ✅ 开箱即用（内置探测/预测/多候选） | ⚠️ relay+DCUtR 自己拼 | ✅ 完整 ICE(STUN/TURN) | ❌ 需自己打洞 |
| 中继 | ✅ DERP（可自建） | ⚠️ 需自建 relay | ✅ TURN | ❌ |
| 加密 | ✅ QUIC+TLS（默认 E2E） | ✅ noise/TLS | ✅ DTLS/SRTP | ✅ TLS 1.3 |
| 发现对端 | ✅ NodeId（公钥寻址） | ✅ mDNS/DHT | 需信令 | ❌ |
| API 友好度 | 极高 | 低（概念多） | 中（信令自建） | 高 |
| 浏览器互通 | ❌ | ⚠️ WebRTC transport 可拼 | ✅ 原生 | ❌ |
| 墙内可用性 | ⚠️ 公共 relay 需自建 derper | ⚠️ 中继需自建 | ⚠️ TURN 需自建 | —（纯传输） |
| 成熟度 | 1.0（2026-06），生态年轻 | 生产验证（IPFS/Polkadot） | 追 libwebrtc，中等 | 成熟活跃 |
| 最适合场景 | 纯 Rust 数据通道、常驻后台 | 需要深度定制/跨协议生态 | 浏览器参与、音视频 | 只要可靠加密传输 |
