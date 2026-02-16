use anyhow::{bail, Result};
use std::process::Command;

/// Firecracker API 客户端 (通过 Unix socket 通信)
pub struct FirecrackerClient {
    fc_bin: String,
    socket_path: String,
}

impl FirecrackerClient {
    pub fn new(fc_bin: &str, socket_path: &str) -> Self {
        Self {
            fc_bin: fc_bin.to_string(),
            socket_path: socket_path.to_string(),
        }
    }

    /// 启动一个新的 Firecracker microVM，返回进程 PID
    pub async fn start_vm(
        &self,
        vmlinux: &str,
        rootfs: &str,
        data_vol: &str,
        vcpu: u32,
        memory_mb: u32,
        vm_ip: &str,
        gateway_ip: &str,
        tap_device: &str,
        tenant_id: &str,
    ) -> Result<u32> {
        // 清理旧 socket
        let _ = std::fs::remove_file(&self.socket_path);

        // 启动 Firecracker 进程
        let child = Command::new(&self.fc_bin)
            .arg("--api-sock")
            .arg(&self.socket_path)
            .spawn()?;

        let pid = child.id();
        tracing::info!("Firecracker started (pid={}, socket={})", pid, self.socket_path);

        // 等待 socket 就绪
        for _ in 0..20 {
            if std::path::Path::new(&self.socket_path).exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        if !std::path::Path::new(&self.socket_path).exists() {
            bail!("Firecracker socket did not appear");
        }

        // 配置 boot source
        let boot_args = format!(
            "init=/init console=ttyS0 reboot=k panic=1 pci=off \
             FC_VM_IP={vm_ip} FC_VM_GATEWAY={gateway_ip} FC_VM_NETMASK=30 \
             FC_TENANT_ID={tenant_id} FC_DNS=8.8.8.8 FC_PORT=8080"
        );

        self.api_put(
            "/boot-source",
            &serde_json::json!({
                "kernel_image_path": vmlinux,
                "boot_args": boot_args
            }),
        )
        .await?;

        // 配置 rootfs
        self.api_put(
            "/drives/rootfs",
            &serde_json::json!({
                "drive_id": "rootfs",
                "path_on_host": rootfs,
                "is_root_device": true,
                "is_read_only": false
            }),
        )
        .await?;

        // 配置数据卷
        self.api_put(
            "/drives/data",
            &serde_json::json!({
                "drive_id": "data",
                "path_on_host": data_vol,
                "is_root_device": false,
                "is_read_only": false
            }),
        )
        .await?;

        // 配置机器资源
        self.api_put(
            "/machine-config",
            &serde_json::json!({
                "vcpu_count": vcpu,
                "mem_size_mib": memory_mb
            }),
        )
        .await?;

        // 配置网络
        let mac = generate_mac(vm_ip);
        self.api_put(
            "/network-interfaces/eth0",
            &serde_json::json!({
                "iface_id": "eth0",
                "guest_mac": mac,
                "host_dev_name": tap_device
            }),
        )
        .await?;

        // 启动实例
        self.api_put(
            "/actions",
            &serde_json::json!({
                "action_type": "InstanceStart"
            }),
        )
        .await?;

        tracing::info!("VM started for tenant '{}' (ip={}, pid={})", tenant_id, vm_ip, pid);
        Ok(pid)
    }

    /// 暂停 VM
    pub async fn pause_vm(&self) -> Result<()> {
        self.api_patch("/vm", &serde_json::json!({"state": "Paused"}))
            .await
    }

    /// 恢复 VM
    pub async fn resume_vm(&self) -> Result<()> {
        self.api_patch("/vm", &serde_json::json!({"state": "Resumed"}))
            .await
    }

    /// 创建快照
    pub async fn create_snapshot(&self, snapshot_path: &str, mem_path: &str) -> Result<()> {
        self.api_put(
            "/snapshot/create",
            &serde_json::json!({
                "snapshot_type": "Full",
                "snapshot_path": snapshot_path,
                "mem_file_path": mem_path
            }),
        )
        .await
    }

    async fn api_put(&self, path: &str, body: &serde_json::Value) -> Result<()> {
        let output = Command::new("curl")
            .args([
                "--unix-socket",
                &self.socket_path,
                "-s",
                "-w",
                "%{http_code}",
                "-X",
                "PUT",
                "-H",
                "Content-Type: application/json",
                "-d",
                &body.to_string(),
                &format!("http://localhost{}", path),
            ])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let status_code = stdout.chars().rev().take(3).collect::<String>().chars().rev().collect::<String>();

        if !status_code.starts_with('2') {
            let response_body = &stdout[..stdout.len().saturating_sub(3)];
            bail!(
                "Firecracker API PUT {} failed ({}): {}",
                path,
                status_code,
                response_body
            );
        }

        Ok(())
    }

    async fn api_patch(&self, path: &str, body: &serde_json::Value) -> Result<()> {
        let output = Command::new("curl")
            .args([
                "--unix-socket",
                &self.socket_path,
                "-s",
                "-w",
                "%{http_code}",
                "-X",
                "PATCH",
                "-H",
                "Content-Type: application/json",
                "-d",
                &body.to_string(),
                &format!("http://localhost{}", path),
            ])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let status_code = stdout.chars().rev().take(3).collect::<String>().chars().rev().collect::<String>();

        if !status_code.starts_with('2') {
            let response_body = &stdout[..stdout.len().saturating_sub(3)];
            bail!(
                "Firecracker API PATCH {} failed ({}): {}",
                path,
                status_code,
                response_body
            );
        }

        Ok(())
    }
}

/// 根据 VM IP 生成 MAC 地址
fn generate_mac(vm_ip: &str) -> String {
    let parts: Vec<u8> = vm_ip
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();

    if parts.len() == 4 {
        format!(
            "06:00:{:02X}:{:02X}:{:02X}:{:02X}",
            parts[0], parts[1], parts[2], parts[3]
        )
    } else {
        "06:00:AC:10:00:02".to_string()
    }
}
