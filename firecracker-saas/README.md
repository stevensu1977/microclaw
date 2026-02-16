# MicroClaw Firecracker SaaS

基于 Firecracker microVM 的轻量级多租户部署方案，实现硬件级隔离的 SaaS 架构。

## 架构

每个租户运行在独立的 Firecracker microVM 中，通过 TAP 网络设备和 iptables 实现网络隔离。控制平面管理 VM 生命周期、子网分配和租户配置。

```
Internet → Nginx (TLS + 子域名路由) → Control Plane API
                                     → Tenant VMs (172.16.N.2:8080)
```

## 前置条件

- Linux 主机，KVM 支持 (`/dev/kvm`)
- 裸金属或支持嵌套虚拟化 (AWS `.metal`, GCP N2, Hetzner dedicated)
- Docker (构建 rootfs)
- Rust toolchain (编译 MicroClaw 和控制平面)

## 快速开始

```bash
# 1. 一次性主机配置 (安装 Firecracker, 配置网络等)
make setup

# 2. 构建静态二进制 + rootfs
make build

# 3. (可选) 创建黄金快照，加速后续启动
make build-golden

# 4. 编译并启动控制平面
make run-control

# 5. 创建租户
make create-tenant \
  TENANT_ID=demo \
  ANTHROPIC_API_KEY=sk-ant-...

# 6. 访问租户
#    Web UI: https://demo.microclaw.example.com
```

## 常用命令

```bash
make help            # 显示所有命令
make list-tenants    # 列出租户
make stop-tenant TENANT_ID=demo    # 停止租户
make delete-tenant TENANT_ID=demo  # 删除租户
make health          # 控制平面健康检查
make metrics         # Prometheus 指标
```

## 目录结构

```
firecracker-saas/
├── Makefile              # 构建与部署命令
├── guest/                # VM 内部文件
│   ├── init.sh           # PID 1 初始化脚本
│   ├── health-agent.sh   # 健康检查代理
│   └── microclaw.service # systemd 服务
├── scripts/              # 构建与部署脚本
│   ├── build-static.sh   # 静态编译 MicroClaw
│   ├── build-rootfs.sh   # 构建最小化 rootfs
│   ├── create-golden-snapshot.sh
│   ├── create-tap.sh     # 创建 TAP 网络设备
│   └── deploy.sh         # 一键部署
├── control-plane/        # Rust 控制平面
│   ├── Cargo.toml
│   └── src/
├── config/               # 配置模板
│   ├── nginx-saas.conf
│   ├── vm-config.json
│   └── tiers.yaml
└── systemd/              # systemd 服务文件
```

## 租户等级

| Tier | vCPU | 内存 | 磁盘 |
|------|------|------|------|
| free | 1 | 128MB | 128MB |
| pro | 1 | 256MB | 512MB |
| team | 2 | 512MB | 2GB |
| enterprise | 4 | 1GB | 8GB |

## API

控制平面 API 端点:

| 操作 | 方法 | 路径 |
|------|------|------|
| 创建租户 | POST | `/api/v1/tenants` |
| 列出租户 | GET | `/api/v1/tenants` |
| 获取详情 | GET | `/api/v1/tenants/{id}` |
| 启动 | POST | `/api/v1/tenants/{id}/start` |
| 停止 | POST | `/api/v1/tenants/{id}/stop` |
| 暂停 | POST | `/api/v1/tenants/{id}/pause` |
| 恢复 | POST | `/api/v1/tenants/{id}/resume` |
| 快照 | POST | `/api/v1/tenants/{id}/snapshot` |
| 更新配置 | PUT | `/api/v1/tenants/{id}/env` |
| 删除 | DELETE | `/api/v1/tenants/{id}` |
| 健康检查 | GET | `/api/v1/tenants/{id}/health` |

详细方案见 [FIRECRACKER.md](../FIRECRACKER.md)。
