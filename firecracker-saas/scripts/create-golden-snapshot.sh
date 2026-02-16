#!/bin/bash
# 创建 MicroClaw 黄金快照 (Golden Snapshot)
# 预热一个标准 VM，在 MicroClaw 完成初始化后创建快照，
# 后续租户可从快照恢复，实现 ~125ms 启动。

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
FC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$FC_DIR/build}"

FC_BIN="${FC_BIN:-firecracker}"
VMLINUX="${VMLINUX:-$BUILD_DIR/vmlinux}"
ROOTFS="${ROOTFS:-$BUILD_DIR/rootfs.ext4}"
SNAPSHOT_DIR="${SNAPSHOT_DIR:-$BUILD_DIR/snapshots/golden}"
SOCKET="/tmp/fc-golden.sock"
API_URL="http://localhost"

GOLDEN_IP="172.16.0.2"
GOLDEN_GW="172.16.0.1"
GOLDEN_TAP="fc-golden"

mkdir -p "$SNAPSHOT_DIR"

# ─── 清理旧资源 ─────────────────────────────────────────────────────
cleanup() {
  echo "==> Cleaning up..."
  rm -f "$SOCKET"
  ip link del "$GOLDEN_TAP" 2>/dev/null || true
}
trap cleanup EXIT

# ─── 创建临时 TAP 设备 ──────────────────────────────────────────────
echo "==> Creating temporary TAP device..."
ip tuntap add dev "$GOLDEN_TAP" mode tap
ip addr add "$GOLDEN_GW/30" dev "$GOLDEN_TAP"
ip link set "$GOLDEN_TAP" up

# ─── 创建临时 rootfs 副本 ───────────────────────────────────────────
GOLDEN_ROOTFS="$BUILD_DIR/golden-rootfs.ext4"
cp "$ROOTFS" "$GOLDEN_ROOTFS"

# ─── 启动 Firecracker ───────────────────────────────────────────────
echo "==> Starting Firecracker for golden snapshot..."
rm -f "$SOCKET"
$FC_BIN --api-sock "$SOCKET" &
FC_PID=$!
sleep 0.5

# 设置内核
curl --unix-socket "$SOCKET" -s -X PUT \
  -H "Content-Type: application/json" \
  -d "{
    \"kernel_image_path\": \"$VMLINUX\",
    \"boot_args\": \"init=/init console=ttyS0 reboot=k panic=1 pci=off FC_VM_IP=$GOLDEN_IP FC_VM_GATEWAY=$GOLDEN_GW FC_TENANT_ID=golden\"
  }" \
  "$API_URL/boot-source"

# 设置 rootfs
curl --unix-socket "$SOCKET" -s -X PUT \
  -H "Content-Type: application/json" \
  -d "{
    \"drive_id\": \"rootfs\",
    \"path_on_host\": \"$GOLDEN_ROOTFS\",
    \"is_root_device\": true,
    \"is_read_only\": false
  }" \
  "$API_URL/drives/rootfs"

# 设置机器配置
curl --unix-socket "$SOCKET" -s -X PUT \
  -H "Content-Type: application/json" \
  -d '{
    "vcpu_count": 1,
    "mem_size_mib": 128
  }' \
  "$API_URL/machine-config"

# 设置网络
curl --unix-socket "$SOCKET" -s -X PUT \
  -H "Content-Type: application/json" \
  -d "{
    \"iface_id\": \"eth0\",
    \"guest_mac\": \"06:00:AC:10:00:02\",
    \"host_dev_name\": \"$GOLDEN_TAP\"
  }" \
  "$API_URL/network-interfaces/eth0"

# 启动 VM
curl --unix-socket "$SOCKET" -s -X PUT \
  -H "Content-Type: application/json" \
  -d '{"action_type": "InstanceStart"}' \
  "$API_URL/actions"

echo "==> VM started, waiting for MicroClaw to initialize..."

# ─── 等待 MicroClaw 就绪 ────────────────────────────────────────────
MAX_WAIT=30
for i in $(seq 1 $MAX_WAIT); do
  if curl -s --connect-timeout 1 "http://$GOLDEN_IP:8080/health" >/dev/null 2>&1; then
    echo "==> MicroClaw is ready (after ${i}s)"
    break
  fi
  if [ "$i" -eq "$MAX_WAIT" ]; then
    echo "ERROR: MicroClaw did not become ready within ${MAX_WAIT}s"
    kill "$FC_PID" 2>/dev/null
    exit 1
  fi
  sleep 1
done

# ─── 暂停 VM ────────────────────────────────────────────────────────
echo "==> Pausing VM..."
curl --unix-socket "$SOCKET" -s -X PATCH \
  -H "Content-Type: application/json" \
  -d '{"state": "Paused"}' \
  "$API_URL/vm"

# ─── 创建快照 ───────────────────────────────────────────────────────
echo "==> Creating snapshot..."
curl --unix-socket "$SOCKET" -s -X PUT \
  -H "Content-Type: application/json" \
  -d "{
    \"snapshot_type\": \"Full\",
    \"snapshot_path\": \"$SNAPSHOT_DIR/vm.snap\",
    \"mem_file_path\": \"$SNAPSHOT_DIR/vm.mem\"
  }" \
  "$API_URL/snapshot/create"

# ─── 停止 VM ────────────────────────────────────────────────────────
kill "$FC_PID" 2>/dev/null || true
wait "$FC_PID" 2>/dev/null || true

# 清理临时 rootfs
rm -f "$GOLDEN_ROOTFS"

echo "==> Golden snapshot created:"
ls -lh "$SNAPSHOT_DIR/"
echo ""
echo "Snapshot files:"
echo "  VM state:  $SNAPSHOT_DIR/vm.snap"
echo "  Memory:    $SNAPSHOT_DIR/vm.mem"
