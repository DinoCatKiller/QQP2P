# holepunch-mvp（已归档）

> 归档时间：2026-08-24
> 归档原因：**MVP（双方 UDP 打洞）为可行性验证产物**，正式项目已按 `docs/plan/P2P_INFRA_PLAN.md` 转向 libp2p 传输 + 自研编排层方案。
> 本工程仅保留作为"上帝视角"打洞联调工具，**只维护不演进**。

## 功能

- STUN 映射获取 + UDP Hole Punching（对称 NAT 场景穿透率有限）
- 启动即保活 + 无限期等待 + 未连通自动重试
- 不依赖 QQ 信令、不依赖 NapCat，纯命令行联调

## 使用

```bash
# 进程A: 指定对方映射地址作为默认值, 启动后回车即可采用(也可输入新地址覆盖)
cargo run -- holepunch --port 8080 --peer-uid 2 --peer-mapped "B的ip:port"

# 进程B: 不传 --peer-mapped, 启动后从 stdin 输入对方映射地址(不输入则一直保活等待)
cargo run -- holepunch --port 8081 --peer-uid 1

# 查询本机 NAT 映射地址
cargo run -- mapped
cargo run -- mapped --port 8080
```

参数说明：

| 参数 | 说明 |
|------|------|
| `--port` | UDP 打洞监听端口，两个进程必须不同 |
| `--peer-uid` | 对端标识（本机会话表 key，可任意填，仅日志用） |
| `--peer-mapped` | 对方 NAT 映射地址 `ip:port`；作为交互输入默认值，也可输入新地址覆盖 |
| `--retry` | 打洞重试轮数上限，0=无限重试直到连通（默认 0） |
| `--keepalive` | 保活间隔秒数，0=禁用保活（默认 20） |
| `--stun` | STUN 服务器（默认 `stun.l.google.com:19302`） |

## 架构（归档时点）

```
src/
├── main.rs       # CLI：holepunch / mapped 子命令
├── p2p.rs        # P2PNode（打洞支撑：socket/STUN/映射缓存/会话表）
└── holepunch.rs  # STUN + 打洞核心逻辑（含单元测试）
```

## 文档

打洞方案与实测记录见 `docs/legacy/P2P_HOLE_PUNCHING.md`。
