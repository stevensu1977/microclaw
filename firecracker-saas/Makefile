# MicroClaw Firecracker SaaS - 构建与部署
#
# 用法:
#   make setup          # 一次性主机配置
#   make build          # 构建内核 + rootfs
#   make build-golden   # 创建黄金快照
#   make run-control    # 启动控制平面
#   make create-tenant  # 创建租户

SHELL := /bin/bash
FC_DIR := $(shell pwd)
PROJECT_ROOT := $(shell cd .. && pwd)
BUILD_DIR := $(FC_DIR)/build

# 可覆盖的变量
FC_BIN ?= firecracker
VMLINUX ?= $(BUILD_DIR)/vmlinux
ROOTFS ?= $(BUILD_DIR)/rootfs.ext4

.PHONY: setup build build-static build-rootfs build-golden run-control create-tenant clean help

help: ## 显示帮助
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

setup: ## 一次性主机配置 (需要 root)
	sudo bash scripts/deploy.sh

build: build-static build-rootfs ## 构建静态二进制 + rootfs
	@echo "==> Build complete"
	@echo "    Binary: $(PROJECT_ROOT)/target/x86_64-unknown-linux-musl/release/microclaw"
	@echo "    Rootfs: $(ROOTFS)"

build-static: ## 静态编译 MicroClaw
	bash scripts/build-static.sh

build-rootfs: ## 构建最小化 rootfs
	bash scripts/build-rootfs.sh

build-golden: ## 创建黄金快照 (需要 root)
	sudo bash scripts/create-golden-snapshot.sh

build-control: ## 编译控制平面
	cd control-plane && cargo build --release

run-control: ## 启动控制平面 (前台运行)
	cd control-plane && \
	FC_BIN=$(FC_BIN) \
	VMLINUX_PATH=$(VMLINUX) \
	ROOTFS_PATH=$(ROOTFS) \
	DATA_DIR=/var/lib/microclaw-saas/tenants \
	BIND_ADDR=0.0.0.0:8080 \
	RUST_LOG=info \
	cargo run --release

# 创建租户 (需要设置变量)
# 用法: make create-tenant TENANT_ID=demo ANTHROPIC_API_KEY=sk-ant-...
create-tenant: ## 创建租户 (需要 TENANT_ID, ANTHROPIC_API_KEY)
ifndef TENANT_ID
	$(error TENANT_ID is required. Usage: make create-tenant TENANT_ID=demo ANTHROPIC_API_KEY=sk-...)
endif
ifndef ANTHROPIC_API_KEY
	$(error ANTHROPIC_API_KEY is required)
endif
	@echo "==> Creating tenant: $(TENANT_ID)"
	curl -s -X POST http://localhost:8080/api/v1/tenants \
		-H "Content-Type: application/json" \
		-d '{ \
			"tenant_id": "$(TENANT_ID)", \
			"tier": "$(or $(TIER),pro)", \
			"channels": ["web"], \
			"env_vars": { \
				"ANTHROPIC_API_KEY": "$(ANTHROPIC_API_KEY)" \
				$(if $(TELEGRAM_BOT_TOKEN),,"TELEGRAM_BOT_TOKEN": "$(TELEGRAM_BOT_TOKEN)") \
			} \
		}' | python3 -m json.tool 2>/dev/null || true

list-tenants: ## 列出所有租户
	curl -s http://localhost:8080/api/v1/tenants | python3 -m json.tool 2>/dev/null || true

stop-tenant: ## 停止租户 (需要 TENANT_ID)
ifndef TENANT_ID
	$(error TENANT_ID is required)
endif
	curl -s -X POST http://localhost:8080/api/v1/tenants/$(TENANT_ID)/stop | python3 -m json.tool 2>/dev/null || true

delete-tenant: ## 删除租户 (需要 TENANT_ID)
ifndef TENANT_ID
	$(error TENANT_ID is required)
endif
	curl -s -X DELETE http://localhost:8080/api/v1/tenants/$(TENANT_ID) | python3 -m json.tool 2>/dev/null || true

health: ## 控制平面健康检查
	curl -s http://localhost:8080/health | python3 -m json.tool 2>/dev/null || true

metrics: ## 查看 Prometheus 指标
	curl -s http://localhost:8080/metrics

clean: ## 清理构建产物
	rm -rf $(BUILD_DIR)
	cd control-plane && cargo clean 2>/dev/null || true
