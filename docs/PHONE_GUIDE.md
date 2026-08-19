# QQ P2P 手机+电脑连接指南（全 Rust 版）

> 手机端与电脑端运行**同一份 Rust 代码**，各自连接本机 NapCat（OneBot v11），
> 两端通过 QQ 消息互换公网节点信息，再建立 TCP 直连（P2P）。
> 打洞方案详见 `P2P_HOLE_PUNCHING.md`。

## 架构说明

```
电脑                                 手机
┌──────────────────────┐            ┌──────────────────────────────┐
│ qqp2p.exe  (Rust编译) │   TCP直连   │ qqp2p (Termux 中 Rust 编译)   │
│      ↕                │◄──────────►│      ↕                       │
│ NapCat (Windows 版)   │   P2P连接   │ NapCat (Termux/proot 中 Linux 版) │
│ 127.0.0.1:3000/3001   │            │ 127.0.0.1:3000/3001          │
└──────────────────────┘            └──────────────────────────────┘
    登录一个 QQ 号                     登录另一个 QQ 号
```

要点：

- `qqp2p` 连接地址默认 `127.0.0.1:3000`（HTTP）/ `3001`（WebSocket），正好指向**本机** NapCat，手机端**无需修改任何配置**。
- 手机端**不是**复制电脑的 `qqp2p.exe`（Windows 程序无法在 Android 运行），而是把源码放进 Termux 重新编译出 ARM 原生二进制。

## 一、电脑端

```bash
# 1. 编译
cd H:\QQP2P
cargo build --release

# 2. 启动电脑 NapCat（Windows 版），登录电脑机器人 QQ

# 3. 运行
target\release\qqp2p.exe start --user-id 你的电脑QQ号 --port 8080
```

## 二、手机端（Termux）

### 步骤 1：安装 Termux

- 从 **F-Droid** 或 GitHub Releases 下载最新 Termux APK
  （`https://github.com/termux/termux-app/releases/latest`）
- ⚠️ 不要用 Google Play 版（已停止维护，会报仓库不可用）
- 首次打开执行 `termux-setup-storage` 授权存储访问

### 步骤 2：安装手机 NapCat（QQ 协议层）

```bash
# 拉取项目脚本（或先 git clone 项目）
pkg install -y git
git clone https://github.com/DinoCatKiller/QQP2P.git
cd QQP2P

# 一键安装：自动装 proot 容器 + Linux 版 QQ + NapCat
bash scripts/termux_napcat.sh
```

装完启动 NapCat 后：

1. **首次扫码登录**手机机器人 QQ（登录方式取决于安装器提示）
2. 打开 WebUI：`http://127.0.0.1:6099/webui`
3. 在网络配置中启用并确认：
   - HTTP 服务：`127.0.0.1:3000`
   - WebSocket 服务端：`127.0.0.1:3001`，路径 `/onebot/v11/ws`

> 若官方一键脚本不可用，脚本末尾会打印手动方案（proot-distro 装 Ubuntu → 容器内装 NapCat），照抄即可。

### 步骤 3：编译手机端机器人（全 Rust）

```bash
cd QQP2P
bash scripts/termux_setup.sh
```

脚本会：更新软件源 → 安装 `rust clang openssl pkg-config git` → `cargo build --release`
（首次编译约 5~15 分钟，属正常现象，保持屏幕常亮）。

### 步骤 4：启动手机端机器人

```bash
./target/release/qqp2p start --user-id 你的手机QQ号 --port 8080
```

看到 `[*] 等待QQ消息...` 即启动成功。

## 三、在 QQ 中测试 P2P

**A 发送：**

```
@机器人 给我一个p2p
```

**A 收到：**

```
🌐 我的P2P节点信息:
📍 公网IP: xxx.xxx.xxx.xxx
🔌 端口: 8080
```

**B 也发送 `@机器人 给我一个p2p`，获取自己的节点信息。**

**然后互告对端节点：**

```
@机器人 我的IP是 B的公网IP:8080
```

**双方收到：**

```
✅ 已记录你的IP: xxx.xxx.xxx.xxx:8080
🔄 正在尝试连接...
```

连接成功后即建立 TCP 直连，后续数据不再经过任何中间服务器。

## 常见问题

### Q: 手机上为什么要编译 Rust，不直接复制 qqp2p.exe？
A: `qqp2p.exe` 是 Windows 可执行文件（PE + x86_64），Android 是 Linux 内核 + ARM，格式不兼容，无法运行。手机端必须用同一份源码在 Termux 里编译出 ARM 版二进制。

### Q: 手机 NapCat 一定装得起来吗？
A: NapCat 官方无 Android 版，但官方提供 Termux 安装器（在 proot 容器中跑 Linux 版）。已在社区大量验证。若你手机内存 < 4GB，建议先跑通"手机连电脑 NapCat"验证 P2P，再折腾手机 NapCat。

### Q: 手机没有 NapCat，能先验证 P2P 吗？
A: 可以。让电脑 NapCat 的监听地址从 `127.0.0.1` 改为 `0.0.0.0`，手机端 qqp2p 通过 `--napcat-host 电脑IP`（若代码已支持该参数）或临时改源码地址连接。前提是手机与电脑在同一局域网（或使用内网穿透）。

### Q: 如何获取电脑 IP？
A: Windows 执行 `ipconfig`，查看 IPv4 地址；手机查看 WiFi 详情。

### Q: 防火墙问题
A: 确保电脑防火墙放行：
- TCP `8080`（P2P 连接）
- TCP `3000`（NapCat HTTP）
- TCP `3001`（NapCat WebSocket）
- UDP/TCP `6099`（NapCat WebUI，可选）

## 依赖说明

Rust 依赖已在 `Cargo.toml` 中（tokio / clap / reqwest / serde / tokio-tungstenite 等），手机端由 `termux_setup.sh` 自动安装工具链，无需手动处理。

> 手机端配置文件 `config.ini`：当前版本 NapCat 地址为代码内硬编码默认值 `127.0.0.1:3000/3001`，与手机本机 NapCat 一致，无需改动；`config.ini` 为预留配置。
