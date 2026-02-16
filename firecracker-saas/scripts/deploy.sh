#!/bin/bash
# MicroClaw Firecracker SaaS 一键部署脚本
# 在裸金属 / KVM 主机上完成初始设置

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
FC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# ─── 颜色输出 ───────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC} $1"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# ─── 检查前置条件 ───────────────────────────────────────────────────
check_prerequisites() {
  info "Checking prerequisites..."

  # KVM 支持
  if [ ! -e /dev/kvm ]; then
    error "/dev/kvm not found. This host does not support KVM virtualization."
  fi
  info "  /dev/kvm: OK"

  # root 权限
  if [ "$EUID" -ne 0 ]; then
    error "This script must be run as root (for TAP devices and iptables)"
  fi
  info "  root: OK"

  # Firecracker 二进制
  if ! command -v firecracker &>/dev/null; then
    warn "  firecracker: not found, will install"
    install_firecracker
  else
    info "  firecracker: $(firecracker --version 2>&1 | head -1)"
  fi

  # Docker (用于构建 rootfs)
  if ! command -v docker &>/dev/null; then
    error "Docker is required for building rootfs. Install docker first."
  fi
  info "  docker: OK"

  # iptables
  if ! command -v iptables &>/dev/null; then
    error "iptables is required for network isolation."
  fi
  info "  iptables: OK"
}

# ─── 安装 Firecracker ───────────────────────────────────────────────
install_firecracker() {
  info "Installing Firecracker..."

  FC_VERSION="1.7.0"
  ARCH=$(uname -m)

  curl -sSL "https://github.com/firecracker-microvm/firecracker/releases/download/v${FC_VERSION}/firecracker-v${FC_VERSION}-${ARCH}.tgz" \
    -o /tmp/firecracker.tgz

  tar -xzf /tmp/firecracker.tgz -C /tmp
  mv "/tmp/release-v${FC_VERSION}-${ARCH}/firecracker-v${FC_VERSION}-${ARCH}" /usr/local/bin/firecracker
  chmod +x /usr/local/bin/firecracker
  rm -rf /tmp/firecracker.tgz "/tmp/release-v${FC_VERSION}-${ARCH}"

  info "Firecracker v${FC_VERSION} installed"
}

# ─── 下载 Linux 内核 ────────────────────────────────────────────────
download_kernel() {
  BUILD_DIR="$FC_DIR/build"
  mkdir -p "$BUILD_DIR"

  if [ -f "$BUILD_DIR/vmlinux" ]; then
    info "Kernel already exists at $BUILD_DIR/vmlinux"
    return
  fi

  info "Downloading pre-built kernel..."

  FC_VERSION="1.7.0"
  ARCH=$(uname -m)

  curl -sSL "https://github.com/firecracker-microvm/firecracker/releases/download/v${FC_VERSION}/firecracker-v${FC_VERSION}-${ARCH}.tgz" \
    -o /tmp/firecracker-full.tgz

  # 提取内核 (如果包含)
  # 如果发行版不包含内核，使用 Firecracker CI 的预编译内核
  KERNEL_URL="https://s3.amazonaws.com/spec.ccfc.min/ci-artifacts/kernels/${ARCH}/vmlinux-5.10.217"
  curl -sSL "$KERNEL_URL" -o "$BUILD_DIR/vmlinux" || {
    warn "Failed to download kernel from S3, trying alternative..."
    warn "Please provide a vmlinux kernel at $BUILD_DIR/vmlinux"
  }

  if [ -f "$BUILD_DIR/vmlinux" ]; then
    info "Kernel downloaded: $BUILD_DIR/vmlinux"
  fi
}

# ─── 创建数据目录 ───────────────────────────────────────────────────
setup_directories() {
  info "Creating data directories..."

  mkdir -p /var/lib/microclaw-saas/tenants
  mkdir -p /var/lib/microclaw-saas/snapshots
  mkdir -p /var/log/microclaw-saas
  mkdir -p "$FC_DIR/build/snapshots/golden"

  info "  /var/lib/microclaw-saas/tenants"
  info "  /var/lib/microclaw-saas/snapshots"
  info "  /var/log/microclaw-saas"
}

# ─── 配置 sysctl ────────────────────────────────────────────────────
setup_sysctl() {
  info "Configuring sysctl..."

  sysctl -w net.ipv4.ip_forward=1 > /dev/null
  sysctl -w net.ipv4.conf.all.forwarding=1 > /dev/null

  # 持久化
  if ! grep -q "net.ipv4.ip_forward" /etc/sysctl.conf 2>/dev/null; then
    echo "net.ipv4.ip_forward=1" >> /etc/sysctl.conf
  fi

  info "  IP forwarding enabled"
}

# ─── 配置基础 iptables ──────────────────────────────────────────────
setup_iptables() {
  info "Configuring base iptables rules..."

  HOST_IFACE=$(ip route get 8.8.8.8 | awk '{print $5; exit}')

  # NAT 出站流量
  iptables -t nat -C POSTROUTING -s 172.16.0.0/16 -o "$HOST_IFACE" -j MASQUERADE 2>/dev/null || \
    iptables -t nat -A POSTROUTING -s 172.16.0.0/16 -o "$HOST_IFACE" -j MASQUERADE

  # 租户隔离: 阻止 VM 间直接通信
  iptables -C FORWARD -i "fc-+" -o "fc-+" -j DROP 2>/dev/null || \
    iptables -A FORWARD -i "fc-+" -o "fc-+" -j DROP

  info "  NAT masquerade on $HOST_IFACE"
  info "  Inter-tenant traffic blocked"
}

# ─── 安装 systemd 服务 ──────────────────────────────────────────────
install_services() {
  info "Installing systemd services..."

  cp "$FC_DIR/systemd/microclaw-control.service" /etc/systemd/system/
  systemctl daemon-reload

  info "  microclaw-control.service installed"
  info "  (start with: systemctl start microclaw-control)"
}

# ─── 主流程 ─────────────────────────────────────────────────────────
main() {
  echo "============================================"
  echo "  MicroClaw Firecracker SaaS Deployment"
  echo "============================================"
  echo ""

  check_prerequisites
  setup_directories
  download_kernel
  setup_sysctl
  setup_iptables
  install_services

  echo ""
  info "Deployment setup complete!"
  echo ""
  echo "Next steps:"
  echo "  1. Build MicroClaw:  cd $FC_DIR && make build"
  echo "  2. Start control:    make run-control"
  echo "  3. Create tenant:    make create-tenant TENANT_ID=demo ANTHROPIC_API_KEY=sk-..."
  echo ""
}

main "$@"
