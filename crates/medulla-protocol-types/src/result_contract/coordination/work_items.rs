//! WorkItem lease transition result bodies of the `coordination` domain.

use serde::{Deserialize, Serialize};

/// Why a `RenewWorkItemLease` renewed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LeaseRenewalRefusal {
    /// No WorkItem has this id.
    Missing,
    /// The caller no longer holds the live lease it named.
    Fenced,
}

/// `RenewWorkItemLease`. A renewal carries the renewed lease; a refusal carries only
/// its reason. Every outcome names the WorkItem rows it changed, so the serving
/// projection can be refreshed from authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkItemLeaseRenewal {
    pub renewed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<LeaseRenewalRefusal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fencing_token: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at_ms: Option<u64>,
    /// A cancel reached this in-flight item: stop at the next step boundary
    /// and commit `cancelled`.
    #[serde(default)]
    pub cancel_requested: bool,
    pub changed_work_item_ids: Vec<String>,
}

/// The outcome a `CommitWorkItemResult` answers with: a reply to one call,
/// not a lifecycle (the WorkItem's own lifecycle is `WorkItemStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WorkItemCommitOutcome {
    /// No WorkItem with this id is visible to the tenant.
    Missing,
    /// The WorkItem was already terminal; nothing changed.
    Noop,
    /// The caller no longer holds the live lease it named.
    Fenced,
    Succeeded,
    Failed,
    Cancelled,
    /// A retryable failure that exhausted its attempts.
    DeadLetter,
    /// A retryable failure below its attempt ceiling, rescheduled with backoff.
    RetryScheduled,
}

/// The outcome a `CancelWorkItem` answers with: a reply to one call, not a
/// lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WorkItemCancelOutcome {
    /// No WorkItem with this id is visible to the tenant.
    Missing,
    /// The WorkItem was already terminal; nothing changed.
    Noop,
    /// The WorkItem is running under a live lease and cannot be cancelled now.
    InFlight,
    /// The WorkItem is in a state cancellation does not apply to.
    NotCancellable,
    Cancelled,
}

/// A terminal-direction WorkItem transition (`CommitWorkItemResult`,
/// `CancelWorkItem`). The WorkItem id and fence are present once the item was found;
/// the fence is present only when the transition applied.
///
/// Each instantiation gets its own schema name: without the rename schemars
/// named the second one `WorkItemTransition2`, a name no author chose (caught
/// by `no_schema_definition_name_is_shared_by_distinct_types`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    feature = "contract-schema",
    schemars(rename = "WorkItemTransition_{Outcome}")
)]
pub struct WorkItemTransition<Outcome> {
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fencing_token: Option<u64>,
    pub changed_work_item_ids: Vec<String>,
}

/// The status a `DeferWorkItem` answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WorkItemDeferStatus {
    /// No WorkItem with this id exists.
    Missing,
    /// The caller no longer holds the live lease it named.
    Fenced,
    /// The lease was released and the WorkItem returned to `ready` for a later retry.
    Deferred,
}

/// `DeferWorkItem`. A deferral carries the bumped fence and the retry schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkItemDeferral {
    pub status: WorkItemDeferStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fencing_token: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_count: Option<u64>,
    pub changed_work_item_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R875.2
    /// Verifies MEDULLA-DURABLE-KERNEL-R875.2: the commit and cancel replies carry
    /// their per-call answer under `outcome`, keep their wire words, and no
    /// longer answer under the lifecycle key `status`.
    #[test]
    fn work_item_reply_outcomes_are_not_lifecycle_fields() {
        let commit_words = [
            (WorkItemCommitOutcome::Missing, "missing"),
            (WorkItemCommitOutcome::Noop, "noop"),
            (WorkItemCommitOutcome::Fenced, "fenced"),
            (WorkItemCommitOutcome::Succeeded, "succeeded"),
            (WorkItemCommitOutcome::Failed, "failed"),
            (WorkItemCommitOutcome::Cancelled, "cancelled"),
            (WorkItemCommitOutcome::DeadLetter, "dead_letter"),
            (WorkItemCommitOutcome::RetryScheduled, "retry_scheduled"),
        ];
        for (outcome, word) in commit_words {
            let reply = WorkItemTransition {
                outcome,
                work_item_id: Some("wi-1".to_string()),
                lease_epoch: None,
                fencing_token: None,
                changed_work_item_ids: Vec::new(),
            };
            let wire = serde_json::to_value(&reply).unwrap();
            assert_eq!(wire["outcome"], word);
            assert!(wire.get("status").is_none(), "{wire}");
            let back: WorkItemTransition<WorkItemCommitOutcome> =
                serde_json::from_value(wire).unwrap();
            assert_eq!(back, reply);
        }
        let cancel_words = [
            (WorkItemCancelOutcome::Missing, "missing"),
            (WorkItemCancelOutcome::Noop, "noop"),
            (WorkItemCancelOutcome::InFlight, "in_flight"),
            (WorkItemCancelOutcome::NotCancellable, "not_cancellable"),
            (WorkItemCancelOutcome::Cancelled, "cancelled"),
        ];
        for (outcome, word) in cancel_words {
            let reply = WorkItemTransition {
                outcome,
                work_item_id: None,
                lease_epoch: None,
                fencing_token: None,
                changed_work_item_ids: Vec::new(),
            };
            let wire = serde_json::to_value(&reply).unwrap();
            assert_eq!(wire["outcome"], word);
            assert!(wire.get("status").is_none(), "{wire}");
        }

        // The old lifecycle key is gone: a reply that names `status` instead of
        // `outcome` does not decode (hard cutover, no alias).
        let old = serde_json::json!({"status": "fenced", "changed_work_item_ids": []});
        assert!(serde_json::from_value::<WorkItemTransition<WorkItemCommitOutcome>>(old).is_err());
    }
}
