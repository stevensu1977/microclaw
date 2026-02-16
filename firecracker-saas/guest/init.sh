#!/bin/sh
# MicroClaw Firecracker microVM init (PID 1)
# This script runs as the first process inside the microVM.

# ─── 挂载虚拟文件系统 ────────────────────────────────────────────────
mount -t proc     proc     /proc   2>/dev/null || true
mount -t sysfs    sysfs    /sys    2>/dev/null || true
mount -t devtmpfs devtmpfs /dev    2>/dev/null || true
mkdir -p /dev/pts /dev/shm /tmp /run
mount -t devpts   devpts   /dev/pts 2>/dev/null || true
mount -t tmpfs    tmpfs    /dev/shm 2>/dev/null || true
mount -t tmpfs    tmpfs    /tmp     2>/dev/null || true
mount -t tmpfs    tmpfs    /run     2>/dev/null || true

# ─── 解析内核命令行参数 ──────────────────────────────────────────────
# Firecracker 通过 boot_args 传递: FC_VM_IP=... FC_TENANT_ID=...
for param in $(cat /proc/cmdline); do
  case "$param" in
    FC_VM_IP=*)       FC_VM_IP="${param#*=}" ;;
    FC_VM_GATEWAY=*)  FC_VM_GATEWAY="${param#*=}" ;;
    FC_VM_NETMASK=*)  FC_VM_NETMASK="${param#*=}" ;;
    FC_TENANT_ID=*)   FC_TENANT_ID="${param#*=}" ;;
    FC_DNS=*)         FC_DNS="${param#*=}" ;;
    FC_PORT=*)        FC_PORT="${param#*=}" ;;
  esac
done

FC_VM_IP="${FC_VM_IP:-172.16.0.2}"
FC_VM_GATEWAY="${FC_VM_GATEWAY:-172.16.0.1}"
FC_VM_NETMASK="${FC_VM_NETMASK:-30}"
FC_TENANT_ID="${FC_TENANT_ID:-unknown}"
FC_DNS="${FC_DNS:-8.8.8.8}"
FC_PORT="${FC_PORT:-8080}"

echo "[init] MicroClaw microVM starting for tenant=$FC_TENANT_ID"

# ─── 配置网络 ───────────────────────────────────────────────────────
ip link set lo up
ip link set eth0 up
ip addr add "${FC_VM_IP}/${FC_VM_NETMASK}" dev eth0
ip route add default via "$FC_VM_GATEWAY" dev eth0

echo "nameserver $FC_DNS" > /etc/resolv.conf
echo "nameserver 8.8.4.4" >> /etc/resolv.conf

echo "[init] Network: ip=$FC_VM_IP gateway=$FC_VM_GATEWAY"

# ─── 挂载数据卷 ─────────────────────────────────────────────────────
mkdir -p /data
if [ -b /dev/vdb ]; then
  mount /dev/vdb /data 2>/dev/null || {
    echo "[init] Formatting data volume..."
    mkfs.ext4 -F -L tenant-data /dev/vdb
    mount /dev/vdb /data
  }
else
  echo "[init] WARNING: No data volume, using tmpfs"
  mount -t tmpfs -o size=64m tmpfs /data
fi

mkdir -p /data/config /data/runtime /data/logs
chown -R 1000:1000 /data

# ─── 加载租户环境变量 ───────────────────────────────────────────────
if [ -f /data/config/.env ]; then
  echo "[init] Loading tenant environment"
  set -a
  . /data/config/.env
  set +a
fi

# ─── 创建默认配置 ───────────────────────────────────────────────────
if [ ! -f /data/config/config.yaml ]; then
  # Use ANTHROPIC_API_KEY from env if available, otherwise use placeholder
  # (placeholder allows MicroClaw to start for golden snapshot;
  #  real key is injected via tenant .env at runtime)
  _API_KEY="${ANTHROPIC_API_KEY:-sk-placeholder-will-be-replaced}"
  _AUTH_TOKEN="${WEB_AUTH_TOKEN:-fc-internal-token}"
  _LLM_PROVIDER="${LLM_PROVIDER:-anthropic}"
  cat > /data/config/config.yaml << YAML
data_dir: /data/runtime
working_dir: /data/workspace
log_level: info
llm_provider: "$_LLM_PROVIDER"
api_key: "$_API_KEY"

web_enabled: true
web_port: $FC_PORT
web_host: "0.0.0.0"
web_auth_token: "$_AUTH_TOKEN"
YAML

  # Optional: custom LLM base URL (e.g. proxy or third-party Anthropic-compatible endpoint)
  if [ -n "$LLM_BASE_URL" ]; then
    echo "llm_base_url: \"$LLM_BASE_URL\"" >> /data/config/config.yaml
  fi

  # Optional: LLM model override
  if [ -n "$LLM_MODEL" ]; then
    echo "model: \"$LLM_MODEL\"" >> /data/config/config.yaml
  fi

  chown 1000:1000 /data/config/config.yaml
fi

# ─── 优雅关机处理 ───────────────────────────────────────────────────
MICROCLAW_PID=""

shutdown() {
  echo "[init] Shutting down..."
  if [ -n "$MICROCLAW_PID" ] && kill -0 "$MICROCLAW_PID" 2>/dev/null; then
    kill -TERM "$MICROCLAW_PID"
    for i in $(seq 1 20); do
      kill -0 "$MICROCLAW_PID" 2>/dev/null || break
      sleep 0.5
    done
    kill -9 "$MICROCLAW_PID" 2>/dev/null || true
  fi
  sync
  umount /data 2>/dev/null || true
  echo "[init] Shutdown complete"
  reboot -f
}

trap shutdown SIGTERM SIGINT SIGHUP

# ─── 启动 MicroClaw ─────────────────────────────────────────────────
echo "[init] Starting MicroClaw on port $FC_PORT"

export HOME=/home/microclaw
export MICROCLAW_CONFIG=/data/config/config.yaml
export RUST_LOG=info

# 以非 root 用户运行
su -s /bin/sh microclaw -c "
  /usr/local/bin/microclaw start --config /data/config/config.yaml
" &
MICROCLAW_PID=$!

echo "[init] MicroClaw started (pid=$MICROCLAW_PID)"

# ─── 进程守护 ───────────────────────────────────────────────────────
while true; do
  wait "$MICROCLAW_PID" 2>/dev/null
  EXIT_CODE=$?

  if [ $EXIT_CODE -eq 0 ]; then
    echo "[init] MicroClaw exited cleanly"
    break
  fi

  echo "[init] MicroClaw crashed (code=$EXIT_CODE), restarting in 2s..."
  sleep 2

  su -s /bin/sh microclaw -c "
    /usr/local/bin/microclaw start --config /data/config/config.yaml
  " &
  MICROCLAW_PID=$!
done
