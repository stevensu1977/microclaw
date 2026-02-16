#!/bin/bash
# 静态编译 MicroClaw for Firecracker microVM
# 输出: target/x86_64-unknown-linux-musl/release/microclaw

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "==> Building static MicroClaw binary"

cd "$PROJECT_ROOT"

# 安装 musl 工具链
rustup target add x86_64-unknown-linux-musl

# 指定 musl C 编译器
export CC_x86_64_unknown_linux_musl=musl-gcc

# 使用 vendored OpenSSL（静态编译无法链接系统 libssl）
export OPENSSL_STATIC=1
export OPENSSL_NO_VENDOR=0

# 静态编译，优化大小
RUSTFLAGS='-C target-feature=+crt-static -C link-self-contained=yes' \
cargo build --release \
    --target x86_64-unknown-linux-musl \
    --features "openssl-vendored"

# 压缩二进制
strip target/x86_64-unknown-linux-musl/release/microclaw
upx --best target/x86_64-unknown-linux-musl/release/microclaw 2>/dev/null || true

# 输出大小
echo "==> Build complete:"
ls -lh target/x86_64-unknown-linux-musl/release/microclaw
