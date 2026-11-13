//! Fan-out aggregation (MEDULLA-DECISION-ENGINE-R134.100).
//!
//! Medulla applies the plan's already-selected [`AggregationOp`] to the
//! complete receipt set of a fan-out. It adds no fan-out width, no concurrency
//! policy and no second aggregation authority: the harness owns the durable
//! instance receipts and the capacity refusals.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{plan_explicit_tasks, AggregationOp, TaskPlan, TaskPlanAggregation, TaskPlanProposal};
use crate::decision::delegation::digest_parts;
use crate::decision::knob_families::fanout::FanoutKnobs;
use crate::decision::record::outcome::{DecisionRefusal, RefusalCode};
use medulla_types::contract::BoundedVec;

/// The most subtasks one fan-out aggregates.
pub const MAX_FANOUT_TASKS: usize = 64;

/// A result is missing, or an ok result carries no digest, or a failed one names no gap.
pub const FANOUT_CHILD_MISSING: RefusalCode = RefusalCode::new("FANOUT_CHILD_MISSING");
/// A task id appears more than once in the result set.
pub const FANOUT_CHILD_DUPLICATE: RefusalCode = RefusalCode::new("FANOUT_CHILD_DUPLICATE");
/// A result names a task the plan did not expect, or a receipt the caller cannot see.
pub const FANOUT_CHILD_UNKNOWN: RefusalCode = RefusalCode::new("FANOUT_CHILD_UNKNOWN");
/// The plan digest or the plan's aggregation is not a valid selection.
pub const FANOUT_PLAN_MISMATCH: RefusalCode = RefusalCode::new("FANOUT_PLAN_MISMATCH");
/// Every child failed: there is no valid result to aggregate.
pub const FANOUT_NO_VALID_RESULT: RefusalCode = RefusalCode::new("FANOUT_NO_VALID_RESULT");

/// A child returned raw output instead of an accepted summary (MEDULLA-DECISION-ENGINE-R134.10).
pub const RAW_RETURN_REFUSED: RefusalCode = RefusalCode::new("RAW_RETURN_REFUSED");
/// One child summary is larger than `fanout.max_child_summary_tokens`.
pub const SUMMARY_OVERSIZED: RefusalCode = RefusalCode::new("SUMMARY_OVERSIZED");
/// The summaries together exceed the parent's remaining budget.
pub const SUMMARY_BUDGET_EXCEEDED: RefusalCode = RefusalCode::new("SUMMARY_BUDGET_EXCEEDED");
/// A summary was counted with a different tokenizer than the budget's.
pub const TOKENIZER_MISMATCH: RefusalCode = RefusalCode::new("TOKENIZER_MISMATCH");
/// The plan declares no final artifact, so it has no designated coordinator delivery
/// (MEDULLA-DECISION-ENGINE-R134.101).
pub const COORDINATOR_ABSENT: RefusalCode = RefusalCode::new("COORDINATOR_ABSENT");
/// The finalizer is not the plan's output task.
pub const NON_COORDINATOR_FINALIZE: RefusalCode = RefusalCode::new("NON_COORDINATOR_FINALIZE");
/// The delivered artifact's schema is not the one the plan declares.
pub const ARTIFACT_SCHEMA_MISMATCH: RefusalCode = RefusalCode::new("ARTIFACT_SCHEMA_MISMATCH");
/// The delivered artifact's digest is not the digest of the stored readback.
pub const ARTIFACT_DIGEST_MISMATCH: RefusalCode = RefusalCode::new("ARTIFACT_DIGEST_MISMATCH");

/// The `REFUSALS` table of the family, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    RAW_RETURN_REFUSED,
    SUMMARY_OVERSIZED,
    SUMMARY_BUDGET_EXCEEDED,
    TOKENIZER_MISMATCH,
    COORDINATOR_ABSENT,
    NON_COORDINATOR_FINALIZE,
    ARTIFACT_SCHEMA_MISMATCH,
    ARTIFACT_DIGEST_MISMATCH,
    FANOUT_CHILD_MISSING,
    FANOUT_CHILD_DUPLICATE,
    FANOUT_CHILD_UNKNOWN,
    FANOUT_PLAN_MISMATCH,
    FANOUT_NO_VALID_RESULT,
];

/// How one subtask ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FanoutStatus {
    Ok,
    Failed,
}

impl FanoutStatus {
    /// The wire name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}

/// The summary a child returns in place of raw output
/// (MEDULLA-DECISION-ENGINE-R134.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChildSummary {
    /// `"summary"` for an accepted return; any other kind is raw output.
    pub artifact_kind: String,
    pub schema_id: String,
    pub schema_version: u32,
    pub summary_digest: String,
    pub summary_tokens: u64,
    pub tokenizer_revision: String,
}

/// The parent's summary budget (MEDULLA-DECISION-ENGINE-R134.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FanoutBudget {
    pub parent_remaining_tokens: u64,
    /// Absent: `knob fanout.max_summary_tokens` applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_summary_tokens: Option<u64>,
    pub tokenizer_revision: String,
}

/// What the coordinator reports after delivering the final artifact
/// (MEDULLA-DECISION-ENGINE-R134.101).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FinalArtifactReceipt {
    pub finalizer_task_id: String,
    pub schema_id: String,
    pub schema_version: u32,
    pub kind: String,
    pub artifact_digest: String,
    /// The graph node holding the stored artifact; its `digest` is read back.
    pub readback_id: String,
}

/// The receipt of one subtask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FanoutSubtaskResult {
    pub task_id: String,
    pub status: FanoutStatus,
    /// The output digest; present exactly when `status` is `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_digest: Option<String>,
    pub receipt_id: String,
    pub receipt_digest: String,
    /// Why the subtask failed; present exactly when `status` is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_gap: Option<String>,
    /// The summary an ok child returns; required when the request carries a budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<ChildSummary>,
}

/// The complete receipt set of one fan-out, with the plan's aggregation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FanoutAggregateRequest {
    /// SHA-256 of the plan, 64 lowercase hex characters.
    pub plan_digest: String,
    pub aggregation: TaskPlanAggregation,
    pub expected_task_ids: BoundedVec<String, MAX_FANOUT_TASKS>,
    pub results: BoundedVec<FanoutSubtaskResult, MAX_FANOUT_TASKS>,
    /// The parent's summary budget: present means children must return summaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<FanoutBudget>,
    /// The plan the digest names; required with `final_artifact_receipt`. The
    /// expected children are the plan's tasks other than its output task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<TaskPlanProposal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_artifact_receipt: Option<FinalArtifactReceipt>,
}

/// One entry of the canonical aggregate: a task, its status and its output
/// digest or failure gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FanoutEntry {
    pub task_id: String,
    pub status: FanoutStatus,
    /// The result digest of an ok entry, the failure gap of a failed one.
    pub value: String,
}

/// The aggregate of one fan-out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FanoutAggregate {
    pub op: AggregationOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reducer_id: Option<String>,
    /// Every entry in canonical task order.
    pub entries: Vec<FanoutEntry>,
    /// The output digests the operator selected, in order.
    pub outputs: Vec<String>,
    pub aggregate_digest: String,
    /// The accepted summaries' accounting, when the request carried a budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summaries: Option<SummaryAccount>,
    /// The sealed delivery of the final artifact, when the coordinator delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<FinalDelivery>,
}

/// Token accounting of the accepted summaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SummaryAccount {
    /// `(task id, summary digest, tokens)` per ok child, in task order.
    pub children: Vec<(String, String, u64)>,
    pub total_tokens: u64,
    pub limit_tokens: u64,
    pub remaining_tokens: u64,
}

/// The delivery record of one final artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FinalDelivery {
    pub plan_digest: String,
    pub coordinator: String,
    pub child_receipt_ids: Vec<String>,
    pub schema_id: String,
    pub schema_version: u32,
    pub artifact_digest: String,
    pub readback_id: String,
    pub delivery_digest: String,
}

/// Why a fan-out was not aggregated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FanoutRefusal {
    ChildMissing {
        task_id: String,
    },
    ChildDuplicate {
        task_id: String,
    },
    ChildUnknown {
        task_id: String,
    },
    PlanMismatch {
        detail: String,
    },
    NoValidResult,
    RawReturn {
        task_id: String,
    },
    SummaryOversized {
        task_id: String,
        tokens: u64,
        limit: u64,
    },
    SummaryBudgetExceeded {
        total: Option<u64>,
        limit: u64,
    },
    TokenizerMismatch {
        task_id: String,
    },
    CoordinatorAbsent,
    NonCoordinatorFinalize {
        finalizer: String,
    },
    ArtifactSchemaMismatch,
    ArtifactDigestMismatch,
}

impl DecisionRefusal for FanoutRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::ChildMissing { .. } => FANOUT_CHILD_MISSING,
            Self::ChildDuplicate { .. } => FANOUT_CHILD_DUPLICATE,
            Self::ChildUnknown { .. } => FANOUT_CHILD_UNKNOWN,
            Self::PlanMismatch { .. } => FANOUT_PLAN_MISMATCH,
            Self::NoValidResult => FANOUT_NO_VALID_RESULT,
            Self::RawReturn { .. } => RAW_RETURN_REFUSED,
            Self::SummaryOversized { .. } => SUMMARY_OVERSIZED,
            Self::SummaryBudgetExceeded { .. } => SUMMARY_BUDGET_EXCEEDED,
            Self::TokenizerMismatch { .. } => TOKENIZER_MISMATCH,
            Self::CoordinatorAbsent => COORDINATOR_ABSENT,
            Self::NonCoordinatorFinalize { .. } => NON_COORDINATOR_FINALIZE,
            Self::ArtifactSchemaMismatch => ARTIFACT_SCHEMA_MISMATCH,
            Self::ArtifactDigestMismatch => ARTIFACT_DIGEST_MISMATCH,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::ChildMissing { task_id }
            | Self::ChildDuplicate { task_id }
            | Self::ChildUnknown { task_id } => format!("task={task_id}"),
            Self::PlanMismatch { detail } => detail.clone(),
            Self::NoValidResult => "every child failed".to_string(),
            Self::RawReturn { task_id } => format!("task={task_id}: a child must return a summary"),
            Self::SummaryOversized {
                task_id,
                tokens,
                limit,
            } => format!("task={task_id}: {tokens} tokens over the child limit {limit}"),
            Self::SummaryBudgetExceeded { total, limit } => match total {
                Some(total) => format!("{total} summary tokens over the budget {limit}"),
                None => format!("the summary token sum overflows the budget {limit}"),
            },
            Self::TokenizerMismatch { task_id } => {
                format!("task={task_id}: tokenizer differs from the budget's")
            }
            Self::CoordinatorAbsent => "the plan declares no final artifact".to_string(),
            Self::NonCoordinatorFinalize { finalizer } => {
                format!("finalizer={finalizer} is not the plan's output task")
            }
            Self::ArtifactSchemaMismatch => {
                "the artifact schema differs from the plan's declaration".to_string()
            }
            Self::ArtifactDigestMismatch => {
                "the artifact digest is not the stored readback digest".to_string()
            }
        }
    }
}

fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn op_name(op: AggregationOp) -> &'static str {
    match op {
        AggregationOp::Concat => "concat",
        AggregationOp::MajorityVote => "majority_vote",
        AggregationOp::Reduce => "reduce",
        AggregationOp::FirstValid => "first_valid",
    }
}

/// SHA-256 (64 lowercase hex characters) over the operator, the ordered
/// `(task_id, status, result_digest or gap)` entries and the plan digest.
/// Independent readback recomputes it from the same three inputs.
pub fn aggregate_digest(op: AggregationOp, entries: &[FanoutEntry], plan_digest: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(op_name(op).as_bytes());
    for entry in entries {
        hasher.update([0u8]);
        hasher.update(entry.task_id.as_bytes());
        hasher.update([0u8]);
        hasher.update(entry.status.name().as_bytes());
        hasher.update([0u8]);
        hasher.update(entry.value.as_bytes());
    }
    hasher.update([0u8]);
    hasher.update(plan_digest.as_bytes());
    hex::encode(hasher.finalize())
}

/// Apply the plan's operator to the complete receipt set. The result set must
/// equal `expected_task_ids` exactly once each.
pub fn aggregate_fanout(
    request: &FanoutAggregateRequest,
    knobs: &FanoutKnobs,
) -> Result<FanoutAggregate, FanoutRefusal> {
    let mismatch = |detail: &str| FanoutRefusal::PlanMismatch {
        detail: detail.to_string(),
    };
    if !is_digest(&request.plan_digest) {
        return Err(mismatch("plan_digest is not 64 lowercase hex characters"));
    }
    let reducer_ok = match (request.aggregation.op, &request.aggregation.reducer_id) {
        (AggregationOp::Reduce, Some(id)) => !id.is_empty(),
        (AggregationOp::Reduce, None) => false,
        (_, reducer) => reducer.is_none(),
    };
    if !reducer_ok {
        return Err(mismatch("reducer_id must be present exactly for reduce"));
    }
    let plan = bind_plan(request)?;
    let mut by_task: BTreeMap<&str, &FanoutSubtaskResult> = BTreeMap::new();
    for result in request.results.iter() {
        if !request
            .expected_task_ids
            .iter()
            .any(|id| id == &result.task_id)
        {
            return Err(FanoutRefusal::ChildUnknown {
                task_id: result.task_id.clone(),
            });
        }
        if by_task.insert(result.task_id.as_str(), result).is_some() {
            return Err(FanoutRefusal::ChildDuplicate {
                task_id: result.task_id.clone(),
            });
        }
    }
    let mut expected: Vec<&str> = request
        .expected_task_ids
        .iter()
        .map(String::as_str)
        .collect();
    expected.sort_unstable();
    expected.dedup();
    let mut entries = Vec::with_capacity(expected.len());
    for task_id in expected {
        let missing = || FanoutRefusal::ChildMissing {
            task_id: task_id.to_string(),
        };
        let result = by_task.get(task_id).ok_or_else(missing)?;
        let value = match result.status {
            FanoutStatus::Ok => result
                .result_digest
                .as_deref()
                .filter(|digest| is_digest(digest))
                .ok_or_else(missing)?,
            FanoutStatus::Failed => result
                .failure_gap
                .as_deref()
                .filter(|gap| !gap.trim().is_empty())
                .ok_or_else(missing)?,
        };
        entries.push(FanoutEntry {
            task_id: task_id.to_string(),
            status: result.status,
            value: value.to_string(),
        });
    }
    let ok: Vec<&FanoutEntry> = entries
        .iter()
        .filter(|entry| entry.status == FanoutStatus::Ok)
        .collect();
    if ok.is_empty() {
        return Err(FanoutRefusal::NoValidResult);
    }
    let outputs: Vec<String> = match request.aggregation.op {
        AggregationOp::Concat | AggregationOp::Reduce => {
            ok.iter().map(|entry| entry.value.clone()).collect()
        }
        AggregationOp::FirstValid => vec![ok[0].value.clone()],
        AggregationOp::MajorityVote => {
            // `ok` is in task-id order, so the first digest to reach the best
            // count is carried by the lowest task id among the tied digests.
            let mut counts: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
            for (position, entry) in ok.iter().enumerate() {
                let slot = counts.entry(entry.value.as_str()).or_insert((0, position));
                slot.0 += 1;
            }
            let best = counts
                .iter()
                .max_by(|a, b| a.1 .0.cmp(&b.1 .0).then(b.1 .1.cmp(&a.1 .1)))
                .map(|(digest, _)| (*digest).to_string())
                .unwrap_or_default();
            vec![best]
        }
    };
    let summaries = match &request.budget {
        Some(budget) => Some(check_summaries(&ok, &by_task, budget, knobs)?),
        None => None,
    };
    let delivery = match (&request.final_artifact_receipt, &plan) {
        (Some(receipt), Some(plan)) => Some(check_delivery(request, receipt, plan, &by_task)?),
        _ => None,
    };
    let aggregate_digest = aggregate_digest(request.aggregation.op, &entries, &request.plan_digest);
    Ok(FanoutAggregate {
        op: request.aggregation.op,
        reducer_id: request.aggregation.reducer_id.clone(),
        entries,
        outputs,
        aggregate_digest,
        summaries,
        delivery,
    })
}

/// Derive the plan a request carries and bind it to the request: the digest
/// must be the plan's own, and the expected children must be its tasks other
/// than the output task. A delivery receipt without a plan has nothing to bind to.
fn bind_plan(request: &FanoutAggregateRequest) -> Result<Option<TaskPlan>, FanoutRefusal> {
    let mismatch = |detail: String| FanoutRefusal::PlanMismatch { detail };
    let Some(proposal) = &request.plan else {
        if request.final_artifact_receipt.is_some() {
            return Err(mismatch(
                "final_artifact_receipt requires the plan it delivers".to_string(),
            ));
        }
        return Ok(None);
    };
    let plan = plan_explicit_tasks(proposal).map_err(mismatch)?;
    if plan.plan_digest != request.plan_digest {
        return Err(mismatch(
            "plan_digest is not the carried plan's digest".to_string(),
        ));
    }
    let mut children: Vec<&str> = plan
        .tasks
        .iter()
        .map(|task| task.task_id.as_str())
        .filter(|id| *id != plan.output_task_id)
        .collect();
    children.sort_unstable();
    let mut expected: Vec<&str> = request
        .expected_task_ids
        .iter()
        .map(String::as_str)
        .collect();
    expected.sort_unstable();
    expected.dedup();
    if children != expected {
        return Err(mismatch(
            "expected_task_ids are not the plan's tasks other than its output task".to_string(),
        ));
    }
    Ok(Some(plan))
}

/// Every ok child returns an accepted summary inside the limits, counted with
/// the budget's tokenizer.
fn check_summaries(
    ok: &[&FanoutEntry],
    by_task: &BTreeMap<&str, &FanoutSubtaskResult>,
    budget: &FanoutBudget,
    knobs: &FanoutKnobs,
) -> Result<SummaryAccount, FanoutRefusal> {
    let mut children = Vec::with_capacity(ok.len());
    let mut total: Option<u64> = Some(0);
    for entry in ok {
        let raw = || FanoutRefusal::RawReturn {
            task_id: entry.task_id.clone(),
        };
        let summary = by_task
            .get(entry.task_id.as_str())
            .and_then(|result| result.summary.as_ref())
            .filter(|summary| summary.artifact_kind == "summary")
            .ok_or_else(raw)?;
        if !is_digest(&summary.summary_digest) {
            return Err(FanoutRefusal::ChildMissing {
                task_id: entry.task_id.clone(),
            });
        }
        if summary.tokenizer_revision != budget.tokenizer_revision {
            return Err(FanoutRefusal::TokenizerMismatch {
                task_id: entry.task_id.clone(),
            });
        }
        if summary.summary_tokens > knobs.max_child_summary_tokens {
            return Err(FanoutRefusal::SummaryOversized {
                task_id: entry.task_id.clone(),
                tokens: summary.summary_tokens,
                limit: knobs.max_child_summary_tokens,
            });
        }
        total = total.and_then(|sum| sum.checked_add(summary.summary_tokens));
        children.push((
            entry.task_id.clone(),
            summary.summary_digest.clone(),
            summary.summary_tokens,
        ));
    }
    let limit = budget
        .max_summary_tokens
        .unwrap_or(knobs.max_summary_tokens)
        .min(budget.parent_remaining_tokens);
    match total {
        Some(total) if total <= limit => Ok(SummaryAccount {
            children,
            total_tokens: total,
            limit_tokens: limit,
            remaining_tokens: budget.parent_remaining_tokens - total,
        }),
        other => Err(FanoutRefusal::SummaryBudgetExceeded {
            total: other,
            limit,
        }),
    }
}

/// The coordinator delivery: the finalizer is the plan's output task and the
/// artifact is the one the plan declares.
fn check_delivery(
    request: &FanoutAggregateRequest,
    receipt: &FinalArtifactReceipt,
    plan: &TaskPlan,
    by_task: &BTreeMap<&str, &FanoutSubtaskResult>,
) -> Result<FinalDelivery, FanoutRefusal> {
    let decl = plan
        .final_artifact
        .as_ref()
        .ok_or(FanoutRefusal::CoordinatorAbsent)?;
    if receipt.finalizer_task_id != plan.output_task_id {
        return Err(FanoutRefusal::NonCoordinatorFinalize {
            finalizer: receipt.finalizer_task_id.clone(),
        });
    }
    if receipt.schema_id != decl.schema_id
        || receipt.schema_version != decl.schema_version
        || receipt.kind != decl.kind
    {
        return Err(FanoutRefusal::ArtifactSchemaMismatch);
    }
    if !is_digest(&receipt.artifact_digest) || receipt.readback_id.trim().is_empty() {
        return Err(FanoutRefusal::ArtifactDigestMismatch);
    }
    let child_receipt_ids: Vec<String> = by_task
        .values()
        .map(|result| result.receipt_id.clone())
        .collect();
    let version = receipt.schema_version.to_string();
    let mut parts: Vec<&str> = vec![
        "fanout-delivery/v1",
        &request.plan_digest,
        &plan.output_task_id,
        &receipt.schema_id,
        &version,
        &receipt.artifact_digest,
        &receipt.readback_id,
    ];
    parts.extend(child_receipt_ids.iter().map(String::as_str));
    Ok(FinalDelivery {
        plan_digest: request.plan_digest.clone(),
        coordinator: plan.output_task_id.clone(),
        schema_id: receipt.schema_id.clone(),
        schema_version: receipt.schema_version,
        artifact_digest: receipt.artifact_digest.clone(),
        readback_id: receipt.readback_id.clone(),
        delivery_digest: digest_parts(&parts),
        child_receipt_ids,
    })
}
