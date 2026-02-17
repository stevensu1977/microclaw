use std::sync::{Mutex, MutexGuard};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::tenant::{Tenant, TenantStatus, Tier};

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn new(path: &str) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        apply_schema_migrations(&conn)?;

        Ok(Database {
            conn: Mutex::new(conn),
        })
    }

    fn lock_conn(&self) -> MutexGuard<'_, Connection> {
        match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn insert_tenant(&self, tenant: &Tenant) -> Result<()> {
        let conn = self.lock_conn();
        let channels_json = serde_json::to_string(&tenant.channels)?;
        conn.execute(
            "INSERT INTO tenants (id, tier, status, vm_ip, gateway_ip, tap_device, socket_path, data_dir, vm_pid, channels, skip_tool_approval, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                tenant.id,
                tier_to_str(&tenant.tier),
                status_to_str(&tenant.status),
                tenant.vm_ip,
                tenant.gateway_ip,
                tenant.tap_device,
                tenant.socket_path,
                tenant.data_dir,
                tenant.vm_pid,
                channels_json,
                tenant.skip_tool_approval as i32,
                tenant.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn update_tenant_status(
        &self,
        id: &str,
        status: TenantStatus,
        vm_pid: Option<u32>,
    ) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE tenants SET status = ?1, vm_pid = ?2 WHERE id = ?3",
            params![status_to_str(&status), vm_pid, id],
        )?;
        Ok(())
    }

    pub fn delete_tenant(&self, id: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute("DELETE FROM tenants WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn load_all_tenants(&self) -> Result<Vec<Tenant>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, tier, status, vm_ip, gateway_ip, tap_device, socket_path, data_dir, vm_pid, channels, skip_tool_approval, created_at
             FROM tenants",
        )?;

        let tenants = stmt
            .query_map([], |row| {
                let tier_str: String = row.get(1)?;
                let status_str: String = row.get(2)?;
                let channels_json: String = row.get(9)?;
                let skip_tool: i32 = row.get(10)?;
                let created_str: String = row.get(11)?;

                Ok(TenantRow {
                    id: row.get(0)?,
                    tier_str,
                    status_str,
                    vm_ip: row.get(3)?,
                    gateway_ip: row.get(4)?,
                    tap_device: row.get(5)?,
                    socket_path: row.get(6)?,
                    data_dir: row.get(7)?,
                    vm_pid: row.get(8)?,
                    channels_json,
                    skip_tool_approval: skip_tool != 0,
                    created_at_str: created_str,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(tenants.len());
        for row in tenants {
            let tenant = Tenant {
                id: row.id,
                tier: str_to_tier(&row.tier_str),
                status: str_to_status(&row.status_str),
                vm_ip: row.vm_ip,
                gateway_ip: row.gateway_ip,
                tap_device: row.tap_device,
                socket_path: row.socket_path,
                data_dir: row.data_dir,
                vm_pid: row.vm_pid,
                channels: serde_json::from_str(&row.channels_json).unwrap_or_default(),
                created_at: chrono::DateTime::parse_from_rfc3339(&row.created_at_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                skip_tool_approval: row.skip_tool_approval,
            };
            result.push(tenant);
        }

        Ok(result)
    }

    pub fn get_subnet_next_index(&self) -> Result<u16> {
        let conn = self.lock_conn();
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM db_meta WHERE key = 'subnet_next_index'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        Ok(raw.and_then(|s| s.parse::<u16>().ok()).unwrap_or(1))
    }

    pub fn set_subnet_next_index(&self, index: u16) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO db_meta(key, value) VALUES('subnet_next_index', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![index.to_string()],
        )?;
        Ok(())
    }
}

/// Intermediate struct for reading rows before converting to Tenant.
struct TenantRow {
    id: String,
    tier_str: String,
    status_str: String,
    vm_ip: String,
    gateway_ip: String,
    tap_device: String,
    socket_path: String,
    data_dir: String,
    vm_pid: Option<u32>,
    channels_json: String,
    skip_tool_approval: bool,
    created_at_str: String,
}

fn tier_to_str(tier: &Tier) -> &'static str {
    match tier {
        Tier::Free => "Free",
        Tier::Pro => "Pro",
        Tier::Team => "Team",
        Tier::Enterprise => "Enterprise",
    }
}

fn str_to_tier(s: &str) -> Tier {
    match s {
        "Free" => Tier::Free,
        "Pro" => Tier::Pro,
        "Team" => Tier::Team,
        "Enterprise" => Tier::Enterprise,
        _ => Tier::Free,
    }
}

fn status_to_str(status: &TenantStatus) -> &'static str {
    match status {
        TenantStatus::Creating => "Creating",
        TenantStatus::Running => "Running",
        TenantStatus::Stopped => "Stopped",
        TenantStatus::Paused => "Paused",
        TenantStatus::Failed => "Failed",
    }
}

fn str_to_status(s: &str) -> TenantStatus {
    match s {
        "Creating" => TenantStatus::Creating,
        "Running" => TenantStatus::Running,
        "Stopped" => TenantStatus::Stopped,
        "Paused" => TenantStatus::Paused,
        "Failed" => TenantStatus::Failed,
        _ => TenantStatus::Failed,
    }
}

fn apply_schema_migrations(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS db_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )?;

    let version = get_schema_version(conn)?;

    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tenants (
                id TEXT PRIMARY KEY,
                tier TEXT NOT NULL,
                status TEXT NOT NULL,
                vm_ip TEXT NOT NULL,
                gateway_ip TEXT NOT NULL,
                tap_device TEXT NOT NULL,
                socket_path TEXT NOT NULL,
                data_dir TEXT NOT NULL,
                vm_pid INTEGER,
                channels TEXT NOT NULL,
                skip_tool_approval INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );",
        )?;
        set_schema_version(conn, 1)?;
    }

    Ok(())
}

fn get_schema_version(conn: &Connection) -> Result<i64> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM db_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    Ok(raw.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0))
}

fn set_schema_version(conn: &Connection, version: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO db_meta(key, value) VALUES('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![version.to_string()],
    )?;
    Ok(())
}
