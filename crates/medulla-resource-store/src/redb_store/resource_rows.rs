//! Durable resource row types and their snapshot projections.
use medulla_store::redb_store::store_prelude::*;
use medulla_store::redb_store::*;
use super::resource::*;

/// Authoritative reservation row.  The immutable WorkItem admission record is
/// retained after release/reclaim; held totals and fairness debt are updated only
/// by this transaction, never by telemetry or a local scheduler mirror.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DurableResourceReservation {
    pub record: ResourceReservationRecord,
    pub held_cpu_weight: u64,
    pub held_memory_mib: u64,
    pub held_disk_mib: u64,
    pub held_process_slots: u64,
    pub fairness_debt: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DurableResourceHost {
    pub tenant_ref: String,
    pub host_ref: String,
    pub revision: u64,
    pub capacity: ResourceCapacity,
    pub observed: ResourceCapacity,
    pub heartbeat_at_ms: u64,
    pub heartbeat_ttl_ms: u64,
    pub now_ms: u64,
    pub draining: bool,
    pub quarantined: bool,
    pub labels: Vec<String>,
    pub target_kind: String,
    pub target_alias: Option<String>,
    pub disk_used_mib: u64,
    pub disk_capacity_mib: u64,
    pub held_cpu_weight: u64,
    pub held_memory_mib: u64,
    pub held_disk_mib: u64,
    pub held_process_slots: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DurableResourceFairness {
    pub debt: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DurableResourceDiskPolicy {
    pub blocked: bool,
    pub low_watermark_mib: Option<u64>,
    pub high_watermark_mib: Option<u64>,
    pub revision: u64,
}

pub fn resource_reservation_host_capacity_snapshot(
    value: &ResourceCapacity,
) -> ResourceReservationHostCapacitySnapshot {
    ResourceReservationHostCapacitySnapshot {
        cpu_weight: value.cpu_weight,
        memory_mib: value.memory_mib,
        disk_mib: value.disk_mib,
        process_slots: value.process_slots,
    }
}

pub fn resource_host_update_capacity_snapshot(
    value: &ResourceCapacity,
) -> ResourceHostUpdateCapacitySnapshot {
    ResourceHostUpdateCapacitySnapshot {
        cpu_weight: value.cpu_weight,
        memory_mib: value.memory_mib,
        disk_mib: value.disk_mib,
        process_slots: value.process_slots,
    }
}

pub fn resource_reservation_disk_policy_snapshot(
    policy_key: String,
    policy: &DurableResourceDiskPolicy,
) -> ResourceReservationDiskPolicySnapshot {
    ResourceReservationDiskPolicySnapshot {
        policy_key,
        blocked: policy.blocked,
        low_watermark_mib: policy.low_watermark_mib,
        high_watermark_mib: policy.high_watermark_mib,
        revision: policy.revision,
    }
}

pub fn resource_host_update_disk_policy_snapshot(
    policy_key: String,
    policy: &DurableResourceDiskPolicy,
) -> ResourceHostUpdateDiskPolicySnapshot {
    ResourceHostUpdateDiskPolicySnapshot {
        policy_key,
        blocked: policy.blocked,
        low_watermark_mib: policy.low_watermark_mib,
        high_watermark_mib: policy.high_watermark_mib,
        revision: policy.revision,
    }
}

pub fn resource_reservation_host_snapshot(
    host: &DurableResourceHost,
    policies: &[(String, DurableResourceDiskPolicy)],
) -> Result<ResourceReservationHostSnapshot, String> {
    let mut labels = host.labels.clone();
    labels.sort();
    let mut policies = policies.to_vec();
    policies.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(ResourceReservationHostSnapshot {
        host_ref: host.host_ref.clone(),
        revision: host.revision,
        capacity: resource_reservation_host_capacity_snapshot(&host.capacity),
        observed: resource_reservation_host_capacity_snapshot(&host.observed),
        heartbeat_at_ms: host.heartbeat_at_ms,
        heartbeat_ttl_ms: host.heartbeat_ttl_ms,
        draining: host.draining,
        quarantined: host.quarantined,
        labels,
        target_kind: resource_reservation_snapshot_kind(&host.target_kind)?,
        target_alias: host.target_alias.clone(),
        disk_used_mib: host.disk_used_mib,
        disk_capacity_mib: host.disk_capacity_mib,
        held_cpu_weight: host.held_cpu_weight,
        held_memory_mib: host.held_memory_mib,
        held_disk_mib: host.held_disk_mib,
        held_process_slots: host.held_process_slots,
        disk_policies: policies
            .into_iter()
            .map(|(key, policy)| resource_reservation_disk_policy_snapshot(key, &policy))
            .collect(),
    })
}

pub fn resource_host_update_snapshot(
    host: &DurableResourceHost,
    policies: &[(String, DurableResourceDiskPolicy)],
) -> Result<ResourceHostUpdateSnapshot, String> {
    let mut labels = host.labels.clone();
    labels.sort();
    let mut policies = policies.to_vec();
    policies.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(ResourceHostUpdateSnapshot {
        host_ref: host.host_ref.clone(),
        revision: host.revision,
        capacity: resource_host_update_capacity_snapshot(&host.capacity),
        observed: resource_host_update_capacity_snapshot(&host.observed),
        heartbeat_at_ms: host.heartbeat_at_ms,
        heartbeat_ttl_ms: host.heartbeat_ttl_ms,
        draining: host.draining,
        quarantined: host.quarantined,
        labels,
        target_kind: resource_host_update_snapshot_kind(&host.target_kind)?,
        target_alias: host.target_alias.clone(),
        disk_used_mib: host.disk_used_mib,
        disk_capacity_mib: host.disk_capacity_mib,
        held_cpu_weight: host.held_cpu_weight,
        held_memory_mib: host.held_memory_mib,
        held_disk_mib: host.held_disk_mib,
        held_process_slots: host.held_process_slots,
        disk_policies: policies
            .into_iter()
            .map(|(key, policy)| resource_host_update_disk_policy_snapshot(key, &policy))
            .collect(),
    })
}

pub fn resource_collect_disk_policy_rows(
    policies: &mut ScopedOwnerTableMut<'_, (&str, &str), &[u8]>,
    graph: &str,
    host_ref: &str,
    crypto: DurableCrypto<'_>,
) -> Result<Vec<(String, DurableResourceDiskPolicy)>, String> {
    let prefix = format!("{host_ref}\0");
    let mut rows = Vec::new();
    for row in policies.scope_rows().map_err(|error| error.to_string())? {
        if rows.len() >= MAX_RESOURCE_HOST_DISK_POLICIES {
            return Err("resource disk-policy scan exceeds native bound".to_string());
        }
        let (key, value) = row.map_err(|error| error.to_string())?;
        let (row_graph, policy_key) = key.value();
        if row_graph != graph {
            return Err("resource disk-policy row escaped its scope".to_string());
        }
        if !policy_key.starts_with(&prefix) {
            if policy_key < prefix.as_str() {
                continue;
            }
            break;
        }
        let policy_key = policy_key
            .strip_prefix(&prefix)
            .ok_or_else(|| "resource disk-policy key escaped host scope".to_string())?;
        resource_text(policy_key, "resource disk_policy_key")?;
        let policy = resource_decode::<DurableResourceDiskPolicy>(value.value(), crypto)?;
        rows.push((policy_key.to_string(), policy));
    }
    Ok(rows)
}
