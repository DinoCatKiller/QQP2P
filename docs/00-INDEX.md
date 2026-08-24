# QQP2P 文档索引

> 文档按"调研 → 计划 → 指南 → 归档"四层组织。实施以 `plan/P2P_INFRA_PLAN.md` 为准。

## research/ 调研结论

| 文档 | 内容 |
|---|---|
| [P2P_OVERLAY_RESEARCH.md](research/P2P_OVERLAY_RESEARCH.md) | P2P Overlay 网络调研（应用层通道 vs 虚拟网卡） |
| [P2P_LIBRARY_RESEARCH.md](research/P2P_LIBRARY_RESEARCH.md) | P2P 库选型对比（iroh / libp2p / 自研） |
| [P2P_ECOSYSTEM_RESEARCH.md](research/P2P_ECOSYSTEM_RESEARCH.md) | P2P 生态与虚拟网卡方案调研（EasyTier/wintun 等） |

## plan/ 方案与计划（当前实施依据）

| 文档 | 内容 |
|---|---|
| [P2P_INFRA_PLAN.md](plan/P2P_INFRA_PLAN.md) | **实施总计划**：libp2p 传输 + 自研编排层 + wintun TUN，N0-N5 里程碑 |
| [architecture.png](plan/architecture.png) | 架构图 |

## guides/ 使用指南

| 文档 | 内容 |
|---|---|
| [PHONE_GUIDE.md](guides/PHONE_GUIDE.md) | 手机端（Termux）部署指南 |

## legacy/ 已废弃路线（仅存档，不再演进）

| 文档 | 内容 |
|---|---|
| [P2P_HOLE_PUNCHING.md](legacy/P2P_HOLE_PUNCHING.md) | 自研 UDP 打洞方案（MVP 可行性验证，已归档；代码见 `legacy/holepunch-mvp/`） |

## 相关目录

| 路径 | 说明 |
|---|---|
| `src/` | 主工程源码（QQ 信令控制面 + 后续按计划接入 libp2p） |
| `legacy/holepunch-mvp/` | MVP 打洞独立工程（UDP NAT 穿越联调工具，仅维护不演进） |
| `docs/` | 本文档树 |
