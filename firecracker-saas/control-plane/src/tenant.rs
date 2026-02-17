use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::firecracker::FirecrackerClient;
use crate::network::SubnetAllocator;
use crate::snapshot::SnapshotManager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: String,
    pub tier: Tier,
    pub status: TenantStatus,
    pub vm_ip: String,
    pub gateway_ip: String,
    pub tap_device: String,
    pub socket_path: String,
    pub data_dir: String,
    pub vm_pid: Option<u32>,
    pub channels: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When true, the MicroClaw instance inside the VM skips the approval loop
    /// for high-risk tools (e.g. bash). Only enable for trusted tenants.
    #[serde(default)]
    pub skip_tool_approval: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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

    pub fn disk_mb(&self) -> u32 {
        match self {
            Tier::Free => 128,
            Tier::Pro => 512,
            Tier::Team => 2048,
            Tier::Enterprise => 8192,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TenantStatus {
    Creating,
    Running,
    Stopped,
    Paused,
    Failed,
}

pub struct CreateTenantRequest {
    pub tenant_id: String,
    pub tier: Tier,
    pub channels: Vec<String>,
    pub env_vars: HashMap<String, String>,
    pub skip_tool_approval: bool,
}

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub vm_status: String,
    pub microclaw_status: String,
    pub uptime_s: Option<u64>,
}

pub struct TenantManager {
    tenants: HashMap<String, Tenant>,
    subnet_allocator: SubnetAllocator,
    snapshot_manager: SnapshotManager,
    db: Arc<Database>,
    fc_bin: String,
    vmlinux: String,
    rootfs: String,
    data_dir: String,
}

impl TenantManager {
    pub fn new(
        fc_bin: String,
        vmlinux: String,
        rootfs: String,
        data_dir: String,
        snapshot_dir: String,
        subnet_allocator: SubnetAllocator,
        db: Arc<Database>,
    ) -> Self {
        let snapshot_manager = SnapshotManager::new(fc_bin.clone(), snapshot_dir);
        Self {
            tenants: HashMap::new(),
            subnet_allocator,
            snapshot_manager,
            db,
            fc_bin,
            vmlinux,
            rootfs,
            data_dir,
        }
    }

    /// Recover tenant state from SQLite on startup.
    /// Loads all persisted tenants, rebuilds the SubnetAllocator, and reconciles
    /// against actual system state (checks if VM processes are still alive).
    pub fn recover(&mut self) {
        let tenants = match self.db.load_all_tenants() {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Failed to load tenants from DB: {}", e);
                return;
            }
        };

        // Restore subnet allocator next_index
        match self.db.get_subnet_next_index() {
            Ok(idx) => self.subnet_allocator.set_next_index(idx),
            Err(e) => tracing::warn!("Failed to load subnet_next_index from DB: {}", e),
        }

        let count = tenants.len();
        for mut tenant in tenants {
            // Rebuild subnet allocation from vm_ip (parse 172.16.{index}.2)
            if let Some(index) = parse_subnet_index(&tenant.vm_ip) {
                self.subnet_allocator
                    .restore_allocation(&tenant.id, index);
            }

            // Reconcile: check if VM process is actually alive
            match tenant.status {
                TenantStatus::Running | TenantStatus::Paused => {
                    if let Some(pid) = tenant.vm_pid {
                        if !process_alive(pid) {
                            tracing::warn!(
                                "Tenant '{}' was {:?} but VM process {} is dead, marking Stopped",
                                tenant.id,
                                tenant.status,
                                pid
                            );
                            tenant.status = TenantStatus::Stopped;
                            tenant.vm_pid = None;
                            let _ = self.db.update_tenant_status(
                                &tenant.id,
                                TenantStatus::Stopped,
                                None,
                            );
                        }
                    } else {
                        // No PID recorded but status says running — mark stopped
                        tracing::warn!(
                            "Tenant '{}' was {:?} but has no VM PID, marking Stopped",
                            tenant.id,
                            tenant.status
                        );
                        tenant.status = TenantStatus::Stopped;
                        let _ = self.db.update_tenant_status(
                            &tenant.id,
                            TenantStatus::Stopped,
                            None,
                        );
                    }
                }
                TenantStatus::Creating => {
                    // Incomplete provisioning from a previous crash
                    tracing::warn!(
                        "Tenant '{}' was in Creating state, marking Failed",
                        tenant.id
                    );
                    tenant.status = TenantStatus::Failed;
                    let _ = self.db.update_tenant_status(
                        &tenant.id,
                        TenantStatus::Failed,
                        None,
                    );
                }
                _ => {}
            }

            self.tenants.insert(tenant.id.clone(), tenant);
        }

        if count > 0 {
            tracing::info!("Recovered {} tenant(s) from database", count);
        }
    }

    pub async fn create_tenant(&mut self, req: CreateTenantRequest) -> Result<Tenant> {
        if self.tenants.contains_key(&req.tenant_id) {
            bail!("tenant '{}' already exists", req.tenant_id);
        }

        // 1. 分配子网
        let (gateway_ip, vm_ip) = self.subnet_allocator.allocate(&req.tenant_id)?;
        let tap_device = format!("fc-{}", &req.tenant_id[..req.tenant_id.len().min(11)]);
        let socket_path = format!("/tmp/fc-{}.sock", req.tenant_id);
        let tenant_data_dir = format!("{}/{}", self.data_dir, req.tenant_id);

        // Run provisioning steps with rollback on failure
        match self
            .provision_tenant(
                &req,
                &gateway_ip,
                &vm_ip,
                &tap_device,
                &socket_path,
                &tenant_data_dir,
            )
            .await
        {
            Ok(vm_pid) => {
                let tenant = Tenant {
                    id: req.tenant_id.clone(),
                    tier: req.tier,
                    status: TenantStatus::Running,
                    vm_ip,
                    gateway_ip,
                    tap_device,
                    socket_path,
                    data_dir: tenant_data_dir,
                    vm_pid: Some(vm_pid),
                    channels: req.channels,
                    created_at: chrono::Utc::now(),
                    skip_tool_approval: req.skip_tool_approval,
                };

                self.db.insert_tenant(&tenant)?;
                // Persist subnet allocator's next_index
                let _ = self.db.set_subnet_next_index(self.subnet_allocator.next_index());
                self.tenants.insert(req.tenant_id, tenant.clone());
                tracing::info!("Tenant '{}' created successfully", tenant.id);
                Ok(tenant)
            }
            Err(e) => {
                // Rollback: release subnet, clean up tap device, remove data dir, remove socket
                tracing::warn!(
                    "Tenant '{}' creation failed, rolling back: {}",
                    req.tenant_id,
                    e
                );
                self.subnet_allocator.release(&req.tenant_id);
                let _ = crate::network::delete_tap_device(&tap_device);
                let _ = std::fs::remove_dir_all(&tenant_data_dir);
                let _ = std::fs::remove_file(&socket_path);
                Err(e)
            }
        }
    }

    async fn provision_tenant(
        &self,
        req: &CreateTenantRequest,
        gateway_ip: &str,
        vm_ip: &str,
        tap_device: &str,
        socket_path: &str,
        tenant_data_dir: &str,
    ) -> Result<u32> {
        // 2. 创建 TAP 设备
        crate::network::create_tap_device(tap_device, gateway_ip)?;

        // 3. 创建数据卷
        std::fs::create_dir_all(tenant_data_dir)?;
        let data_vol = format!("{}/data.ext4", tenant_data_dir);
        crate::network::create_data_volume(&data_vol, req.tier.disk_mb())?;

        // 4. 写入环境变量 (inject skip_tool_approval if set)
        let mut env_vars = req.env_vars.clone();
        if req.skip_tool_approval {
            env_vars.insert(
                "MICROCLAW_SKIP_TOOL_APPROVAL".to_string(),
                "true".to_string(),
            );
        }
        write_tenant_env(tenant_data_dir, &env_vars)?;

        // 5. 创建 rootfs 副本 (CoW)
        let tenant_rootfs = format!("{}/rootfs.ext4", tenant_data_dir);
        std::fs::copy(&self.rootfs, &tenant_rootfs)?;

        // 6. 启动 Firecracker VM
        let fc = FirecrackerClient::new(&self.fc_bin, socket_path);
        let vm_pid = fc
            .start_vm(
                &self.vmlinux,
                &tenant_rootfs,
                &data_vol,
                req.tier.vcpu(),
                req.tier.memory_mb(),
                vm_ip,
                gateway_ip,
                tap_device,
                &req.tenant_id,
            )
            .await?;

        Ok(vm_pid)
    }

    /// Register a pre-existing tenant (e.g. for testing or recovery).
    pub fn register_tenant(&mut self, tenant: Tenant) -> Result<()> {
        self.db.insert_tenant(&tenant)?;
        self.tenants.insert(tenant.id.clone(), tenant);
        Ok(())
    }

    pub fn list_tenants(&self) -> Vec<Tenant> {
        self.tenants.values().cloned().collect()
    }

    pub fn get_tenant(&self, id: &str) -> Option<Tenant> {
        self.tenants.get(id).cloned()
    }

    pub async fn delete_tenant(&mut self, id: &str) -> Result<()> {
        let tenant = self.tenants.get(id).ok_or_else(|| anyhow::anyhow!("tenant not found"))?;

        // 停止 VM
        if let Some(pid) = tenant.vm_pid {
            let _ = nix_kill(pid);
        }

        // 删除 TAP 设备
        let _ = crate::network::delete_tap_device(&tenant.tap_device);

        // 删除数据目录
        let _ = std::fs::remove_dir_all(&tenant.data_dir);

        // 释放子网
        self.subnet_allocator.release(id);

        // 清理 socket
        let _ = std::fs::remove_file(&tenant.socket_path);

        self.db.delete_tenant(id)?;
        self.tenants.remove(id);
        tracing::info!("Tenant '{}' deleted", id);
        Ok(())
    }

    pub async fn start_tenant(&mut self, id: &str) -> Result<()> {
        let tenant = self.tenants.get_mut(id).ok_or_else(|| anyhow::anyhow!("tenant not found"))?;

        if tenant.status == TenantStatus::Running {
            bail!("tenant is already running");
        }

        // 尝试从黄金快照恢复 (更快)
        let vm_pid = if self.snapshot_manager.has_golden_snapshot() {
            let (snap, mem) = self.snapshot_manager.golden_snapshot_path();
            tracing::info!("Starting tenant '{}' from golden snapshot", id);
            self.snapshot_manager
                .restore_from_snapshot(&tenant.socket_path, &snap, &mem)
                .await?
        } else {
            let fc = FirecrackerClient::new(&self.fc_bin, &tenant.socket_path);
            let tenant_rootfs = format!("{}/rootfs.ext4", tenant.data_dir);
            let data_vol = format!("{}/data.ext4", tenant.data_dir);

            fc.start_vm(
                &self.vmlinux,
                &tenant_rootfs,
                &data_vol,
                tenant.tier.vcpu(),
                tenant.tier.memory_mb(),
                &tenant.vm_ip,
                &tenant.gateway_ip,
                &tenant.tap_device,
                &tenant.id,
            )
            .await?
        };

        self.db
            .update_tenant_status(id, TenantStatus::Running, Some(vm_pid))?;
        tenant.vm_pid = Some(vm_pid);
        tenant.status = TenantStatus::Running;
        Ok(())
    }

    pub async fn stop_tenant(&mut self, id: &str) -> Result<()> {
        let tenant = self.tenants.get_mut(id).ok_or_else(|| anyhow::anyhow!("tenant not found"))?;

        if let Some(pid) = tenant.vm_pid {
            nix_kill(pid)?;
        }

        self.db
            .update_tenant_status(id, TenantStatus::Stopped, None)?;
        tenant.vm_pid = None;
        tenant.status = TenantStatus::Stopped;
        let _ = std::fs::remove_file(&tenant.socket_path);
        Ok(())
    }

    pub async fn pause_tenant(&mut self, id: &str) -> Result<()> {
        let tenant = self.tenants.get_mut(id).ok_or_else(|| anyhow::anyhow!("tenant not found"))?;

        if tenant.status != TenantStatus::Running {
            bail!("tenant is not running");
        }

        let fc = FirecrackerClient::new(&self.fc_bin, &tenant.socket_path);
        fc.pause_vm().await?;

        self.db
            .update_tenant_status(id, TenantStatus::Paused, tenant.vm_pid)?;
        tenant.status = TenantStatus::Paused;
        Ok(())
    }

    pub async fn resume_tenant(&mut self, id: &str) -> Result<()> {
        let tenant = self.tenants.get_mut(id).ok_or_else(|| anyhow::anyhow!("tenant not found"))?;

        if tenant.status != TenantStatus::Paused {
            bail!("tenant is not paused");
        }

        let fc = FirecrackerClient::new(&self.fc_bin, &tenant.socket_path);
        fc.resume_vm().await?;

        self.db
            .update_tenant_status(id, TenantStatus::Running, tenant.vm_pid)?;
        tenant.status = TenantStatus::Running;
        Ok(())
    }

    pub async fn snapshot_tenant(&mut self, id: &str) -> Result<String> {
        let tenant = self.tenants.get(id).ok_or_else(|| anyhow::anyhow!("tenant not found"))?;

        if tenant.status != TenantStatus::Running && tenant.status != TenantStatus::Paused {
            bail!("tenant must be running or paused to snapshot");
        }

        let snapshot_dir = format!("{}/snapshots/{}", tenant.data_dir, chrono::Utc::now().format("%Y%m%d_%H%M%S"));
        std::fs::create_dir_all(&snapshot_dir)?;

        let fc = FirecrackerClient::new(&self.fc_bin, &tenant.socket_path);

        // 暂停 VM (如果正在运行)
        if tenant.status == TenantStatus::Running {
            fc.pause_vm().await?;
        }

        let snap_path = format!("{}/vm.snap", snapshot_dir);
        let mem_path = format!("{}/vm.mem", snapshot_dir);
        fc.create_snapshot(&snap_path, &mem_path).await?;

        // 恢复 VM
        if tenant.status == TenantStatus::Running {
            fc.resume_vm().await?;
        }

        Ok(snapshot_dir)
    }

    pub async fn update_env(&mut self, id: &str, env_vars: HashMap<String, String>) -> Result<()> {
        let tenant = self.tenants.get(id).ok_or_else(|| anyhow::anyhow!("tenant not found"))?;
        if tenant.status == TenantStatus::Running || tenant.status == TenantStatus::Paused {
            bail!("tenant must be stopped before updating env (data volume is in use by VM)");
        }
        write_tenant_env(&tenant.data_dir, &env_vars)?;
        tracing::info!("Tenant '{}' env updated", id);
        Ok(())
    }

    pub async fn check_health(&self, id: &str) -> Result<HealthStatus> {
        let tenant = self.tenants.get(id).ok_or_else(|| anyhow::anyhow!("tenant not found"))?;

        let vm_status = match tenant.status {
            TenantStatus::Running => "running",
            TenantStatus::Stopped => "stopped",
            TenantStatus::Paused => "paused",
            TenantStatus::Creating => "creating",
            TenantStatus::Failed => "failed",
        };

        // 尝试请求 VM 内的健康检查
        let microclaw_status = if tenant.status == TenantStatus::Running {
            let url = format!("http://{}:8080/health", tenant.vm_ip);
            match reqwest::Client::new()
                .get(&url)
                .timeout(std::time::Duration::from_secs(2))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => "healthy".to_string(),
                _ => "unreachable".to_string(),
            }
        } else {
            "n/a".to_string()
        };

        Ok(HealthStatus {
            vm_status: vm_status.to_string(),
            microclaw_status,
            uptime_s: None,
        })
    }
}

/// Write tenant env vars INTO the data.ext4 volume image.
/// Mounts the image, writes /config/.env inside it, then unmounts.
fn write_tenant_env(data_dir: &str, env_vars: &HashMap<String, String>) -> Result<()> {
    use std::process::Command;

    if env_vars.is_empty() {
        return Ok(());
    }

    let data_vol = format!("{}/data.ext4", data_dir);
    let mount_dir = format!("{}/mnt", data_dir);

    std::fs::create_dir_all(&mount_dir)?;

    // Mount the data volume
    let output = Command::new("mount")
        .args(["-o", "loop", &data_vol, &mount_dir])
        .output()?;
    if !output.status.success() {
        bail!(
            "mount {} failed: {}",
            data_vol,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Write .env inside the mounted volume, unmount even on error
    let result = (|| -> Result<()> {
        let config_dir = format!("{}/config", mount_dir);
        std::fs::create_dir_all(&config_dir)?;

        let env_content: String = env_vars
            .iter()
            .map(|(k, v)| format!("{}=\"{}\"\n", k, v))
            .collect();
        std::fs::write(format!("{}/.env", config_dir), &env_content)?;

        // Remove existing config.yaml so init.sh regenerates it from the new .env
        let _ = std::fs::remove_file(format!("{}/config.yaml", config_dir));

        // Ensure the microclaw user (uid 1000) can read it
        let _ = Command::new("chown")
            .args(["-R", "1000:1000", &config_dir])
            .output();

        Ok(())
    })();

    // Always unmount
    let _ = Command::new("umount").arg(&mount_dir).output();
    let _ = std::fs::remove_dir(&mount_dir);

    result
}

/// Parse the subnet index from a VM IP like "172.16.{index}.2".
fn parse_subnet_index(vm_ip: &str) -> Option<u16> {
    let parts: Vec<&str> = vm_ip.split('.').collect();
    if parts.len() == 4 {
        parts[2].parse::<u16>().ok()
    } else {
        None
    }
}

/// Check if a process with the given PID is alive.
fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{}", pid)).exists()
}

fn nix_kill(pid: u32) -> Result<()> {
    use std::process::Command;
    Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .output()?;
    // Wait briefly then force kill
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output();
    Ok(())
}
