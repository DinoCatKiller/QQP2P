#!/data/data/com.termux/files/usr/bin/bash
# termux_setup.sh - 手机端一键安装 Rust 并编译 qqp2p 机器人（全 Rust 方案）
#
# 用法:  bash termux_setup.sh [项目路径]
#   项目路径: 源码所在目录, 默认 ~/qqp2p; 若不存在则自动 git clone
#
# 说明:
#   - 自动更新 Termux 软件源并安装 Rust 工具链（clang/openssl 是 reqwest 编译必需）
#   - 编译产物: <项目路径>/target/release/qqp2p
#   - 手机首次编译约 5~15 分钟, 属正常现象, 请保持屏幕常亮并耐心等待

set -euo pipefail

PROJECT_DIR="${1:-$HOME/qqp2p}"
REPO_URL="https://github.com/DinoCatKiller/QQP2P.git"

echo "=================================================="
echo "  Termux 一键编译 qqp2p（全 Rust 手机端）"
echo "=================================================="

echo
echo "[1/4] 更新软件源..."
pkg update -y
pkg upgrade -y

echo
echo "[2/4] 安装 Rust 工具链与编译依赖..."
# clang: Rust 链接 C 代码所需; openssl/pkg-config: reqwest(native-tls) 编译必需
pkg install -y rust clang openssl pkg-config git

echo
echo "[3/4] 获取项目源码..."
if [ -f "$PROJECT_DIR/Cargo.toml" ]; then
    echo "    已找到项目: $PROJECT_DIR"
else
    echo "    未找到项目, 从 $REPO_URL 克隆..."
    git clone "$REPO_URL" "$PROJECT_DIR"
fi

echo
echo "[4/4] 编译 release 版本（首次约 5~15 分钟, 请耐心）..."
cd "$PROJECT_DIR"
cargo build --release

echo
echo "=================================================="
echo "[+] 编译完成!"
echo "    二进制: $PROJECT_DIR/target/release/qqp2p"
echo
echo "    启动命令:"
echo "      cd $PROJECT_DIR"
echo "      ./target/release/qqp2p start --user-id 你的手机QQ号 --port 8080"
echo
echo "    手机端默认连接 127.0.0.1:3000/3001,"
echo "    即手机本机 NapCat, 无需修改任何配置。"
echo "=================================================="
