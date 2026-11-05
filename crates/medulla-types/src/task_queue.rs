//! Typed durable task-queue model with refusals (MEDULLA-DURABLE-KERNEL-R071).
//!
//! The served `TaskQueueEnqueue`/`TaskQueueClaim`/`TaskQueueAck` methods of
//! MEDULLA-DURABLE-KERNEL-R072 carry these types. Each request is validated
//! before it reaches the queue; an out-of-bound field is refused with its
//! registered code and never clamped. A claim carries no lane or partition
//! field (MEDULLA-PLACEMENT-R002.5): placement is the engine's choice.

use serde::{Deserialize, Serialize};

use crate::contract::{EngineErrorCode, ServerErrorCode};

/// Longest queue name, in characters.
pub const TASK_QUEUE_NAME_MAX_CHARS: usize = 128;
/// Longest `payload_ref`, in bytes.
pub const TASK_PAYLOAD_REF_MAX_BYTES: usize = 512;
/// Longest `idempotency_key`, in bytes.
pub const TASK_IDEMPOTENCY_KEY_MAX_BYTES: usize = 256;
/// Highest priority value.
pub const TASK_PRIORITY_MAX: u8 = 9;
/// Default priority.
pub const TASK_PRIORITY_DEFAULT: u8 = 5;
/// Shortest claim lease, in milliseconds.
pub const TASK_LEASE_MIN_MS: u64 = 1_000;
/// Longest claim lease, in milliseconds.
pub const TASK_LEASE_MAX_MS: u64 = 300_000;
/// Longest ack `detail`, in bytes.
pub const TASK_ACK_DETAIL_MAX_BYTES: usize = 1_024;

fn default_priority() -> u8 {
    TASK_PRIORITY_DEFAULT
}

/// Enqueue one task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// Claim the next ready task under a worker grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskClaim {
    pub worker_id: String,
    pub grant_id: String,
    pub lease_ms: u64,
}

/// Terminal or retry outcome of a claimed task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TaskOutcome {
    Succeeded,
    Failed,
    Retry,
}

/// Acknowledge a claimed task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskAck {
    pub task_id: String,
    pub worker_id: String,
    pub outcome: TaskOutcome,
    #[serde(default)]
    pub detail: String,
}

/// A task-queue request refused before it reaches the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidTaskQueueRequest {
    /// The queue name is empty, too long or outside `[a-z0-9._-]`.
    QueueName,
    /// The lease is outside 1 000 to 300 000 ms.
    Lease,
    /// `payload_ref` is longer than 512 bytes.
    PayloadRefTooLong,
    /// `idempotency_key` is empty or longer than 256 bytes.
    IdempotencyKey,
    /// `priority` is above 9.
    Priority,
    /// Ack `detail` is longer than 1 024 bytes.
    DetailTooLong,
}

impl InvalidTaskQueueRequest {
    /// The registered wire code of this refusal.
    pub fn code(self) -> &'static str {
        match self {
            Self::QueueName => ServerErrorCode::TaskQueueNameBlank.as_str(),
            Self::Lease => ServerErrorCode::TaskLeaseZero.as_str(),
            Self::PayloadRefTooLong
            | Self::IdempotencyKey
            | Self::Priority
            | Self::DetailTooLong => EngineErrorCode::InvalidArgument.as_str(),
        }
    }
}

impl std::fmt::Display for InvalidTaskQueueRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            Self::QueueName => "task queue name must be 1 to 128 characters of [a-z0-9._-]",
            Self::Lease => "task lease_ms must be 1000 to 300000",
            Self::PayloadRefTooLong => "task payload_ref must be at most 512 bytes",
            Self::IdempotencyKey => "task idempotency_key must be 1 to 256 bytes",
            Self::Priority => "task priority must be 0 to 9",
            Self::DetailTooLong => "task ack detail must be at most 1024 bytes",
        };
        write!(f, "{}: {detail}", self.code())
    }
}

impl std::error::Error for InvalidTaskQueueRequest {}

fn valid_queue_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= TASK_QUEUE_NAME_MAX_CHARS
        && name
            .chars()
            .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '.' | '_' | '-'))
}

impl TaskEnqueue {
    /// Validate every bound without touching the queue.
    pub fn validate(&self) -> Result<(), InvalidTaskQueueRequest> {
        if !valid_queue_name(&self.queue) {
            return Err(InvalidTaskQueueRequest::QueueName);
        }
        if self.payload_ref.len() > TASK_PAYLOAD_REF_MAX_BYTES {
            return Err(InvalidTaskQueueRequest::PayloadRefTooLong);
        }
        if self.idempotency_key.is_empty()
            || self.idempotency_key.len() > TASK_IDEMPOTENCY_KEY_MAX_BYTES
        {
            return Err(InvalidTaskQueueRequest::IdempotencyKey);
        }
        if self.priority > TASK_PRIORITY_MAX {
            return Err(InvalidTaskQueueRequest::Priority);
        }
        Ok(())
    }
}

impl TaskClaim {
    /// Validate the lease bound.
    pub fn validate(&self) -> Result<(), InvalidTaskQueueRequest> {
        if !(TASK_LEASE_MIN_MS..=TASK_LEASE_MAX_MS).contains(&self.lease_ms) {
            return Err(InvalidTaskQueueRequest::Lease);
        }
        Ok(())
    }
}

impl TaskAck {
    /// Validate the detail bound.
    pub fn validate(&self) -> Result<(), InvalidTaskQueueRequest> {
        if self.detail.len() > TASK_ACK_DETAIL_MAX_BYTES {
            return Err(InvalidTaskQueueRequest::DetailTooLong);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enqueue() -> TaskEnqueue {
        TaskEnqueue {
            queue: "a".repeat(TASK_QUEUE_NAME_MAX_CHARS),
            kind: "ingest".into(),
            payload_ref: "p".repeat(TASK_PAYLOAD_REF_MAX_BYTES),
            idempotency_key: "k".repeat(TASK_IDEMPOTENCY_KEY_MAX_BYTES),
            priority: TASK_PRIORITY_MAX,
            not_before_ms: 0,
        }
    }

    fn round_trip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, value);
    }

    #[test]
    fn each_type_round_trips_and_each_refusal_has_its_code() {
        let ok = enqueue();
        assert_eq!(ok.validate(), Ok(()));
        round_trip(&ok);
        let defaulted: TaskEnqueue = serde_json::from_str(
            r#"{"queue":"q.1_x-y","kind":"k","payload_ref":"","idempotency_key":"i"}"#,
        )
        .expect("defaults");
        assert_eq!(defaulted.priority, TASK_PRIORITY_DEFAULT);
        for lease_ms in [TASK_LEASE_MIN_MS, TASK_LEASE_MAX_MS] {
            let claim = TaskClaim {
                worker_id: "w".into(),
                grant_id: "g".into(),
                lease_ms,
            };
            assert_eq!(claim.validate(), Ok(()));
            round_trip(&claim);
        }
        for outcome in [
            TaskOutcome::Succeeded,
            TaskOutcome::Failed,
            TaskOutcome::Retry,
        ] {
            let ack = TaskAck {
                task_id: "t".into(),
                worker_id: "w".into(),
                outcome,
                detail: "d".repeat(TASK_ACK_DETAIL_MAX_BYTES),
            };
            assert_eq!(ack.validate(), Ok(()));
            round_trip(&ack);
        }
        let claim_shape = serde_json::from_str::<TaskClaim>(
            r#"{"worker_id":"w","grant_id":"g","lease_ms":1000,"lane":"fast"}"#,
        );
        assert!(claim_shape.is_err(), "a claim carries no lane field");

        let refused = |mutate: fn(&mut TaskEnqueue)| {
            let mut req = enqueue();
            mutate(&mut req);
            req.validate().expect_err("refused").code()
        };
        assert_eq!(refused(|r| r.queue.clear()), "TASK_QUEUE_NAME_BLANK");
        assert_eq!(refused(|r| r.queue.push('a')), "TASK_QUEUE_NAME_BLANK");
        assert_eq!(refused(|r| r.queue = "Q".into()), "TASK_QUEUE_NAME_BLANK");
        assert_eq!(refused(|r| r.payload_ref.push('p')), "INVALID_ARGUMENT");
        assert_eq!(refused(|r| r.idempotency_key.clear()), "INVALID_ARGUMENT");
        assert_eq!(refused(|r| r.idempotency_key.push('k')), "INVALID_ARGUMENT");
        assert_eq!(refused(|r| r.priority = 10), "INVALID_ARGUMENT");
        for lease_ms in [0, TASK_LEASE_MIN_MS - 1, TASK_LEASE_MAX_MS + 1] {
            let claim = TaskClaim {
                worker_id: "w".into(),
                grant_id: "g".into(),
                lease_ms,
            };
            assert_eq!(
                claim.validate().expect_err("lease").code(),
                "TASK_LEASE_ZERO"
            );
        }
        let long = TaskAck {
            task_id: "t".into(),
            worker_id: "w".into(),
            outcome: TaskOutcome::Failed,
            detail: "d".repeat(TASK_ACK_DETAIL_MAX_BYTES + 1),
        };
        assert_eq!(
            long.validate().expect_err("detail").code(),
            "INVALID_ARGUMENT"
        );
        for code in [
            ServerErrorCode::TaskQueueNameBlank,
            ServerErrorCode::TaskLeaseZero,
            ServerErrorCode::TaskAckNotClaimed,
            ServerErrorCode::TaskAckLeaseExpired,
            ServerErrorCode::TaskDuplicate,
        ] {
            assert!(ServerErrorCode::ALL.contains(&code));
        }
        assert_eq!(
            ServerErrorCode::TaskAckNotClaimed.as_str(),
            "TASK_ACK_NOT_CLAIMED"
        );
        assert_eq!(
            ServerErrorCode::TaskAckLeaseExpired.as_str(),
            "TASK_ACK_LEASE_EXPIRED"
        );
        assert_eq!(ServerErrorCode::TaskDuplicate.as_str(), "TASK_DUPLICATE");
    }
}
