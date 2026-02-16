#!/bin/bash
# 构建最小化 rootfs for Firecracker microVM
# 基于 Alpine Linux，打包 MicroClaw 静态二进制
#
# 用法: ROOTFS_SIZE_MB=128 MICROCLAW_BIN=path/to/bin ./build-rootfs.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
FC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT="$(cd "$FC_DIR/.." && pwd)"

ROOTFS_SIZE_MB="${ROOTFS_SIZE_MB:-128}"
BUILD_DIR="${BUILD_DIR:-$FC_DIR/build}"
OUTPUT="${OUTPUT:-$BUILD_DIR/rootfs.ext4}"
MICROCLAW_BIN="${MICROCLAW_BIN:-$PROJECT_ROOT/target/x86_64-unknown-linux-musl/release/microclaw}"

mkdir -p "$BUILD_DIR"

if [ ! -f "$MICROCLAW_BIN" ]; then
  echo "ERROR: MicroClaw binary not found at $MICROCLAW_BIN"
  echo "Run scripts/build-static.sh first."
  exit 1
fi

echo "==> Building rootfs (size=${ROOTFS_SIZE_MB}MB)"

# 创建临时目录
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

# 基于 Alpine 构建最小系统
cat > "$TMPDIR/Dockerfile" << 'EOF'
FROM alpine:3.19 AS base

# 最小化依赖
RUN apk add --no-cache \
    ca-certificates \
    tzdata \
    tini

# 创建用户
RUN adduser -D -u 1000 microclaw

# 目录结构
RUN mkdir -p /data /app /var/log/microclaw
RUN chown -R microclaw:microclaw /data /app /var/log/microclaw

# 复制二进制
COPY microclaw /usr/local/bin/microclaw
RUN chmod +x /usr/local/bin/microclaw

# 复制 init 脚本
COPY init.sh /init
RUN chmod +x /init

# 复制健康检查代理
COPY health-agent.sh /usr/local/bin/health-agent
RUN chmod +x /usr/local/bin/health-agent

USER microclaw
WORKDIR /app

ENTRYPOINT ["/sbin/tini", "--"]
CMD ["/usr/local/bin/microclaw", "start"]
EOF

# 复制文件到构建上下文
cp "$MICROCLAW_BIN" "$TMPDIR/microclaw"
cp "$FC_DIR/guest/init.sh" "$TMPDIR/init.sh"
cp "$FC_DIR/guest/health-agent.sh" "$TMPDIR/health-agent.sh"

# 构建 Docker 镜像
echo "==> Building Docker image..."
docker build -t microclaw-fc:latest "$TMPDIR"

# 导出为 rootfs
echo "==> Exporting rootfs..."
mkdir -p "$TMPDIR/rootfs"
CONTAINER_ID=$(docker create microclaw-fc:latest)
docker export "$CONTAINER_ID" | tar -C "$TMPDIR/rootfs" -xf -
docker rm "$CONTAINER_ID"

# 创建 ext4 镜像
echo "==> Creating ext4 image ($OUTPUT)..."
dd if=/dev/zero of="$OUTPUT" bs=1M count="$ROOTFS_SIZE_MB"
mkfs.ext4 -F -L rootfs "$OUTPUT"

# 挂载并复制
MOUNT_DIR=$(mktemp -d)
sudo mount -o loop "$OUTPUT" "$MOUNT_DIR"
sudo cp -a "$TMPDIR/rootfs/." "$MOUNT_DIR/"
sudo umount "$MOUNT_DIR"
rmdir "$MOUNT_DIR"

echo "==> Rootfs created: $OUTPUT ($(du -h "$OUTPUT" | cut -f1))"
