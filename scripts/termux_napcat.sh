#!/data/data/com.termux/files/usr/bin/bash
# termux_napcat.sh - 手机端一键安装 NapCat（QQ 协议层）
#
# 用法:  bash termux_napcat.sh
#
# 原理:
#   NapCat 官方没有 Android 版, 但官方提供了 Termux 专用一键安装脚本
#   (github.com/NapNeko/NapCat-Installer, script/install.termux.sh),
#   它会在 proot 容器中安装 Linux 版 QQ + NapCat, 本脚本负责包装它。
#
# 装完后:
#   - 启动 NapCat, 首次需扫码登录手机机器人 QQ
#   - 管理 WebUI: http://127.0.0.1:6099/webui
#   - 在 WebUI 网络配置中启用: HTTP(3000) + WS 服务端(3001, /onebot/v11/ws)
#     监听 127.0.0.1 即可, 与 qqp2p 默认值一致
#
# 若官方脚本失败, 会打印手动方案指引 (proot-distro + Ubuntu + NapCat)。

set -uo pipefail

echo "=================================================="
echo "  Termux 一键安装 NapCat"
echo "=================================================="

echo
echo "[1/3] 更新软件源..."
pkg update -y
pkg upgrade -y

echo
echo "[2/3] 安装基础工具 (proot-distro 用于手动方案兜底)..."
pkg install -y proot-distro curl wget git xz-utils

echo
echo "[3/3] 下载并执行 NapCat 官方 Termux 安装脚本..."
SCRIPT_URL="https://nclatest.znin.net/napneko/napcat-installer/main/script/install.termux.sh"
echo "    下载: $SCRIPT_URL"
echo
if curl -fsSL -o "$HOME/napcat.termux.sh" "$SCRIPT_URL"; then
    bash "$HOME/napcat.termux.sh"
    echo
    echo "[+] 安装流程执行完毕!"
    echo "    - 启动 NapCat 并扫码登录后:"
    echo "        WebUI: http://127.0.0.1:6099/webui"
    echo "    - WebUI 中启用 HTTP(3000) 与 WS 服务端(3001, 路径 /onebot/v11/ws)"
    echo "    - 参考文档: https://napneko.github.io/guide"
else
    echo "[!] 官方脚本下载失败, 请检查网络后重试: bash termux_napcat.sh"
    echo
    echo "手动方案指引 (官方脚本不可用时):"
    echo "  # 1. 安装并进入 Ubuntu 容器"
    echo "  proot-distro install ubuntu"
    echo "  proot-distro login ubuntu"
    echo "  # 2. 容器内安装依赖 + NapCat Linux 安装器"
    echo "  apt update && apt install -y xvfb xauth wget curl"
    echo "  curl -fsSL -o /tmp/napcat.sh https://raw.githubusercontent.com/NapNeko/napcat-linux-installer/refs/heads/main/install.sh"
    echo "  bash /tmp/napcat.sh"
    echo "  # 3. 启动 (首次扫码登录)"
    echo "  xvfb-run -a qq --no-sandbox"
    echo "  # 4. 手机浏览器访问 WebUI: http://127.0.0.1:6099/webui"
fi
