use std::collections::HashMap;
use std::process::Command;

use anyhow::{bail, Result};

/// 子网分配器: 为每个租户分配独立的 /30 子网
pub struct SubnetAllocator {
    base_network: String, // e.g. "172.16"
    next_index: u16,
    allocated: HashMap<String, u16>, // tenant_id -> subnet index
}

impl SubnetAllocator {
    pub fn new(cidr: &str) -> Self {
        // 从 CIDR 提取基础网络 (简化: 只支持 172.16.0.0/16)
        let base = cidr.split('.').take(2).collect::<Vec<_>>().join(".");

        Self {
            base_network: base,
            next_index: 1,
            allocated: HashMap::new(),
        }
    }

    /// 分配一个 /30 子网，返回 (gateway_ip, vm_ip)
    pub fn allocate(&mut self, tenant_id: &str) -> Result<(String, String)> {
        if self.allocated.contains_key(tenant_id) {
            bail!("subnet already allocated for tenant '{}'", tenant_id);
        }

        if self.next_index > 65000 {
            bail!("subnet pool exhausted");
        }

        let index = self.next_index;
        self.next_index += 1;
        self.allocated.insert(tenant_id.to_string(), index);

        // 每个租户用一个 /30:
        // 172.16.{index}.1 = gateway (host TAP)
        // 172.16.{index}.2 = VM
        let gateway_ip = format!("{}.{}.1", self.base_network, index);
        let vm_ip = format!("{}.{}.2", self.base_network, index);

        tracing::info!(
            "Allocated subnet for '{}': gateway={}, vm={}",
            tenant_id,
            gateway_ip,
            vm_ip
        );

        Ok((gateway_ip, vm_ip))
    }

    pub fn release(&mut self, tenant_id: &str) {
        self.allocated.remove(tenant_id);
    }

    /// Set the next subnet index (used during recovery from DB).
    pub fn set_next_index(&mut self, index: u16) {
        self.next_index = index;
    }

    /// Restore a tenant→subnet allocation without bumping next_index (used during recovery).
    pub fn restore_allocation(&mut self, tenant_id: &str, index: u16) {
        self.allocated.insert(tenant_id.to_string(), index);
    }

    /// Return the current next_index value (for persisting to DB).
    pub fn next_index(&self) -> u16 {
        self.next_index
    }
}

/// 创建 TAP 网络设备
pub fn create_tap_device(tap_name: &str, gateway_ip: &str) -> Result<()> {
    tracing::info!("Creating TAP device: {} (gateway={})", tap_name, gateway_ip);

    // 删除已存在的同名 TAP 设备 (忽略错误，可能不存在)
    let _ = run_cmd("ip", &["link", "del", tap_name]);

    // 创建 TAP 设备
    run_cmd("ip", &["tuntap", "add", "dev", tap_name, "mode", "tap"])?;
    run_cmd("ip", &["addr", "add", &format!("{}/30", gateway_ip), "dev", tap_name])?;
    run_cmd("ip", &["link", "set", tap_name, "up"])?;

    // 启用 IP 转发
    run_cmd("sysctl", &["-w", "net.ipv4.ip_forward=1"])?;

    // 检测主机出口网卡
    let host_iface = detect_host_interface()?;

    // 子网 (从 gateway_ip 推导 /30 网络)
    let parts: Vec<&str> = gateway_ip.rsplitn(2, '.').collect();
    let subnet = format!("{}.0/30", parts[1]);

    // NAT 规则
    run_cmd(
        "iptables",
        &["-t", "nat", "-A", "POSTROUTING", "-s", &subnet, "-o", &host_iface, "-j", "MASQUERADE"],
    )?;
    run_cmd(
        "iptables",
        &["-A", "FORWARD", "-i", tap_name, "-o", &host_iface, "-j", "ACCEPT"],
    )?;
    run_cmd(
        "iptables",
        &[
            "-A", "FORWARD", "-i", &host_iface, "-o", tap_name,
            "-m", "state", "--state", "RELATED,ESTABLISHED", "-j", "ACCEPT",
        ],
    )?;

    Ok(())
}

/// 删除 TAP 网络设备及其关联的 iptables 规则
pub fn delete_tap_device(tap_name: &str) -> Result<()> {
    tracing::info!("Deleting TAP device: {}", tap_name);

    // 读取 TAP 设备的 gateway IP（用于推导子网，清理 NAT 规则）
    let gateway_ip = get_tap_gateway_ip(tap_name);

    // 清理 iptables FORWARD 规则（与 TAP 名称关联）
    let _ = delete_iptables_rules_by_interface("FORWARD", tap_name);

    // 清理 iptables NAT POSTROUTING 规则（与子网关联）
    if let Some(gw) = &gateway_ip {
        let parts: Vec<&str> = gw.rsplitn(2, '.').collect();
        let subnet = format!("{}.0/30", parts[1]);
        let _ = delete_nat_rules_by_subnet(&subnet);
    }

    run_cmd("ip", &["link", "del", tap_name])?;
    Ok(())
}

/// 从 TAP 设备读取 gateway IP 地址
fn get_tap_gateway_ip(tap_name: &str) -> Option<String> {
    let output = Command::new("ip")
        .args(["-4", "addr", "show", tap_name])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // 解析 "inet 172.16.1.1/30" 格式
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("inet ") {
            if let Some(addr) = trimmed.split_whitespace().nth(1) {
                return addr.split('/').next().map(|s| s.to_string());
            }
        }
    }
    None
}

/// 删除 FORWARD 链中所有关联指定网卡的规则
fn delete_iptables_rules_by_interface(chain: &str, iface: &str) -> Result<()> {
    let output = Command::new("iptables")
        .args(["-S", chain])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains(iface) {
            // 将 "-A FORWARD ..." 转为 "-D FORWARD ..." 来删除
            let delete_rule = line.replacen("-A ", "-D ", 1);
            let args: Vec<&str> = delete_rule.split_whitespace().collect();
            let _ = Command::new("iptables").args(&args).output();
        }
    }
    Ok(())
}

/// 删除 NAT POSTROUTING 链中所有关联指定子网的规则
fn delete_nat_rules_by_subnet(subnet: &str) -> Result<()> {
    let output = Command::new("iptables")
        .args(["-t", "nat", "-S", "POSTROUTING"])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains(subnet) {
            let delete_rule = line.replacen("-A ", "-D ", 1);
            let args: Vec<&str> = delete_rule.split_whitespace().collect();
            let _ = Command::new("iptables").args(["-t", "nat"]).args(&args).output();
        }
    }
    Ok(())
}

/// 创建数据卷 (ext4 磁盘镜像)
pub fn create_data_volume(path: &str, size_mb: u32) -> Result<()> {
    tracing::info!("Creating data volume: {} ({}MB)", path, size_mb);

    // 创建稀疏文件
    run_cmd(
        "dd",
        &[
            "if=/dev/zero",
            &format!("of={}", path),
            "bs=1M",
            &format!("count={}", size_mb),
        ],
    )?;

    // 格式化为 ext4
    run_cmd("mkfs.ext4", &["-F", "-L", "tenant-data", path])?;

    Ok(())
}

fn detect_host_interface() -> Result<String> {
    let output = Command::new("ip")
        .args(["route", "get", "8.8.8.8"])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for part in stdout.split_whitespace() {
        // 网卡名通常在 "dev" 关键字之后
        if part.starts_with("eth") || part.starts_with("ens") || part.starts_with("enp") {
            return Ok(part.to_string());
        }
    }

    // 回退: 解析 "dev XXX" 模式
    if let Some(idx) = stdout.find("dev ") {
        let rest = &stdout[idx + 4..];
        if let Some(iface) = rest.split_whitespace().next() {
            return Ok(iface.to_string());
        }
    }

    bail!("could not detect host network interface")
}

fn run_cmd(cmd: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(cmd).args(args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} {} failed: {}", cmd, args.join(" "), stderr.trim());
    }
    Ok(())
}
