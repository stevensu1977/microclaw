# MicroClaw Firecracker 部署方案

基于 Firecracker microVM 的轻量级多租户部署方案，实现硬件级隔离的 SaaS 架构。

---

## 一、架构概览

```
                         ┌─────────────────────────────────────────────┐
                         │           Internet / Clients                │
                         └────────────────────┬────────────────────────┘
                                              │
                         ┌────────────────────▼────────────────────────┐
                         │   Nginx (TLS 终止 + 子域名路由)              │
                         │   *.microclaw.example.com                   │
                         └──────────┬─────────────────────┬────────────┘
                                    │                     │
                    ┌───────────────▼──────────┐   ┌──────▼───────────────┐
                    │   Control Plane API      │   │   Tenant Traffic     │
                    │   api.microclaw.example  │   │   {id}.microclaw...  │
                    │   (Rust/Python :8080)    │   │                      │
                    │                          │   │   Reverse proxy to   │
                    │   - Tenant CRUD          │   │   VM 172.16.N.2:8080 │
                    │   - VM 生命周期           │   │                      │
                    │   - 健康检查             │   │   WebSocket proxy    │
                    │   - Metrics              │   │                      │
                    └───────────┬──────────────┘   └──────────────────────┘
                                │
              ┌─────────────────▼──────────────────────────────────────┐
              │                  Tenant Manager                        │
              │   (子网分配, TAP 设备, iptables, Firecracker API)      │
              └────┬──────────────────┬──────────────────┬─────────────┘
                   │                  │                  │
         ┌─────────▼──────┐  ┌───────▼────────┐  ┌──────▼─────────┐
         │ tap: fc-tenant1│  │ tap: fc-tenant2│  │ tap: fc-tenant3│
         │ 172.16.1.0/30  │  │ 172.16.2.0/30  │  │ 172.16.3.0/30  │
         │                │  │                │  │                │
         │ ┌────────────┐ │  │ ┌────────────┐ │  │ ┌────────────┐ │
         │ │  microVM   │ │  │ │  microVM   │ │  │ │  microVM   │ │
         │ │  Tenant A  │ │  │ │  Tenant B  │ │  │ │  Tenant C  │ │
         │ │            │ │  │ │            │ │  │ │            │ │
         │ │ MicroClaw  │ │  │ │ MicroClaw  │ │  │ │ MicroClaw  │ │
         │ │  (Rust)    │ │  │ │  (Rust)    │ │  │ │  (Rust)    │ │
         │ │  :8080     │ │  │ │  :8080     │ │  │ │  :8080     │ │
         │ │            │ │  │ │            │ │  │ │            │ │
         │ │  /data     │ │  │ │  /data     │ │  │ │  /data     │ │
         │ │  (ext4)    │ │  │ │  (ext4)    │ │  │ │  (ext4)    │ │
         │ └────────────┘ │  │ └────────────┘ │  │ └────────────┘ │
         └────────────────┘  └────────────────┘  └────────────────┘
```

---

## 二、MicroClaw vs Node.js 对比优势

| 指标 | openclaw (Node.js) | MicroClaw (Rust) | 优势 |
|------|-------------------|------------------|------|
| 二进制大小 | ~150MB | ~15-20MB | **7-10x 更小** |
| 内存基线 | ~80-150MB | ~10-30MB | **5-8x 更省** |
| 冷启动时间 | ~5-8s | ~0.5-1s | **5-10x 更快** |
| rootfs 大小 | ~500MB+ | ~50-80MB | **6-10x 更小** |
| Snapshot 恢复 | ~125-200ms | ~125-200ms | 相当 |
| 单机租户密度 | ~100 (512MB tier) | ~400+ (128MB tier) | **4x 更多** |

### 资源配置建议

```yaml
# Rust 版本可以使用更小的资源配额
tiers:
  free:
    vcpu: 1
    memory_mb: 128      # Node.js 需要 256MB
    disk_mb: 128
    network_mbps: 5

  pro:
    vcpu: 1
    memory_mb: 256      # Node.js 需要 512MB
    disk_mb: 512
    network_mbps: 20

  team:
    vcpu: 2
    memory_mb: 512      # Node.js 需要 1GB
    disk_mb: 2048
    network_mbps: 50

  enterprise:
    vcpu: 4
    memory_mb: 1024     # Node.js 需要 2GB
    disk_mb: 8192
    network_mbps: 100
```

---

## 三、目录结构

```
firecracker/
├── Makefile                              # 构建与部署命令
├── README.md                             # 快速开始指南
│
├── guest/                                # VM 内部文件
│   ├── init.sh                           # PID 1 初始化脚本
│   ├── microclaw.service                 # systemd 服务 (可选)
│   └── health-agent.sh                   # 健康检查代理
│
├── scripts/
│   ├── build-rootfs.sh                   # 构建最小化 rootfs
│   ├── build-static.sh                   # 静态编译 MicroClaw
│   ├── create-golden-snapshot.sh         # 创建黄金快照
│   ├── create-tap.sh                     # 创建 TAP 网络设备
│   └── deploy.sh                         # 一键部署脚本
│
├── control-plane/                        # SaaS 控制平面
│   ├── Cargo.toml                        # Rust 实现 (可选)
│   ├── src/
│   │   ├── main.rs
│   │   ├── api.rs                        # HTTP API
│   │   ├── tenant.rs                     # 租户管理
│   │   ├── network.rs                    # 网络分配
│   │   ├── firecracker.rs                # FC API 客户端
│   │   └── snapshot.rs                   # 快照管理
│   │
│   └── python/                           # Python 实现 (备选)
│       ├── requirements.txt
│       ├── api_gateway.py
│       └── tenant_manager.py
│
├── config/
│   ├── nginx-saas.conf                   # Nginx 配置模板
│   ├── vm-config.json                    # Firecracker VM 配置模板
│   └── tiers.yaml                        # 租户等级配置
│
└── systemd/
    ├── microclaw-control.service         # 控制平面服务
    └── microclaw-nginx.service           # Nginx 服务
```

---

## 四、构建流程

### 4.1 静态编译 MicroClaw

```bash
#!/bin/bash
# scripts/build-static.sh

set -e

# 安装 musl 工具链
rustup target add x86_64-unknown-linux-musl

# 静态编译，优化大小
RUSTFLAGS='-C target-feature=+crt-static -C link-self-contained=yes' \
cargo build --release \
    --target x86_64-unknown-linux-musl \
    --features "bundled-sqlite"

# 压缩二进制
strip target/x86_64-unknown-linux-musl/release/microclaw
upx --best target/x86_64-unknown-linux-musl/release/microclaw || true

# 输出大小
ls -lh target/x86_64-unknown-linux-musl/release/microclaw
# 预期: ~10-15MB
```

### 4.2 构建最小化 rootfs

```bash
#!/bin/bash
# scripts/build-rootfs.sh

set -e

ROOTFS_SIZE_MB="${ROOTFS_SIZE_MB:-128}"
OUTPUT="${OUTPUT:-build/rootfs.ext4}"
MICROCLAW_BIN="${MICROCLAW_BIN:-target/x86_64-unknown-linux-musl/release/microclaw}"

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

USER microclaw
WORKDIR /app

ENTRYPOINT ["/sbin/tini", "--"]
CMD ["/usr/local/bin/microclaw", "start"]
EOF

# 复制文件
cp "$MICROCLAW_BIN" "$TMPDIR/microclaw"
cp guest/init.sh "$TMPDIR/init.sh"

# 构建 Docker 镜像
docker build -t microclaw-fc:latest "$TMPDIR"

# 导出为 rootfs
CONTAINER_ID=$(docker create microclaw-fc:latest)
docker export "$CONTAINER_ID" | tar -C "$TMPDIR/rootfs" -xf -
docker rm "$CONTAINER_ID"

# 创建 ext4 镜像
dd if=/dev/zero of="$OUTPUT" bs=1M count="$ROOTFS_SIZE_MB"
mkfs.ext4 -F -L rootfs "$OUTPUT"

# 挂载并复制
MOUNT_DIR=$(mktemp -d)
sudo mount -o loop "$OUTPUT" "$MOUNT_DIR"
sudo cp -a "$TMPDIR/rootfs/." "$MOUNT_DIR/"
sudo umount "$MOUNT_DIR"
rmdir "$MOUNT_DIR"

echo "Rootfs created: $OUTPUT ($(du -h $OUTPUT | cut -f1))"
```

### 4.3 VM 内部 init 脚本

```bash
#!/bin/bash
# guest/init.sh - MicroClaw Firecracker microVM init (PID 1)

set -e

# ─── 挂载虚拟文件系统 ────────────────────────────────────────────────
mount -t proc     proc     /proc
mount -t sysfs    sysfs    /sys
mount -t devtmpfs devtmpfs /dev
mkdir -p /dev/pts /dev/shm /tmp /run
mount -t devpts   devpts   /dev/pts
mount -t tmpfs    tmpfs    /dev/shm
mount -t tmpfs    tmpfs    /tmp
mount -t tmpfs    tmpfs    /run

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
  cat > /data/config/config.yaml << YAML
data_dir: /data/runtime
working_dir: /data/workspace
log_level: info

# Web UI (网关模式)
web_enabled: true
web_port: $FC_PORT
web_bind: "0.0.0.0"

# 从环境变量加载 API keys
# ANTHROPIC_API_KEY, OPENAI_API_KEY 等
YAML
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
```

---

## 五、控制平面 API

### 5.1 租户生命周期

| 操作 | API | 说明 |
|------|-----|------|
| 创建租户 | `POST /api/v1/tenants` | 分配子网，创建 VM，启动 MicroClaw |
| 列出租户 | `GET /api/v1/tenants` | 获取所有租户状态 |
| 获取详情 | `GET /api/v1/tenants/{id}` | 获取单个租户详情 |
| 启动 | `POST /api/v1/tenants/{id}/start` | 从停止状态启动 |
| 停止 | `POST /api/v1/tenants/{id}/stop` | 优雅关机，保留数据 |
| 暂停 | `POST /api/v1/tenants/{id}/pause` | 冻结 VM (保留内存) |
| 恢复 | `POST /api/v1/tenants/{id}/resume` | 从暂停状态恢复 |
| 快照 | `POST /api/v1/tenants/{id}/snapshot` | 创建完整快照 |
| 更新配置 | `PUT /api/v1/tenants/{id}/env` | 更新 API keys |
| 删除 | `DELETE /api/v1/tenants/{id}` | 删除 VM 和所有数据 |
| 健康检查 | `GET /api/v1/tenants/{id}/health` | VM 和 MicroClaw 状态 |

### 5.2 创建租户请求示例

```bash
curl -X POST http://localhost:8080/api/v1/tenants \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "tenant_id": "acme-corp",
    "tier": "pro",
    "channels": ["telegram", "web"],
    "env_vars": {
      "ANTHROPIC_API_KEY": "sk-ant-...",
      "TELEGRAM_BOT_TOKEN": "123456:ABC..."
    }
  }'
```

响应：
```json
{
  "tenant_id": "acme-corp",
  "status": "running",
  "tier": "pro",
  "vm": {
    "ip": "172.16.42.2",
    "vcpu": 1,
    "memory_mb": 256
  },
  "endpoints": {
    "web": "https://acme-corp.microclaw.example.com",
    "api": "https://acme-corp.microclaw.example.com/api"
  },
  "created_at": "2024-02-16T12:00:00Z"
}
```

### 5.3 Rust 控制平面实现 (简化)

```rust
// control-plane/src/tenant.rs

use std::collections::HashMap;
use std::process::Command;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: String,
    pub tier: Tier,
    pub status: TenantStatus,
    pub vm_ip: String,
    pub vm_pid: Option<u32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Tier {
    Free,
    Pro,
    Team,
    Enterprise,
}

impl Tier {
    pub fn vcpu(&self) -> u32 {
        match self {
            Tier::Free => 1,
            Tier::Pro => 1,
            Tier::Team => 2,
            Tier::Enterprise => 4,
        }
    }

    pub fn memory_mb(&self) -> u32 {
        match self {
            Tier::Free => 128,
            Tier::Pro => 256,
            Tier::Team => 512,
            Tier::Enterprise => 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TenantStatus {
    Creating,
    Running,
    Stopped,
    Paused,
    Failed,
}

pub struct TenantManager {
    tenants: HashMap<String, Tenant>,
    subnet_allocator: SubnetAllocator,
    fc_bin: String,
    vmlinux: String,
    rootfs: String,
}

impl TenantManager {
    pub async fn create_tenant(&mut self, req: CreateTenantRequest) -> Result<Tenant, Error> {
        // 1. 分配子网
        let subnet = self.subnet_allocator.allocate()?;
        let vm_ip = format!("{}.2", subnet.network());
        let gateway_ip = format!("{}.1", subnet.network());

        // 2. 创建 TAP 设备
        create_tap_device(&req.tenant_id, &gateway_ip)?;

        // 3. 创建数据卷
        let data_vol = create_data_volume(&req.tenant_id, req.tier.disk_mb())?;

        // 4. 写入环境变量
        write_tenant_env(&data_vol, &req.env_vars)?;

        // 5. 启动 Firecracker VM
        let vm_pid = start_firecracker_vm(FirecrackerConfig {
            tenant_id: &req.tenant_id,
            vmlinux: &self.vmlinux,
            rootfs: &self.rootfs,
            data_vol: &data_vol,
            vcpu: req.tier.vcpu(),
            memory_mb: req.tier.memory_mb(),
            vm_ip: &vm_ip,
            gateway_ip: &gateway_ip,
            tap_device: &format!("fc-{}", req.tenant_id),
        })?;

        let tenant = Tenant {
            id: req.tenant_id,
            tier: req.tier,
            status: TenantStatus::Running,
            vm_ip,
            vm_pid: Some(vm_pid),
            created_at: chrono::Utc::now(),
        };

        self.tenants.insert(tenant.id.clone(), tenant.clone());
        Ok(tenant)
    }
}
```

---

## 六、网络配置

### 6.1 子网分配

每个租户分配独立的 /30 子网（4 个 IP，2 个可用）：

```
租户 A: 172.16.1.0/30
  - 172.16.1.1 = Host (TAP gateway)
  - 172.16.1.2 = VM

租户 B: 172.16.2.0/30
  - 172.16.2.1 = Host (TAP gateway)
  - 172.16.2.2 = VM
```

### 6.2 TAP 设备创建

```bash
#!/bin/bash
# scripts/create-tap.sh

TENANT_ID="$1"
GATEWAY_IP="$2"
TAP_NAME="fc-${TENANT_ID:0:11}"  # 限制 15 字符

# 创建 TAP 设备
ip tuntap add dev "$TAP_NAME" mode tap
ip addr add "$GATEWAY_IP/30" dev "$TAP_NAME"
ip link set "$TAP_NAME" up

# 启用 NAT (允许 VM 访问外网)
iptables -t nat -A POSTROUTING -s "${GATEWAY_IP%.*}.0/30" -o eth0 -j MASQUERADE
iptables -A FORWARD -i "$TAP_NAME" -o eth0 -j ACCEPT
iptables -A FORWARD -i eth0 -o "$TAP_NAME" -m state --state RELATED,ESTABLISHED -j ACCEPT

# 阻止租户间通信
iptables -A FORWARD -i "fc-+" -o "fc-+" -j DROP

echo "TAP device $TAP_NAME created with gateway $GATEWAY_IP"
```

### 6.3 iptables 安全规则

```bash
# 租户隔离：阻止 VM 间直接通信
iptables -A FORWARD -i fc-+ -o fc-+ -j DROP

# 允许 VM 访问外网
iptables -A FORWARD -i fc-+ -o eth0 -j ACCEPT
iptables -A FORWARD -i eth0 -o fc-+ -m state --state RELATED,ESTABLISHED -j ACCEPT

# NAT 出站流量
iptables -t nat -A POSTROUTING -s 172.16.0.0/16 -o eth0 -j MASQUERADE

# 限制 VM 只能访问特定端口 (可选)
iptables -A FORWARD -i fc-+ -o eth0 -p tcp --dport 443 -j ACCEPT
iptables -A FORWARD -i fc-+ -o eth0 -p tcp --dport 80 -j ACCEPT
iptables -A FORWARD -i fc-+ -o eth0 -j DROP
```

---

## 七、快照与快速启动

### 7.1 黄金快照

预热一个标准 VM，在 MicroClaw 完成初始化后创建快照：

```bash
#!/bin/bash
# scripts/create-golden-snapshot.sh

# 1. 启动临时 VM
start_temp_vm

# 2. 等待 MicroClaw 就绪
wait_for_health "http://172.16.0.2:8080/health"

# 3. 暂停 VM
curl --unix-socket /tmp/fc-golden.sock -X PATCH \
  -H "Content-Type: application/json" \
  -d '{"state": "Paused"}' \
  "http://localhost/vm"

# 4. 创建快照
curl --unix-socket /tmp/fc-golden.sock -X PUT \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot_type": "Full",
    "snapshot_path": "build/snapshots/golden/vm.snap",
    "mem_file_path": "build/snapshots/golden/vm.mem"
  }' \
  "http://localhost/snapshot/create"

# 5. 停止临时 VM
kill_vm
```

### 7.2 启动时间对比

| 启动方式 | 时间 | 说明 |
|----------|------|------|
| 冷启动 (Node.js) | ~5-8s | 内核 + init + Node.js + 应用 |
| 冷启动 (Rust) | ~0.5-1s | 内核 + init + Rust 二进制 |
| 快照恢复 | ~125-200ms | 直接恢复内存状态 |

---

## 八、监控与健康检查

### 8.1 健康检查端点

VM 内部代理定期报告状态：

```bash
# guest/health-agent.sh
while true; do
  # 检查 MicroClaw 进程
  if pgrep -x microclaw > /dev/null; then
    STATUS="healthy"
  else
    STATUS="unhealthy"
  fi

  # 收集指标
  MEMORY=$(free -m | awk '/Mem:/ {print $3}')
  CPU=$(cat /proc/loadavg | awk '{print $1}')

  # 通过 vsock 或 HTTP 报告给 host
  echo "{\"status\":\"$STATUS\",\"memory_mb\":$MEMORY,\"load\":$CPU}"

  sleep 10
done
```

### 8.2 Prometheus 指标

控制平面暴露 `/metrics` 端点：

```
# 租户统计
microclaw_tenants_total 42
microclaw_tenants_by_status{status="running"} 38
microclaw_tenants_by_status{status="stopped"} 4

# 资源使用
microclaw_tenant_memory_bytes{tenant="acme"} 134217728
microclaw_tenant_cpu_seconds_total{tenant="acme"} 3600

# 请求统计
microclaw_tenant_requests_total{tenant="acme"} 12345
microclaw_tenant_messages_total{tenant="acme"} 567
```

---

## 九、部署检查清单

### 9.1 主机要求

- [ ] Linux 主机，KVM 支持 (`/dev/kvm`)
- [ ] 裸金属或支持嵌套虚拟化 (AWS `.metal`, GCP N2, Hetzner dedicated)
- [ ] 内存 ≥ 16GB (生产建议 64GB+)
- [ ] SSD 存储

### 9.2 软件依赖

- [ ] Firecracker ≥ 1.7
- [ ] Linux kernel (预编译或自定义)
- [ ] iptables / nftables
- [ ] Nginx (反向代理)

### 9.3 安全配置

- [ ] 设置真实的 `ADMIN_TOKEN`
- [ ] 配置 TLS 证书 (`*.microclaw.example.com`)
- [ ] 启用 iptables 租户隔离规则
- [ ] 限制控制平面访问 (防火墙/VPN)

### 9.4 运维配置

- [ ] 持久化存储 (`/var/lib/microclaw-saas/tenants`)
- [ ] 日志轮转 (`/var/log/microclaw-saas/`)
- [ ] 租户数据备份策略
- [ ] 监控告警 (Prometheus + Grafana)

---

## 十、容量规划

### 64GB RAM / 32 vCPU 主机

| Tier | 内存 | 最大租户数 | 活跃租户数 (80% 利用率) |
|------|------|-----------|----------------------|
| free | 128MB | ~400 | ~320 |
| pro | 256MB | ~200 | ~160 |
| team | 512MB | ~100 | ~80 |
| enterprise | 1GB | ~50 | ~40 |

**注**：实际密度取决于工作负载。空闲租户可暂停以释放资源。

---

## 十一、快速开始

```bash
# 1. 克隆并进入目录
cd /path/to/microclaw/firecracker

# 2. 一次性主机配置
make setup

# 3. 构建内核 + rootfs
make build

# 4. (可选) 创建黄金快照
make build-golden-snapshot

# 5. 启动控制平面
make run-control

# 6. 创建租户
make create-tenant \
  TENANT_ID=demo \
  ANTHROPIC_API_KEY=sk-ant-... \
  TELEGRAM_BOT_TOKEN=123456:ABC...

# 7. 访问
# Web UI: https://demo.microclaw.example.com
# API: https://demo.microclaw.example.com/api
```

---

## 十二、后续规划

1. **Phase 1**: 基础框架
   - [ ] 静态编译脚本
   - [ ] 最小化 rootfs 构建
   - [ ] 单租户部署脚本

2. **Phase 2**: 多租户支持
   - [ ] 控制平面 API
   - [ ] 网络隔离
   - [ ] 租户生命周期管理

3. **Phase 3**: 生产就绪
   - [ ] 黄金快照 + 快速启动
   - [ ] 监控 + 告警
   - [ ] 自动扩缩容

4. **Phase 4**: 高级特性
   - [ ] 租户迁移 (跨主机)
   - [ ] 计费集成
   - [ ] 多区域部署
