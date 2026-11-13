//! Branch outcome records and the fusion receipt (MEDULLA-DECISION-ENGINE-R134.14).
//!
//! A branch of a committed plan records its outcome digest or the gap that
//! stopped it. The caller pins the fusion policy triple; Medulla orders the
//! branches by `branch_id` ascending, assigns each its `fusion_position` and
//! seals one receipt over the ordered `(branch_id, outcome_digest or gap code)`
//! pairs. Nothing here reads a clock or a store.

use serde::{Deserialize, Serialize};

use medulla_query_types::solve_scalar::digest_text;
use medulla_types::refusal_code::RefusalCode;

/// A planned branch has no recorded outcome, or its record has neither an
/// outcome digest nor a failure gap.
pub const BRANCH_MISSING: &str = "BRANCH_MISSING";
/// A branch is planned twice, or a record exists under its key with other content.
pub const BRANCH_DUPLICATE: &str = "BRANCH_DUPLICATE";
/// A branch's recorded input digest is not the input digest the plan pinned.
pub const BRANCH_INPUT_CHANGED: &str = "BRANCH_INPUT_CHANGED";
/// The caller did not pin a complete `(fusion_policy_id, revision, digest)`.
pub const FUSION_POLICY_ABSENT: &str = "FUSION_POLICY_ABSENT";
/// A retrieval receipt cites a record ref that no longer reads back as cited.
pub const BRANCH_READBACK_STALE: &str = "BRANCH_READBACK_STALE";

/// Every refusal code of the branch-outcome family; joins `codes::FAMILY_TABLES`
/// under `QuestionKind::BranchOutcome`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(BRANCH_MISSING),
    RefusalCode::new(BRANCH_DUPLICATE),
    RefusalCode::new(BRANCH_INPUT_CHANGED),
    RefusalCode::new(FUSION_POLICY_ABSENT),
    RefusalCode::new(BRANCH_READBACK_STALE),
];

/// Most branches one plan fuses.
pub const MAX_BRANCHES: usize = 64;

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

fn is_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// How one branch ended: an outcome digest or the gap that stopped it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BranchResult {
    Outcome { outcome_digest: String },
    Gap { failure_gap: String },
}

impl BranchResult {
    /// The outcome digest or the gap code, as the fused digest and the premise read it.
    pub fn text(&self) -> String {
        match self {
            Self::Outcome { outcome_digest } => outcome_digest.clone(),
            Self::Gap { failure_gap } => format!("gap:{failure_gap}"),
        }
    }
}

/// One branch's recorded outcome. `fusion_position` is empty as written; the
/// fusion assigns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BranchRecord {
    pub branch_id: String,
    pub plan_digest: String,
    pub input_digest: String,
    pub experience_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fusion_position: Option<u32>,
    pub result: BranchResult,
}

impl BranchRecord {
    /// The record's key in the tenant's decision artifact store.
    pub fn key(&self) -> String {
        branch_record_key(&self.plan_digest, &self.branch_id)
    }

    /// The content digest a ref to this record cites.
    pub fn digest(&self) -> String {
        digest_text("eg/branch-record/v1", self)
    }

    /// A record is admitted only with its ids, its digests and one result.
    pub fn validate(&self) -> Result<(), String> {
        if self.branch_id.trim().is_empty() || self.plan_digest.trim().is_empty() {
            return Err(refuse(
                BRANCH_MISSING,
                "a branch record names its branch and plan",
            ));
        }
        if !is_digest(&self.input_digest) {
            return Err(refuse(
                BRANCH_INPUT_CHANGED,
                "the input digest is not a 64-character lowercase SHA-256",
            ));
        }
        if self.fusion_position.is_some() {
            return Err(refuse(
                BRANCH_DUPLICATE,
                "the fusion position is assigned by the fusion, never written",
            ));
        }
        match &self.result {
            BranchResult::Outcome { outcome_digest } if is_digest(outcome_digest) => Ok(()),
            BranchResult::Outcome { .. } => Err(refuse(
                BRANCH_MISSING,
                "the outcome digest is not a 64-character lowercase SHA-256",
            )),
            BranchResult::Gap { failure_gap } if !failure_gap.trim().is_empty() => Ok(()),
            BranchResult::Gap { .. } => Err(refuse(
                BRANCH_MISSING,
                "a failed branch records its failure gap",
            )),
        }
    }
}

/// The artifact key of the record of `branch_id` under `plan_digest`.
pub fn branch_record_key(plan_digest: &str, branch_id: &str) -> String {
    format!("run_record/branch/{plan_digest}/{branch_id}")
}

/// The artifact key of the retrieval receipt `consumer_branch_id` stored.
pub fn branch_retrieval_key(plan_digest: &str, consumer_branch_id: &str) -> String {
    format!("run_record/branch_retrieval/{plan_digest}/{consumer_branch_id}")
}

/// The fusion policy the caller pins; Medulla never computes one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FusionPolicyRef {
    pub fusion_policy_id: String,
    pub revision: u64,
    pub digest: String,
}

impl FusionPolicyRef {
    pub fn check(&self) -> Result<(), String> {
        if self.fusion_policy_id.trim().is_empty() || !is_digest(&self.digest) {
            return Err(refuse(
                FUSION_POLICY_ABSENT,
                "pin the fusion policy id, revision and a 64-character digest",
            ));
        }
        Ok(())
    }
}

/// One branch the plan committed to, with the input digest it was planned on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PlannedBranch {
    pub branch_id: String,
    pub input_digest: String,
}

/// The exact `(record_id, revision, digest)` of one stored record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BranchRef {
    pub record_id: String,
    pub revision: u64,
    pub digest: String,
}

impl BranchRef {
    pub fn of(record: &BranchRecord) -> Self {
        Self {
            record_id: record.key(),
            revision: record.experience_revision,
            digest: record.digest(),
        }
    }
}

/// One fused branch: its position, its result and the ref of its record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FusedBranch {
    pub branch_id: String,
    pub fusion_position: u32,
    pub result: BranchResult,
    pub record: BranchRef,
}

/// The receipt of one fusion: every planned branch, the policy triple, every
/// outcome ref and the digest over the ordered pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FusionReceipt {
    pub plan_digest: String,
    pub planned: Vec<String>,
    pub policy: FusionPolicyRef,
    pub branches: Vec<FusedBranch>,
    pub fused_digest: String,
}

/// What a later branch consumed: the exact refs it read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BranchRetrievalReceipt {
    pub plan_digest: String,
    pub consumer_branch_id: String,
    pub refs: Vec<BranchRef>,
}

/// Fuse the recorded outcomes of `planned` under the pinned `policy`. Branches
/// are ordered by `branch_id` ascending and take their position from that order.
/// `records` holds every record the caller's tenant has for the plan.
pub fn fuse_branches(
    plan_digest: &str,
    planned: &[PlannedBranch],
    records: &[BranchRecord],
    policy: &FusionPolicyRef,
) -> Result<(Vec<BranchRecord>, FusionReceipt), String> {
    policy.check()?;
    if planned.is_empty() || planned.len() > MAX_BRANCHES {
        return Err(refuse(
            BRANCH_MISSING,
            format!("a fusion covers 1 to {MAX_BRANCHES} planned branches"),
        ));
    }
    let mut order: Vec<&PlannedBranch> = planned.iter().collect();
    order.sort_by(|a, b| a.branch_id.cmp(&b.branch_id));
    if let Some(pair) = order.windows(2).find(|w| w[0].branch_id == w[1].branch_id) {
        return Err(refuse(
            BRANCH_DUPLICATE,
            format!("branch `{}` is planned twice", pair[0].branch_id),
        ));
    }
    let mut fused = Vec::with_capacity(order.len());
    let mut branches = Vec::with_capacity(order.len());
    for (position, plan) in order.iter().enumerate() {
        let record = records
            .iter()
            .find(|r| r.plan_digest == plan_digest && r.branch_id == plan.branch_id)
            .ok_or_else(|| {
                refuse(
                    BRANCH_MISSING,
                    format!("branch `{}` has no recorded outcome", plan.branch_id),
                )
            })?;
        if record.input_digest != plan.input_digest {
            return Err(refuse(
                BRANCH_INPUT_CHANGED,
                format!(
                    "branch `{}` ran on another input than planned",
                    plan.branch_id
                ),
            ));
        }
        let mut positioned = record.clone();
        positioned.fusion_position = Some(position as u32);
        branches.push(FusedBranch {
            branch_id: record.branch_id.clone(),
            fusion_position: position as u32,
            result: record.result.clone(),
            record: BranchRef::of(record),
        });
        fused.push(positioned);
    }
    let pairs: Vec<(&str, String)> = branches
        .iter()
        .map(|b| (b.branch_id.as_str(), b.result.text()))
        .collect();
    let fused_digest = digest_text("eg/branch-fusion/v1", &(plan_digest, policy, pairs));
    let receipt = FusionReceipt {
        plan_digest: plan_digest.to_string(),
        planned: order.iter().map(|p| p.branch_id.clone()).collect(),
        policy: policy.clone(),
        branches,
        fused_digest,
    };
    Ok((fused, receipt))
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn record(branch: &str, result: BranchResult) -> BranchRecord {
        BranchRecord {
            branch_id: branch.into(),
            plan_digest: "plan-1".into(),
            input_digest: D.into(),
            experience_revision: 3,
            fusion_position: None,
            result,
        }
    }

    fn policy() -> FusionPolicyRef {
        FusionPolicyRef {
            fusion_policy_id: "fuse".into(),
            revision: 1,
            digest: D.into(),
        }
    }

    fn planned(ids: &[&str]) -> Vec<PlannedBranch> {
        ids.iter()
            .map(|id| PlannedBranch {
                branch_id: (*id).into(),
                input_digest: D.into(),
            })
            .collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.14
    #[test]
    fn fusion_orders_by_branch_id_and_digests_the_pairs() {
        let out = BranchResult::Outcome {
            outcome_digest: D.into(),
        };
        let gap = BranchResult::Gap {
            failure_gap: "timeout".into(),
        };
        let records = vec![record("b", gap.clone()), record("a", out.clone())];
        let (fused, receipt) =
            fuse_branches("plan-1", &planned(&["b", "a"]), &records, &policy()).unwrap();
        assert_eq!(receipt.planned, ["a", "b"]);
        assert_eq!(receipt.branches[0].fusion_position, 0);
        assert_eq!(receipt.branches[1].result, gap);
        assert_eq!(fused[1].fusion_position, Some(1));
        let (_, again) =
            fuse_branches("plan-1", &planned(&["a", "b"]), &records, &policy()).unwrap();
        assert_eq!(again.fused_digest, receipt.fused_digest);
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.14
    #[test]
    fn fusion_refusals_name_their_codes() {
        let out = BranchResult::Outcome {
            outcome_digest: D.into(),
        };
        let records = vec![record("a", out)];
        let code = |r: Result<_, String>| {
            r.map(|_: (Vec<BranchRecord>, FusionReceipt)| ())
                .unwrap_err()
        };
        assert!(code(fuse_branches(
            "plan-1",
            &planned(&["a", "b"]),
            &records,
            &policy()
        ))
        .starts_with(BRANCH_MISSING));
        assert!(code(fuse_branches(
            "plan-1",
            &planned(&["a", "a"]),
            &records,
            &policy()
        ))
        .starts_with(BRANCH_DUPLICATE));
        let mut moved = planned(&["a"]);
        moved[0].input_digest = "f".repeat(64);
        assert!(code(fuse_branches("plan-1", &moved, &records, &policy()))
            .starts_with(BRANCH_INPUT_CHANGED));
        let mut bare = policy();
        bare.digest.clear();
        assert!(
            code(fuse_branches("plan-1", &planned(&["a"]), &records, &bare))
                .starts_with(FUSION_POLICY_ABSENT)
        );
        assert!(record(
            "a",
            BranchResult::Gap {
                failure_gap: " ".into()
            }
        )
        .validate()
        .unwrap_err()
        .starts_with(BRANCH_MISSING));
    }
}
