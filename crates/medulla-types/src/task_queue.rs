//! Typed durable task-queue model with refusals (MEDULLA-DURABLE-KERNEL-R071).
//!
//! Pure data and field validation. The served enqueue, claim and ack methods
//! are the children of `MEDULLA-DURABLE-KERNEL-R072`. A claim carries no lane
//! or partition field (MEDULLA-PLACEMENT-R002.5).

use crate::contract::ServerErrorCode;
use serde::{Deserialize, Serialize};

pub const MAX_QUEUE_NAME_BYTES: usize = 128;
pub const MAX_PAYLOAD_REF_BYTES: usize = 512;
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
pub const MAX_PRIORITY: u8 = 9;
pub const DEFAULT_PRIORITY: u8 = 5;
pub const MIN_LEASE_MS: u64 = 1_000;
pub const MAX_LEASE_MS: u64 = 300_000;
pub const MAX_ACK_DETAIL_BYTES: usize = 1_024;

fn default_priority() -> u8 {
    DEFAULT_PRIORITY
}

/// A task-queue record that failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskQueueError {
    /// The queue name is blank or outside `[a-z0-9._-]{1,128}`.
    QueueNameBlank,
    /// The lease is outside 1 000 to 300 000 ms.
    LeaseZero,
    /// A bound other than the queue name or lease was violated.
    FieldInvalid(&'static str),
    /// An ack names a task the worker never claimed.
    AckNotClaimed,
    /// An ack arrived after the lease expired.
    AckLeaseExpired,
    /// The idempotency key already names a task.
    Duplicate,
}

impl TaskQueueError {
    /// The registered refusal code, where the refusal is one of the closed set.
    pub fn code(self) -> Option<ServerErrorCode> {
        match self {
            Self::QueueNameBlank => Some(ServerErrorCode::TaskQueueNameBlank),
            Self::LeaseZero => Some(ServerErrorCode::TaskLeaseZero),
            Self::AckNotClaimed => Some(ServerErrorCode::TaskAckNotClaimed),
            Self::AckLeaseExpired => Some(ServerErrorCode::TaskAckLeaseExpired),
            Self::Duplicate => Some(ServerErrorCode::TaskDuplicate),
            Self::FieldInvalid(_) => None,
        }
    }
}

impl std::fmt::Display for TaskQueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code(), self) {
            (Some(code), _) => f.write_str(code.as_str()),
            (None, Self::FieldInvalid(field)) => write!(f, "invalid task field: {field}"),
            (None, _) => Ok(()),
        }
    }
}

impl std::error::Error for TaskQueueError {}

/// A request to add one task to a queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskEnqueue {
    pub queue: String,
    pub kind: String,
    pub payload_ref: String,
    pub idempotency_key: String,
    #[serde(default = "default_priority")]
    pub priority: u8,
    #[serde(default)]
    pub not_before_ms: u64,
}

impl TaskEnqueue {
    pub fn validate(&self) -> Result<(), TaskQueueError> {
        validate_queue_name(&self.queue)?;
        if self.kind.trim().is_empty() {
            return Err(TaskQueueError::FieldInvalid("kind"));
        }
        if self.payload_ref.len() > MAX_PAYLOAD_REF_BYTES {
            return Err(TaskQueueError::FieldInvalid("payload_ref"));
        }
        if self.idempotency_key.is_empty() || self.idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES
        {
            return Err(TaskQueueError::FieldInvalid("idempotency_key"));
        }
        if self.priority > MAX_PRIORITY {
            return Err(TaskQueueError::FieldInvalid("priority"));
        }
        Ok(())
    }
}

fn validate_queue_name(name: &str) -> Result<(), TaskQueueError> {
    let valid = !name.is_empty()
        && name.len() <= MAX_QUEUE_NAME_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'));
    valid.then_some(()).ok_or(TaskQueueError::QueueNameBlank)
}

/// A worker's request to lease the next task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskClaim {
    pub worker_id: String,
    pub grant_id: String,
    pub lease_ms: u64,
}

impl TaskClaim {
    pub fn validate(&self) -> Result<(), TaskQueueError> {
        if self.worker_id.trim().is_empty() {
            return Err(TaskQueueError::FieldInvalid("worker_id"));
        }
        if self.grant_id.trim().is_empty() {
            return Err(TaskQueueError::FieldInvalid("grant_id"));
        }
        if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&self.lease_ms) {
            return Err(TaskQueueError::LeaseZero);
        }
        Ok(())
    }
}

/// How a claimed task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TaskOutcome {
    Succeeded,
    Failed,
    Retry,
}

/// A worker's report that a claimed task ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskAck {
    pub task_id: String,
    pub worker_id: String,
    pub outcome: TaskOutcome,
    #[serde(default)]
    pub detail: String,
}

impl TaskAck {
    pub fn validate(&self) -> Result<(), TaskQueueError> {
        if self.task_id.trim().is_empty() {
            return Err(TaskQueueError::FieldInvalid("task_id"));
        }
        if self.worker_id.trim().is_empty() {
            return Err(TaskQueueError::FieldInvalid("worker_id"));
        }
        if self.detail.len() > MAX_ACK_DETAIL_BYTES {
            return Err(TaskQueueError::FieldInvalid("detail"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enqueue() -> TaskEnqueue {
        TaskEnqueue {
            queue: "ingest.general".into(),
            kind: "document".into(),
            payload_ref: "blob:abc".into(),
            idempotency_key: "k1".into(),
            priority: 5,
            not_before_ms: 0,
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R071
    #[test]
    fn each_type_round_trips_and_each_refusal_has_its_code() {
        let task = enqueue();
        task.validate().unwrap();
        let back: TaskEnqueue = serde_json::from_value(serde_json::to_value(&task).unwrap()).unwrap();
        assert_eq!(back, task);
        let defaulted: TaskEnqueue = serde_json::from_value(serde_json::json!({
            "queue": "q", "kind": "k", "payload_ref": "", "idempotency_key": "x"
        }))
        .unwrap();
        assert_eq!(defaulted.priority, DEFAULT_PRIORITY);

        let claim = TaskClaim { worker_id: "w".into(), grant_id: "g".into(), lease_ms: 1_000 };
        claim.validate().unwrap();
        assert_eq!(
            serde_json::from_value::<TaskClaim>(serde_json::to_value(&claim).unwrap()).unwrap(),
            claim
        );
        let claim_json = serde_json::to_value(&claim).unwrap();
        assert!(claim_json.get("lane").is_none() && claim_json.get("partition").is_none());
        let mut with_lane = claim_json;
        with_lane["lane"] = "general".into();
        assert!(serde_json::from_value::<TaskClaim>(with_lane).is_err());

        for outcome in [TaskOutcome::Succeeded, TaskOutcome::Failed, TaskOutcome::Retry] {
            let ack = TaskAck {
                task_id: "t".into(),
                worker_id: "w".into(),
                outcome,
                detail: "d".into(),
            };
            ack.validate().unwrap();
            let packed: TaskAck =
                rmp_serde::from_slice(&rmp_serde::to_vec_named(&ack).unwrap()).unwrap();
            assert_eq!(packed, ack);
        }

        for bad in ["", "Upper", "has space", &"a".repeat(129)] {
            let t = TaskEnqueue { queue: bad.into(), ..enqueue() };
            let e = t.validate().unwrap_err();
            assert_eq!(e.code().unwrap().as_str(), "TASK_QUEUE_NAME_BLANK");
        }
        for lease in [0, 999, 300_001] {
            let c = TaskClaim { worker_id: "w".into(), grant_id: "g".into(), lease_ms: lease };
            assert_eq!(c.validate().unwrap_err().code().unwrap().as_str(), "TASK_LEASE_ZERO");
        }
        assert_eq!(
            TaskEnqueue { payload_ref: "x".repeat(513), ..enqueue() }.validate().unwrap_err(),
            TaskQueueError::FieldInvalid("payload_ref")
        );
        assert_eq!(
            TaskEnqueue { idempotency_key: String::new(), ..enqueue() }.validate().unwrap_err(),
            TaskQueueError::FieldInvalid("idempotency_key")
        );
        assert_eq!(
            TaskEnqueue { priority: 10, ..enqueue() }.validate().unwrap_err(),
            TaskQueueError::FieldInvalid("priority")
        );
        assert_eq!(
            TaskAck { task_id: "t".into(), worker_id: "w".into(), outcome: TaskOutcome::Failed, detail: "x".repeat(1025) }
                .validate()
                .unwrap_err(),
            TaskQueueError::FieldInvalid("detail")
        );
        assert_eq!(TaskQueueError::AckNotClaimed.code().unwrap().as_str(), "TASK_ACK_NOT_CLAIMED");
        assert_eq!(TaskQueueError::AckLeaseExpired.code().unwrap().as_str(), "TASK_ACK_LEASE_EXPIRED");
        assert_eq!(TaskQueueError::Duplicate.code().unwrap().as_str(), "TASK_DUPLICATE");
    }
}
