use anyhow::Result;

/// 快照管理: 创建和恢复黄金快照
pub struct SnapshotManager {
    fc_bin: String,
    snapshot_dir: String,
}

impl SnapshotManager {
    pub fn new(fc_bin: String, snapshot_dir: String) -> Self {
        Self {
            fc_bin,
            snapshot_dir,
        }
    }

    /// 从快照恢复 VM (用于快速启动)
    pub async fn restore_from_snapshot(
        &self,
        socket_path: &str,
        snapshot_path: &str,
        mem_path: &str,
    ) -> Result<u32> {
        let _ = std::fs::remove_file(socket_path);

        let child = std::process::Command::new(&self.fc_bin)
            .arg("--api-sock")
            .arg(socket_path)
            .spawn()?;

        let pid = child.id();

        // 等待 socket
        for _ in 0..20 {
            if std::path::Path::new(socket_path).exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // 加载快照
        let output = std::process::Command::new("curl")
            .args([
                "--unix-socket",
                socket_path,
                "-s",
                "-X",
                "PUT",
                "-H",
                "Content-Type: application/json",
                "-d",
                &serde_json::json!({
                    "snapshot_path": snapshot_path,
                    "mem_backend": {
                        "backend_path": mem_path,
                        "backend_type": "File"
                    },
                    "enable_diff_snapshots": false,
                    "resume_vm": true
                })
                .to_string(),
                &format!("http://localhost/snapshot/load"),
            ])
            .output()?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to restore snapshot: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        tracing::info!("VM restored from snapshot (pid={})", pid);
        Ok(pid)
    }

    /// 获取黄金快照路径
    pub fn golden_snapshot_path(&self) -> (String, String) {
        let snap = format!("{}/golden/vm.snap", self.snapshot_dir);
        let mem = format!("{}/golden/vm.mem", self.snapshot_dir);
        (snap, mem)
    }

    /// 检查黄金快照是否存在
    pub fn has_golden_snapshot(&self) -> bool {
        let (snap, mem) = self.golden_snapshot_path();
        std::path::Path::new(&snap).exists() && std::path::Path::new(&mem).exists()
    }
}
