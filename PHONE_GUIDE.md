# QQ P2P 手机+电脑连接指南

## 架构说明

```
电脑 (编译Rust)          手机 (运行Python)
     |                        |
     |  1. 编译release        |
     |----------------------->|
     |                        |
     |  2. 运行Rust程序       |
     |                        |
     |<-----------------------|
     |  3. 手机通过HTTP调用   |
     |                        |
电脑NapCat               手机NapCat
(127.0.0.1:30000)      (127.0.0.1:30000)
```

## 方案一：电脑+手机（推荐）

### 步骤1：电脑编译Release版本

```bash
cd H:\QQP2P
cargo build --release
```

编译完成后，二进制文件在：
```
H:\QQP2P\target\release\qqp2p.exe
```

### 步骤2：传到手机

**方法A：USB传输**
```bash
# 电脑执行
copy target\release\qqp2p.exe D:\
# 然后USB连接手机，复制文件
```

**方法B：HTTP传输**
```bash
# 电脑启动临时服务器
cd H:\QQP2P\target\release
python -m http.server 8080
```
然后手机浏览器访问 `http://电脑IP:8080/qqp2p.exe`

### 步骤3：手机安装依赖

**Android (Termux):**
```bash
pkg install python
pip install aiohttp websockets
```

**iPhone (iSH/Shadow):**
```bash
apk add python3
pip3 install aiohttp websockets
```

### 步骤4：手机运行

```bash
# 方法A：使用Python版（推荐，支持WebSocket监听）
python bot_full.py --user-id 你的手机QQ号 --start

# 方法B：使用简化版
python bot.py --user-id 你的手机QQ号 --start
```

### 步骤5：电脑运行

```bash
# 在电脑上启动Rust版本
cargo run -- start --user-id 你的电脑QQ号 --port 8080
```

---

## 方案二：纯Python（跨平台）

### 电脑和手机都运行Python版本

```bash
# 1. 安装依赖
pip install aiohttp websockets

# 2. 运行
python bot_full.py --user-id 你的QQ号 --start
```

---

## 完整测试流程

### 设备A（电脑）
```bash
# 1. 启动NapCat
cd H:\napcat
.\launcher.bat

# 2. 编译运行
cd H:\QQP2P
cargo run -- start --user-id A的QQ号 --port 8080
```

### 设备B（手机）
```bash
# 1. 安装Termux
# 从F-Droid下载Termux

# 2. 安装依赖
pkg install python
pip install aiohttp websockets

# 3. 复制bot_full.py到手机
# 4. 启动NapCat（如果手机也有NapCat）
# 5. 运行
python bot_full.py --user-id B的QQ号 --start
```

### 在QQ中测试

**A发送：**
```
@机器人 给我一个p2p
```

**B收到：**
```
🌐 我的P2P节点信息:
📍 公网IP: xxx.xxx.xxx.xxx
🔌 端口: 8080
```

**B也发送：**
```
@机器人 给我一个p2p
```

**A收到：**
```
🌐 我的P2P节点信息:
📍 公网IP: yyy.yyy.yyy.yyy
🔌 端口: 8080
```

**A告诉B自己的IP，B告诉A自己的IP**

**A发送：**
```
@机器人 我的IP是 B的IP:8080
```

**B发送：**
```
@机器人 我的IP是 A的IP:8080
```

**双方收到：**
```
✅ 已记录你的IP: xxx.xxx.xxx.xxx:8080
🔄 正在尝试连接...
```

---

## 常见问题

### Q: 手机没有NapCat怎么办？
A: 手机只运行Python脚本，通过电脑的NapCat API通信。需要确保电脑和手机在同一网络，或者使用内网穿透。

### Q: 如何获取电脑IP？
A: 
```bash
# Windows
ipconfig
# 查看 IPv4 地址
```

### Q: 防火墙问题
A: 确保电脑防火墙允许：
- TCP 8080端口（P2P连接）
- TCP 30000端口（NapCat HTTP）
- TCP 30001端口（NapCat WebSocket）

---

## 依赖安装

### Python依赖
```bash
pip install aiohttp websockets
```

### Rust依赖（已在Cargo.toml中）
```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive"] }
anyhow = "1"
reqwest = { version = "0.11", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
futures-util = "0.3"
tokio-tungstenite = "0.21"
```
