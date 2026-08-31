#!/usr/bin/env bash
# 在 Linux 上编译 rust-bt 并安装到系统目录
#
# 用法：
#   ./install.sh                 # 编译并安装 bt 到 /usr/local/bin（自动按需 sudo）
#   ./install.sh --prefix /opt   # 安装到 /opt/bin
#   ./install.sh --build-only    # 只编译，不安装
#   PREFIX=~/.local ./install.sh # 免 sudo 安装到用户目录
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
BUILD_ONLY=0

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) PREFIX="$2"; shift 2 ;;
        --build-only) BUILD_ONLY=1; shift ;;
        -h|--help)
            sed -n '2,8p' "$0"
            exit 0 ;;
        *) echo "未知参数: $1" >&2; exit 1 ;;
    esac
done

# 必须在仓库根目录执行
cd "$(dirname "$0")"
if [ ! -f Cargo.toml ]; then
    echo "错误：未找到 Cargo.toml，请在仓库根目录运行" >&2
    exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "错误：未找到 cargo，请先安装 Rust 工具链（https://rustup.rs）" >&2
    exit 1
fi

echo "==> 编译 release 版本"
cargo build --release --bin bt

BIN="target/release/bt"
if [ ! -x "$BIN" ]; then
    echo "错误：编译产物 $BIN 不存在" >&2
    exit 1
fi

if [ "$BUILD_ONLY" -eq 1 ]; then
    echo "==> 仅编译完成：$BIN"
    exit 0
fi

DEST="$PREFIX/bin"
echo "==> 安装到 $DEST"

# 目标目录不可写时自动使用 sudo
if [ -w "$DEST" ] || { [ ! -e "$DEST" ] && [ -w "$PREFIX" ]; }; then
    SUDO=""
elif command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
else
    echo "错误：$DEST 不可写且未找到 sudo，可用 PREFIX=~/.local ./install.sh 安装到用户目录" >&2
    exit 1
fi

$SUDO mkdir -p "$DEST"
$SUDO install -m 0755 "$BIN" "$DEST/bt"

echo "==> 完成：$DEST/bt"
"$DEST/bt" --help >/dev/null 2>&1 || true
echo "提示：确保 $DEST 在 PATH 中（export PATH=\"$DEST:\$PATH\"）"
