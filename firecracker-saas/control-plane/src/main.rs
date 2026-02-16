mod api;
mod firecracker;
mod network;
mod proxy;
mod snapshot;
mod tenant;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

use crate::network::SubnetAllocator;
use crate::tenant::TenantManager;

pub struct AppState {
    pub tenant_manager: RwLock<TenantManager>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let fc_bin = std::env::var("FC_BIN").unwrap_or_else(|_| "firecracker".to_string());
    let vmlinux = std::env::var("VMLINUX_PATH")
        .unwrap_or_else(|_| "/var/lib/microclaw-saas/vmlinux".to_string());
    let rootfs = std::env::var("ROOTFS_PATH")
        .unwrap_or_else(|_| "/var/lib/microclaw-saas/rootfs.ext4".to_string());
    let data_dir = std::env::var("DATA_DIR")
        .unwrap_or_else(|_| "/var/lib/microclaw-saas/tenants".to_string());
    let snapshot_dir = std::env::var("SNAPSHOT_DIR")
        .unwrap_or_else(|_| "/var/lib/microclaw-saas/snapshots".to_string());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let subnet_allocator = SubnetAllocator::new("172.16.0.0/16");

    let tenant_manager =
        TenantManager::new(fc_bin, vmlinux, rootfs, data_dir, snapshot_dir, subnet_allocator);

    let state = Arc::new(AppState {
        tenant_manager: RwLock::new(tenant_manager),
    });

    let app = api::router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("Control plane listening on {}", bind_addr);

    axum::serve(listener, app).await?;

    Ok(())
}
