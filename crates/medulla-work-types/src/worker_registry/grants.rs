//! Pure capacity sizing. Served admission, lane selection and reservations remain R075.

use super::WorkerRecord;
use medulla_types::{scheduler_config::InvalidSchedulerConfig, scheduler_config::SchedulerConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::num::NonZeroU32;

/// Per-request facts, not another scheduler policy. The engine supplies current
/// available memory and classifies the selected lane; opaque lane names alone
/// cannot identify a writer. The caller's optional ceiling can only lower a grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerSizingInput {
    pub request_ceiling: Option<u32>,
    pub available_memory_bytes: u64,
    pub is_write_lane: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerCapacityRefusal {
    InvalidConfig(InvalidSchedulerConfig),
    NoCores,
    CpuShareDisabled,
    PoolDisabled,
    QueueDisabled,
    InsufficientMemory,
    RequestCeilingZero,
    WriterWidthZero,
}

impl WorkerCapacityRefusal {
    /// The closed code of the binding limit, carried on the wire refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidConfig(_) => "INVALID_CONFIG",
            Self::NoCores => "NO_CORES",
            Self::CpuShareDisabled => "CPU_SHARE_DISABLED",
            Self::PoolDisabled => "POOL_DISABLED",
            Self::QueueDisabled => "QUEUE_DISABLED",
            Self::InsufficientMemory => "INSUFFICIENT_MEMORY",
            Self::RequestCeilingZero => "REQUEST_CEILING_ZERO",
            Self::WriterWidthZero => "WRITER_WIDTH_ZERO",
        }
    }
}

/// A refusal always exposes zero slots; a grant is necessarily nonzero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerSlotGrant {
    Granted(NonZeroU32),
    Refused(WorkerCapacityRefusal),
}

impl WorkerSlotGrant {
    pub fn slots(&self) -> u32 {
        match self {
            Self::Granted(slots) => slots.get(),
            Self::Refused(_) => 0,
        }
    }
}

/// Size without allocating, reserving lanes, or admitting a task. The CPU share
/// rounds down, with a floor of one only for a positive share and host core count.
/// Every other hard bound is applied *after* that floor, including live available
/// memory, registered memory, the caller ceiling, pool size and write-lane width.
pub fn granted_slots(
    record: &WorkerRecord,
    config: &SchedulerConfig,
    input: WorkerSizingInput,
    shard_writer_width: u32,
) -> WorkerSlotGrant {
    use WorkerCapacityRefusal::*;
    if let Err(error) = config.validate() {
        return WorkerSlotGrant::Refused(InvalidConfig(error));
    }
    let memory_slots =
        record.memory_bytes.min(input.available_memory_bytes) / config.memory_per_worker_bytes;
    let limits = [
        (u64::from(record.cores), NoCores),
        (u64::from(config.cores_per_mille), CpuShareDisabled),
        (u64::from(config.total_workers), PoolDisabled),
        (config.max_queue_depth, QueueDisabled),
        (memory_slots, InsufficientMemory),
        (
            u64::from(input.request_ceiling.unwrap_or(u32::MAX)),
            RequestCeilingZero,
        ),
        (
            u64::from(if input.is_write_lane {
                shard_writer_width
            } else {
                u32::MAX
            }),
            WriterWidthZero,
        ),
    ];
    if let Some((_, reason)) = limits.iter().find(|(limit, _)| *limit == 0) {
        return WorkerSlotGrant::Refused(reason.clone());
    }
    let core_slots = (u64::from(record.cores) * u64::from(config.cores_per_mille) / 1000).max(1);
    let slots = core_slots
        .min(u64::from(config.total_workers))
        .min(memory_slots)
        .min(limits[5].0)
        .min(limits[6].0) as u32;
    // All bounds above are positive and at most u32::MAX.
    WorkerSlotGrant::Granted(NonZeroU32::new(slots).expect("positive checked capacity bounds"))
}

/// Per-lane claim load: running workers and queued depth, as observed by the
/// caller (the durable queue and the registry lease rows own the state,
/// MEDULLA-DURABLE-KERNEL-R072/R074).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LaneLoad {
    pub running: u32,
    pub queued: u32,
}

/// The served admission reply (MEDULLA-DURABLE-KERNEL-R075,
/// MEDULLA-PLACEMENT-R002.4): a nonzero slot grant bound to the lane the
/// engine picked from the lane floors, with the config-derived lease and
/// back-off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkerGrant {
    pub slots: u32,
    pub reserved_lane: String,
    /// `SchedulerConfig.task_lease_ms` over 1 000.
    pub lease_seconds: u64,
    /// `SchedulerConfig.retry_after_ms` over 1 000.
    pub retry_after_seconds: u64,
    /// Identity of the registry lease row the daemon leadership fences on
    /// (the lease row is owned by MEDULLA-DURABLE-KERNEL-R074).
    pub grant_id: String,
}

/// Typed admission refusal; each variant carries its closed wire code
/// (MEDULLA-DURABLE-KERNEL-R075, MEDULLA-INGEST-WORKER-R604.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkerAdmissionRefusal {
    /// The grant would drop the pool below the reserved lane floor.
    AdmitHeldForReserve { lane: String },
    /// No live worker with this id; expired entries are unknown too.
    WorkerUnknown,
    /// `granted_slots` refused; the binding limit's closed code is carried.
    WorkerCapacityZero { limit: String },
}

impl WorkerAdmissionRefusal {
    /// The closed code the served reply and the refusal counters carry.
    pub fn code(&self) -> &'static str {
        match self {
            Self::AdmitHeldForReserve { .. } => "ADMIT_HELD_FOR_RESERVE",
            Self::WorkerUnknown => "WORKER_UNKNOWN",
            Self::WorkerCapacityZero { .. } => "WORKER_CAPACITY_ZERO",
        }
    }
}

/// One admission decision: a grant or a typed refusal. A refusal is never a
/// zero-slot grant, and a grant is never a refusal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WorkerAdmission {
    Granted(WorkerGrant),
    Refused(WorkerAdmissionRefusal),
}

/// The lane the engine picks from the lane floors: the supported lane with the
/// highest floor, ties to the lexicographic first. A worker never admits to a
/// lane it cannot run.
fn select_reserved_lane(record: &WorkerRecord, config: &SchedulerConfig) -> String {
    let mut best: Option<(u32, &str)> = None;
    for lane in &record.lanes_supported {
        let floor = config.lane_floors.get(lane).copied().unwrap_or(0);
        if best.map_or(true, |(top, _)| floor > top) {
            best = Some((floor, lane));
        }
    }
    best.map(|(_, lane)| lane.to_string())
        .unwrap_or_else(|| "general".to_string())
}

/// Engine admission for one served `AdmitWorkerTask`
/// (MEDULLA-DURABLE-KERNEL-R075). Sizes through [`granted_slots`], picks the
/// reserved lane from the lane floors, and refuses a grant that starves the
/// reserve (MEDULLA-INGEST-WORKER-R604.4) — the exception is a lane with
/// queued work and no running worker. The caller owns the registry lookup
/// (a missing live worker is `WORKER_UNKNOWN`) and supplies the per-lane
/// load, the R077 shard-writer width and the grant lease-row identity.
pub fn admit_reserved_worker_task(
    record: &WorkerRecord,
    config: &SchedulerConfig,
    lane_load: &BTreeMap<String, LaneLoad>,
    input: WorkerSizingInput,
    shard_writer_width: u32,
    grant_id: String,
) -> WorkerAdmission {
    let lane = select_reserved_lane(record, config);
    // The engine classifies the selected lane: a lane that holds a floor is a
    // reserved write lane; an opaque lane name alone cannot identify a writer.
    let input = WorkerSizingInput {
        is_write_lane: input.is_write_lane || config.lane_floors.contains_key(&lane),
        ..input
    };
    match granted_slots(record, config, input, shard_writer_width) {
        WorkerSlotGrant::Refused(reason) => {
            return WorkerAdmission::Refused(WorkerAdmissionRefusal::WorkerCapacityZero {
                limit: reason.code().to_string(),
            });
        }
        WorkerSlotGrant::Granted(slots) => {
            let slots = slots.get();
            let load = lane_load.get(&lane).copied().unwrap_or_default();
            let running_total: u32 = lane_load.values().map(|load| load.running).sum();
            let floor = config.lane_floors.get(&lane).copied().unwrap_or(0);
            let free = u64::from(config.total_workers).saturating_sub(u64::from(running_total));
            let starves = free < u64::from(slots) + u64::from(floor);
            let lane_relieves_queue = load.running == 0 && load.queued > 0;
            if starves && !lane_relieves_queue {
                return WorkerAdmission::Refused(WorkerAdmissionRefusal::AdmitHeldForReserve {
                    lane,
                });
            }
            WorkerAdmission::Granted(WorkerGrant {
                slots,
                reserved_lane: lane,
                lease_seconds: config.task_lease_ms / 1000,
                retry_after_seconds: config.retry_after_ms / 1000,
                grant_id,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // spec: MEDULLA-DURABLE-KERNEL-R075
    #[test]
    fn one_worker_pool_is_granted_the_general_lane_and_a_non_reserved_task_is_held() {
        let record = WorkerRecord {
            worker_id: "worker".into(),
            cores: 16,
            memory_bytes: 8 * 1024 * 1024 * 1024,
            lanes_supported: ["general".into()].into(),
            last_heartbeat_ms: 0,
            expires_at_ms: u64::MAX,
        };
        // A one-worker pool: two slots, one reserved for the general lane.
        let config = SchedulerConfig {
            total_workers: 2,
            lane_floors: BTreeMap::from([("general".to_string(), 1)]),
            ..serde_json::from_str(r#"{"total_workers":2,"max_queue_depth":64}"#).unwrap()
        };
        let input = WorkerSizingInput {
            request_ceiling: Some(1),
            available_memory_bytes: record.memory_bytes,
            is_write_lane: false,
        };

        // The pool's single worker is granted the general lane.
        let grant = admit_reserved_worker_task(
            &record,
            &config,
            &BTreeMap::new(),
            input,
            9,
            "grant-1".into(),
        );
        let WorkerAdmission::Granted(grant) = grant else {
            panic!("one-worker pool was refused: {grant:?}")
        };
        assert_eq!(grant.slots, 1);
        assert_eq!(grant.reserved_lane, "general");
        assert_eq!(grant.lease_seconds, 60);
        assert_eq!(grant.retry_after_seconds, 0);
        assert_eq!(grant.grant_id, "grant-1");

        // With the one non-reserved slot taken, only reserved capacity remains:
        // the non-reserved task is held for the reserve.
        let mut load = BTreeMap::new();
        load.insert(
            "general".to_string(),
            LaneLoad {
                running: 1,
                queued: 0,
            },
        );
        let held = admit_reserved_worker_task(&record, &config, &load, input, 9, "grant-2".into());
        let WorkerAdmission::Refused(WorkerAdmissionRefusal::AdmitHeldForReserve { lane }) = held
        else {
            panic!("expected the reserve refusal: {held:?}")
        };
        assert_eq!(lane, "general");
        assert_eq!(
            WorkerAdmissionRefusal::AdmitHeldForReserve { lane: lane.clone() }.code(),
            "ADMIT_HELD_FOR_RESERVE"
        );

        // The exception: a lane with queued work and no running worker is
        // relieved even when the grant starves the reserve.
        let idle_config = SchedulerConfig {
            total_workers: 1,
            lane_floors: config.lane_floors.clone(),
            ..config
        };
        let mut idle = BTreeMap::new();
        idle.insert(
            "general".to_string(),
            LaneLoad {
                running: 0,
                queued: 2,
            },
        );
        let relieved =
            admit_reserved_worker_task(&record, &idle_config, &idle, input, 9, "grant-3".into());
        assert!(
            matches!(relieved, WorkerAdmission::Granted(_)),
            "queued idle lane was held: {relieved:?}"
        );

        // A hard capacity refusal keeps its code and binding limit.
        let starved = WorkerSizingInput {
            available_memory_bytes: 0,
            ..input
        };
        let refused = admit_reserved_worker_task(
            &record,
            &config,
            &BTreeMap::new(),
            starved,
            9,
            "grant-4".into(),
        );
        let WorkerAdmission::Refused(WorkerAdmissionRefusal::WorkerCapacityZero { limit }) =
            refused
        else {
            panic!("expected the capacity refusal: {refused:?}")
        };
        assert_eq!(limit, "INSUFFICIENT_MEMORY");
        assert_eq!(
            WorkerAdmissionRefusal::WorkerCapacityZero {
                limit: limit.clone()
            }
            .code(),
            "WORKER_CAPACITY_ZERO"
        );
    }

    fn fixture() -> (WorkerRecord, SchedulerConfig, WorkerSizingInput) {
        let record = WorkerRecord {
            worker_id: "worker".into(),
            cores: 16,
            memory_bytes: 64 * 1024 * 1024 * 1024,
            lanes_supported: ["general".into()].into(),
            last_heartbeat_ms: 0,
            expires_at_ms: 100,
        };
        let config =
            serde_json::from_str(r#"{"total_workers":100,"max_queue_depth":100}"#).unwrap();
        let input = WorkerSizingInput {
            request_ceiling: None,
            available_memory_bytes: record.memory_bytes,
            is_write_lane: false,
        };
        (record, config, input)
    }

    // spec: MEDULLA-PLACEMENT-R002.3
    #[test]
    fn each_hard_bound_limits_the_grant() {
        let (mut record, mut config, mut input) = fixture();
        assert_eq!(granted_slots(&record, &config, input, 0).slots(), 5); // core-bound
        record.memory_bytes = config.memory_per_worker_bytes * 2;
        assert_eq!(granted_slots(&record, &config, input, 0).slots(), 2);
        record.memory_bytes = u64::MAX;
        input.available_memory_bytes = config.memory_per_worker_bytes * 3;
        assert_eq!(granted_slots(&record, &config, input, 0).slots(), 3);
        input.request_ceiling = Some(2);
        assert_eq!(granted_slots(&record, &config, input, 9).slots(), 2);
        input.is_write_lane = true;
        assert_eq!(granted_slots(&record, &config, input, 1).slots(), 1);
        input.request_ceiling = None;
        config.total_workers = 2;
        assert_eq!(granted_slots(&record, &config, input, 9).slots(), 2);
    }

    // spec: MEDULLA-PLACEMENT-R002.3
    #[test]
    fn floor_one_never_overrides_a_zero_hard_limit() {
        use WorkerCapacityRefusal::*;
        let (mut record, config, input) = fixture();
        record.cores = 1;
        assert_eq!(granted_slots(&record, &config, input, 1).slots(), 1);
        let cases = [
            (
                WorkerSizingInput {
                    available_memory_bytes: 0,
                    ..input
                },
                1,
                InsufficientMemory,
            ),
            (
                WorkerSizingInput {
                    available_memory_bytes: config.memory_per_worker_bytes - 1,
                    ..input
                },
                1,
                InsufficientMemory,
            ),
            (
                WorkerSizingInput {
                    request_ceiling: Some(0),
                    ..input
                },
                1,
                RequestCeilingZero,
            ),
            (
                WorkerSizingInput {
                    is_write_lane: true,
                    ..input
                },
                0,
                WriterWidthZero,
            ),
        ];
        for (input, width, reason) in cases {
            let grant = granted_slots(&record, &config, input, width);
            assert_eq!(grant.slots(), 0);
            assert_eq!(grant, WorkerSlotGrant::Refused(reason));
        }
        record.memory_bytes = 0;
        assert_eq!(
            granted_slots(&record, &config, input, 1),
            WorkerSlotGrant::Refused(InsufficientMemory)
        );
    }

    // spec: MEDULLA-PLACEMENT-R002.3
    #[test]
    fn disabled_resources_invalid_policy_and_large_values_are_safe() {
        use WorkerCapacityRefusal::*;
        let (mut record, mut config, input) = fixture();
        record.cores = 0;
        assert_eq!(
            granted_slots(&record, &config, input, 1),
            WorkerSlotGrant::Refused(NoCores)
        );
        record.cores = u32::MAX;
        for (cpu, pool, depth, reason) in [
            (0, 1, 1, CpuShareDisabled),
            (360, 0, 1, PoolDisabled),
            (360, 1, 0, QueueDisabled),
        ] {
            config.cores_per_mille = cpu;
            config.total_workers = pool;
            config.max_queue_depth = depth;
            assert_eq!(
                granted_slots(&record, &config, input, 1),
                WorkerSlotGrant::Refused(reason)
            );
        }
        config.cores_per_mille = 1000;
        config.total_workers = u32::MAX;
        config.max_queue_depth = u64::MAX;
        config.memory_per_worker_bytes = 1;
        record.memory_bytes = u64::MAX;
        let input = WorkerSizingInput {
            available_memory_bytes: u64::MAX,
            ..input
        };
        assert_eq!(granted_slots(&record, &config, input, 0).slots(), u32::MAX);
        config.memory_per_worker_bytes = 0;
        assert_eq!(
            granted_slots(&record, &config, input, 0),
            WorkerSlotGrant::Refused(InvalidConfig(InvalidSchedulerConfig::ZeroMemoryPerWorker))
        );
    }
}
