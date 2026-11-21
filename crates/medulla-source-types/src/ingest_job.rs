//! Typed ingest job envelope, lanes, partitioning and retry policy
//! (MEDULLA-INGEST-WORKER-R001.1, R001.2, R001.3, R001.4, R001.7, R001.8).
//!
//! Pure data and pure functions. The served queue write, the claim/ack path
//! and the dead-letter routing are later rows (R001.9 to R001.12).

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The kinds of work the ingest worker drains. The set is closed: an unknown
/// kind fails deserialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IngestJobKind {
    Document,
    Codebase,
    SkillWorkflows,
    CapabilityHydration,
}

/// The queue lane a job kind runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IngestLane {
    General,
    Hydration,
}

impl IngestLane {
    /// The served task-queue name for this lane.
    pub fn queue_name(self) -> &'static str {
        match self {
            Self::General => "ingest",
            Self::Hydration => "ingest-hydration",
        }
    }

    /// The scheduler lane this lane runs on (MEDULLA-INGEST-WORKER-R604.1).
    pub fn lane_id(self) -> medulla_types::ingest_lanes::Lane {
        match self {
            Self::General => medulla_types::ingest_lanes::Lane::general(),
            Self::Hydration => medulla_types::ingest_lanes::Lane::hydration(),
        }
    }
}

/// Hydration kinds run on the reserved hydration lane; everything else is general.
pub fn lane_for(kind: IngestJobKind) -> IngestLane {
    match kind {
        IngestJobKind::SkillWorkflows | IngestJobKind::CapabilityHydration => IngestLane::Hydration,
        IngestJobKind::Document | IngestJobKind::Codebase => IngestLane::General,
    }
}

/// One unit of ingest work as the queue carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IngestJobEnvelope {
    pub job_id: String,
    pub tenant: String,
    pub corpus_key: String,
    pub kind: IngestJobKind,
    pub target: String,
    pub attempt: u32,
    /// The drain page this job fetches, when it is one page of a source drain
    /// (MEDULLA-INGEST-WORKER-R605.1).
    #[serde(default)]
    pub page: Option<medulla_ingestion_types::ingest_drain::PageCheckpoint>,
}

/// Why an ingest job envelope or retry policy is refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IngestJobError {
    #[error("ingest job_id must not be blank")]
    BlankJobId,
    #[error("ingest target must not be blank")]
    BlankTarget,
    #[error("unknown ingest job kind")]
    UnknownKind,
    #[error("ingest attempt {attempt} exceeds the retry ceiling {max_attempts}")]
    AttemptAboveCeiling { attempt: u32, max_attempts: u32 },
    #[error("ingest retry policy must allow at least one attempt")]
    ZeroAttempts,
    #[error("ingest retry base delay exceeds the max delay")]
    BaseAboveMax,
    #[error("ingest retry jitter must be a finite fraction in [0, 1]")]
    InvalidJitter,
    #[error("ingest target must not embed credentials")]
    TargetHasCredential,
}

impl IngestJobEnvelope {
    /// Parse and validate an envelope from JSON, mapping an unknown kind to a
    /// typed refusal instead of an opaque decode error.
    pub fn from_json(
        value: serde_json::Value,
        policy: &IngestRetryPolicy,
    ) -> Result<Self, IngestJobError> {
        if let Some(kind) = value.get("kind") {
            if serde_json::from_value::<IngestJobKind>(kind.clone()).is_err() {
                return Err(IngestJobError::UnknownKind);
            }
        }
        let envelope: Self =
            serde_json::from_value(value).map_err(|_| IngestJobError::UnknownKind)?;
        envelope.validate(policy)?;
        Ok(envelope)
    }

    /// Refuse a blank id or target and an attempt above the retry ceiling.
    pub fn validate(&self, policy: &IngestRetryPolicy) -> Result<(), IngestJobError> {
        if self.job_id.trim().is_empty() {
            return Err(IngestJobError::BlankJobId);
        }
        if self.target.trim().is_empty() {
            return Err(IngestJobError::BlankTarget);
        }
        if self.attempt > policy.max_attempts {
            return Err(IngestJobError::AttemptAboveCeiling {
                attempt: self.attempt,
                max_attempts: policy.max_attempts,
            });
        }
        Ok(())
    }

    pub fn lane(&self) -> IngestLane {
        lane_for(self.kind)
    }
}

/// The queue partition: tenant, then corpus, then kind. Jobs that share a key
/// keep their order; different keys never serialize against each other.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IngestPartitionKey {
    pub tenant: String,
    pub corpus_key: String,
    pub kind: &'static str,
}

impl IngestPartitionKey {
    /// The stable text form the queue stores as task metadata: tenant, corpus, kind.
    pub fn render(&self) -> String {
        format!("{}/{}/{}", self.tenant, self.corpus_key, self.kind)
    }
}

pub fn partition_key(envelope: &IngestJobEnvelope) -> IngestPartitionKey {
    let kind = match envelope.kind {
        IngestJobKind::Document => "document",
        IngestJobKind::Codebase => "codebase",
        IngestJobKind::SkillWorkflows => "skill_workflows",
        IngestJobKind::CapabilityHydration => "capability_hydration",
    };
    IngestPartitionKey {
        tenant: envelope.tenant.clone(),
        corpus_key: envelope.corpus_key.clone(),
        kind,
    }
}

/// Bounded exponential retry with jitter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IngestRetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    /// Fraction of the exponential delay that may be removed at random.
    pub jitter: f64,
}

impl IngestRetryPolicy {
    /// The policy the scheduler configuration carries as integer limits
    /// (MEDULLA-INGEST-WORKER-R001.4.1).
    pub fn from_limits(limits: &medulla_work_types::scheduler_config::IngestRetryLimits) -> Self {
        Self {
            max_attempts: limits.max_attempts,
            base_delay_ms: limits.base_delay_ms,
            max_delay_ms: limits.max_delay_ms,
            jitter: f64::from(limits.jitter_milli) / 1000.0,
        }
    }

    pub fn validate(&self) -> Result<(), IngestJobError> {
        if self.max_attempts == 0 {
            return Err(IngestJobError::ZeroAttempts);
        }
        if self.base_delay_ms > self.max_delay_ms {
            return Err(IngestJobError::BaseAboveMax);
        }
        if !self.jitter.is_finite() || !(0.0..=1.0).contains(&self.jitter) {
            return Err(IngestJobError::InvalidJitter);
        }
        Ok(())
    }
}

fn mix(seed: u64, attempt: u32) -> u64 {
    // splitmix64 over the seed and attempt: deterministic and dependency-free.
    let mut z = seed ^ (u64::from(attempt)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Delay before redelivering `attempt` (1-based). The un-jittered ceiling is
/// `min(max_delay, base_delay * 2^(attempt-1))`; jitter removes a seeded share
/// of at most `jitter` of it so workers do not retry in lockstep.
pub fn next_delay(policy: &IngestRetryPolicy, attempt: u32, seed: u64) -> Duration {
    let shift = attempt.saturating_sub(1).min(63);
    let ceiling = policy
        .base_delay_ms
        .saturating_mul(1u64 << shift)
        .min(policy.max_delay_ms);
    let unit = (mix(seed, attempt) >> 11) as f64 / (1u64 << 53) as f64;
    let removed = (ceiling as f64 * policy.jitter.clamp(0.0, 1.0) * unit) as u64;
    Duration::from_millis(ceiling - removed.min(ceiling))
}

/// What an ack of an ingest task does to the task (MEDULLA-INGEST-WORKER-R001.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestAckDecision {
    /// The task is done.
    Complete,
    /// Redeliver after `delay_ms`.
    Redeliver { delay_ms: u64 },
    /// The task exhausted its attempts: fail it and park its dead-letter item.
    DeadLetter,
}

/// The jitter seed of one task: the first 8 bytes of its 64-hex digest id, big-endian. An id
/// that is not hex, which no ingest task has, seeds 0.
fn task_id_seed(task_id: &str) -> u64 {
    task_id
        .get(..16)
        .and_then(|head| u64::from_str_radix(head, 16).ok())
        .unwrap_or(0)
}

/// Decide the ack of one ingest task. `attempt` is the job's 1-based `attempts_made`, counted
/// at each claim. `Succeeded` completes; `Failed` and `Retry` redeliver while
/// `attempt < max_attempts` and dead-letter from `attempt >= max_attempts`
/// (MEDULLA-INGEST-WORKER-R001.9).
pub fn on_ingest_ack(
    policy: &IngestRetryPolicy,
    attempt: u32,
    task_id: &str,
    outcome: medulla_work_types::task_queue::TaskOutcome,
) -> IngestAckDecision {
    use medulla_work_types::task_queue::TaskOutcome;
    match outcome {
        TaskOutcome::Succeeded => IngestAckDecision::Complete,
        TaskOutcome::Failed | TaskOutcome::Retry if attempt >= policy.max_attempts => {
            IngestAckDecision::DeadLetter
        }
        TaskOutcome::Failed | TaskOutcome::Retry => IngestAckDecision::Redeliver {
            delay_ms: next_delay(policy, attempt, task_id_seed(task_id)).as_millis() as u64,
        },
    }
}

/// The receipt for a served `IngestEnqueue`: the task id, its queue, its
/// partition and whether the write deduplicated
/// (MEDULLA-INGEST-WORKER-R001.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IngestEnqueueReceipt {
    /// 64 lowercase hex characters; stable for one (tenant, corpus, job).
    pub task_id: String,
    /// The served task-queue name the task is claimed from.
    pub queue: String,
    /// The partition the task orders within its queue, rendered
    /// as tenant/corpus/kind.
    pub partition: String,
    /// True when the task already existed and the write did nothing.
    pub deduplicated: bool,
}

/// One reserved ingest queue's load: tasks enqueued or claimed and not yet acknowledged, and the
/// age of the oldest one (MEDULLA-INGEST-WORKER-R001.11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IngestQueueDepthRow {
    pub queue: String,
    /// Queued plus running tasks of the verified tenant.
    pub depth: u64,
    /// Milliseconds since the oldest pending task was enqueued; 0 for an empty queue.
    pub oldest_age_ms: u64,
    /// Tasks of the verified tenant's queue parked as dead letters (MEDULLA-INGEST-WORKER-R001.9).
    pub dead_letters: u64,
}

/// The reply of `IngestQueueDepth`, `ingest` before `ingest-hydration`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IngestQueueDepthReport {
    pub queues: Vec<IngestQueueDepthRow>,
}

/// Largest canonical envelope payload one ingest task carries, in bytes
/// (MEDULLA-INGEST-WORKER-R001.4.1).
pub const INGEST_TASK_PAYLOAD_MAX_BYTES: usize = 16384;

/// The lane whose reserved task queue is named `queue`, or `None` when the
/// queue is not an ingest queue (MEDULLA-INGEST-WORKER-R001.4.1).
pub fn ingest_lane_of_queue(queue: &str) -> Option<IngestLane> {
    [IngestLane::General, IngestLane::Hydration]
        .into_iter()
        .find(|lane| lane.queue_name() == queue)
}

/// The domain tag under which the ingest task id is framed.
pub const INGEST_TASK_ID_DOMAIN: &[u8] = b"eg/ingest-task-id";

/// The deterministic task id of an envelope: SHA-256 framed under
/// `eg/ingest-task-id` over tenant, corpus key and job id
/// (MEDULLA-INGEST-WORKER-R001.4).
pub fn task_id(envelope: &IngestJobEnvelope) -> String {
    medulla_types::contract::Digest256::framed(
        INGEST_TASK_ID_DOMAIN,
        &[
            envelope.tenant.as_bytes(),
            envelope.corpus_key.as_bytes(),
            envelope.job_id.as_bytes(),
        ],
    )
    .expect("string lengths always fit the digest framing limit")
    .to_hex()
}

/// A target embeds a credential when the authority of a `scheme://` URI
/// carries userinfo before the host (R001.4).
pub fn target_embeds_credentials(target: &str) -> bool {
    target
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .is_some_and(|authority| authority.contains('@'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy() -> IngestRetryPolicy {
        IngestRetryPolicy {
            max_attempts: 5,
            base_delay_ms: 100,
            max_delay_ms: 2_000,
            jitter: 0.5,
        }
    }

    fn envelope() -> IngestJobEnvelope {
        IngestJobEnvelope {
            job_id: "job-1".into(),
            tenant: "t1".into(),
            corpus_key: "repo-a".into(),
            kind: IngestJobKind::Document,
            target: "docs/readme.md".into(),
            attempt: 1,
            page: None,
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R001.9
    #[test]
    fn ack_decision_redelivers_then_dead_letters() {
        use medulla_work_types::task_queue::TaskOutcome;
        let policy = IngestRetryPolicy {
            max_attempts: 5,
            base_delay_ms: 1_000,
            max_delay_ms: 60_000,
            jitter: 0.2,
        };
        let id = task_id(&envelope());
        for outcome in [TaskOutcome::Failed, TaskOutcome::Retry] {
            for attempt in [1u32, 4] {
                match on_ingest_ack(&policy, attempt, &id, outcome) {
                    IngestAckDecision::Redeliver { delay_ms } => {
                        let ceiling = 1_000u64 << (attempt - 1);
                        assert!(delay_ms <= ceiling && delay_ms * 10 >= ceiling * 8, "{delay_ms}");
                        // The same attempt and task always draw the same delay.
                        assert_eq!(
                            on_ingest_ack(&policy, attempt, &id, outcome),
                            IngestAckDecision::Redeliver { delay_ms }
                        );
                    }
                    other => panic!("attempt {attempt} must redeliver, got {other:?}"),
                }
            }
            assert_eq!(
                on_ingest_ack(&policy, 5, &id, outcome),
                IngestAckDecision::DeadLetter
            );
            assert_eq!(
                on_ingest_ack(&policy, 6, &id, outcome),
                IngestAckDecision::DeadLetter
            );
        }
        for attempt in [1u32, 4, 5] {
            assert_eq!(
                on_ingest_ack(&policy, attempt, &id, TaskOutcome::Succeeded),
                IngestAckDecision::Complete
            );
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R001.1
    /// Verifies MEDULLA-INGEST-WORKER-R001.1: Typed ingest job envelope with refusal tests.
    #[test]
    fn envelope_round_trips_and_refuses_invalid_shapes() {
        let ok = envelope();
        assert_eq!(ok.validate(&policy()), Ok(()));
        let encoded = serde_json::to_value(&ok).unwrap();
        assert_eq!(encoded["kind"], "document");
        assert_eq!(
            IngestJobEnvelope::from_json(encoded.clone(), &policy()),
            Ok(ok.clone())
        );

        let blank_id = IngestJobEnvelope {
            job_id: " ".into(),
            ..ok.clone()
        };
        assert_eq!(
            blank_id.validate(&policy()),
            Err(IngestJobError::BlankJobId)
        );
        let blank_target = IngestJobEnvelope {
            target: String::new(),
            ..ok.clone()
        };
        assert_eq!(
            blank_target.validate(&policy()),
            Err(IngestJobError::BlankTarget)
        );
        let over = IngestJobEnvelope {
            attempt: 6,
            ..ok.clone()
        };
        assert_eq!(
            over.validate(&policy()),
            Err(IngestJobError::AttemptAboveCeiling {
                attempt: 6,
                max_attempts: 5
            })
        );
        let mut unknown = encoded;
        unknown["kind"] = json!("analytics");
        assert_eq!(
            IngestJobEnvelope::from_json(unknown, &policy()),
            Err(IngestJobError::UnknownKind)
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R001.2
    /// Verifies MEDULLA-INGEST-WORKER-R001.2: Typed job kinds and lane derivation.
    #[test]
    fn lane_for_every_kind() {
        assert_eq!(lane_for(IngestJobKind::Document), IngestLane::General);
        assert_eq!(lane_for(IngestJobKind::Codebase), IngestLane::General);
        assert_eq!(
            lane_for(IngestJobKind::SkillWorkflows),
            IngestLane::Hydration
        );
        assert_eq!(
            lane_for(IngestJobKind::CapabilityHydration),
            IngestLane::Hydration
        );
        assert_eq!(IngestLane::General.queue_name(), "ingest");
        assert_eq!(IngestLane::Hydration.queue_name(), "ingest-hydration");
    }

    // spec: MEDULLA-INGEST-WORKER-R001.3
    /// Verifies MEDULLA-INGEST-WORKER-R001.3: Partition key function.
    #[test]
    fn partition_key_orders_per_tenant_and_corpus() {
        let a = envelope();
        let b = IngestJobEnvelope {
            job_id: "job-2".into(),
            target: "docs/other.md".into(),
            ..a.clone()
        };
        assert_eq!(partition_key(&a), partition_key(&b));
        let other_tenant = IngestJobEnvelope {
            tenant: "t2".into(),
            ..a.clone()
        };
        assert_ne!(partition_key(&a), partition_key(&other_tenant));
        let other_corpus = IngestJobEnvelope {
            corpus_key: "repo-b".into(),
            ..a.clone()
        };
        assert_ne!(partition_key(&a), partition_key(&other_corpus));
        let key = partition_key(&a);
        assert_eq!(
            (key.tenant.as_str(), key.corpus_key.as_str(), key.kind),
            ("t1", "repo-a", "document")
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R001.7
    #[test]
    fn retry_policy_refusals() {
        assert_eq!(policy().validate(), Ok(()));
        let zero = IngestRetryPolicy {
            max_attempts: 0,
            ..policy()
        };
        assert_eq!(zero.validate(), Err(IngestJobError::ZeroAttempts));
        let inverted = IngestRetryPolicy {
            base_delay_ms: 3_000,
            ..policy()
        };
        assert_eq!(inverted.validate(), Err(IngestJobError::BaseAboveMax));
        let negative = IngestRetryPolicy {
            jitter: -0.1,
            ..policy()
        };
        assert_eq!(negative.validate(), Err(IngestJobError::InvalidJitter));
        let nan = IngestRetryPolicy {
            jitter: f64::NAN,
            ..policy()
        };
        assert_eq!(nan.validate(), Err(IngestJobError::InvalidJitter));
    }

    // spec: MEDULLA-INGEST-WORKER-R001.8
    #[test]
    fn next_delay_is_bounded_growing_and_seeded() {
        let p = policy();
        for attempt in 1..=10 {
            for seed in 0..32 {
                let delay = next_delay(&p, attempt, seed).as_millis() as u64;
                let ceiling = (100u64 << (attempt - 1)).min(2_000);
                assert!(
                    delay <= ceiling && delay >= ceiling / 2,
                    "{attempt} {delay}"
                );
            }
        }
        let flat = IngestRetryPolicy {
            jitter: 0.0,
            ..p.clone()
        };
        let delays: Vec<u64> = (1..=8)
            .map(|a| next_delay(&flat, a, 7).as_millis() as u64)
            .collect();
        assert!(delays.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(delays[0], 100);
        assert_eq!(*delays.last().unwrap(), 2_000);
        assert_ne!(next_delay(&p, 3, 1), next_delay(&p, 3, 2));
        assert_eq!(next_delay(&p, 3, 1), next_delay(&p, 3, 1));
        assert!(next_delay(&p, u32::MAX, 9) <= Duration::from_millis(2_000));
    }

    // spec: MEDULLA-INGEST-WORKER-R001.4
    #[test]
    fn task_id_is_stable() {
        let a = envelope();
        let b = IngestJobEnvelope {
            target: "docs/other.md".into(),
            attempt: 2,
            ..a.clone()
        };
        let id_a = task_id(&a);
        assert_eq!(id_a.len(), 64);
        assert!(id_a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // Stable across the fields that do not name the job.
        assert_eq!(task_id(&a), id_a);
        assert_eq!(task_id(&a), task_id(&b));
        // Distinct when the tenant, corpus or job identity changes.
        let other_tenant = IngestJobEnvelope {
            tenant: "t2".into(),
            ..a.clone()
        };
        let other_corpus = IngestJobEnvelope {
            corpus_key: "repo-b".into(),
            ..a.clone()
        };
        let other_job = IngestJobEnvelope {
            job_id: "job-2".into(),
            ..a.clone()
        };
        for other in [other_tenant, other_corpus, other_job] {
            assert_ne!(task_id(&other), id_a);
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R001.4
    #[test]
    fn credential_targets_are_refused() {
        // Built at compile time so no credential-shaped literal is tracked.
        assert!(target_embeds_credentials(concat!(
            "postgres://user:pass",
            "@host/db"
        )));
        assert!(target_embeds_credentials(concat!(
            "s3://key",
            "@bucket/object"
        )));
        assert!(target_embeds_credentials(concat!("amqp://", "@host/queue")));
        assert!(!target_embeds_credentials("postgres://host/db"));
        assert!(!target_embeds_credentials("host@not-a-uri"));
        assert!(!target_embeds_credentials("docs/readme.md"));
    }

    // spec: MEDULLA-INGEST-WORKER-R001.4.1
    #[test]
    fn reserved_queues_map_to_lanes_and_limits_build_the_policy() {
        assert_eq!(ingest_lane_of_queue("ingest"), Some(IngestLane::General));
        assert_eq!(
            ingest_lane_of_queue("ingest-hydration"),
            Some(IngestLane::Hydration)
        );
        assert_eq!(ingest_lane_of_queue("other"), None);
        let policy = IngestRetryPolicy::from_limits(
            &medulla_work_types::scheduler_config::IngestRetryLimits::default(),
        );
        assert_eq!(policy.max_attempts, 5);
        assert_eq!(policy.base_delay_ms, 1000);
        assert_eq!(policy.max_delay_ms, 60000);
        assert!((policy.jitter - 0.2).abs() < f64::EPSILON);
        assert!(policy.validate().is_ok());
    }
}
