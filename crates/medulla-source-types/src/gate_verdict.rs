//! Gate verdicts are records (MEDULLA-SCM-R008): the request a harness gate runner sends, the
//! sealed record it becomes, and the landing policy that reads the records.
//!
//! Pure data and decisions; `medulla-h-ingest` turns a decision into graph writes.

use medulla_work_types::step_verdict::VerificationVerdict;
use serde::{Deserialize, Serialize};

use crate::lifecycle::{sha256_hex, LifecycleRefusal, ProjectModeApproval};

/// The graph `type` of a sealed gate verdict node.
pub const GATE_VERDICT_TYPE: &str = "GateVerdict";
/// The graph `type` of the landing policy record.
pub const LANDING_POLICY_TYPE: &str = "LandingPolicy";
/// The property holding the canonical JSON of the verdict record.
pub const GATE_VERDICT_RECORD_PROPERTY: &str = "verdict_record";
/// The property holding the verdict seal.
pub const GATE_VERDICT_SEAL_PROPERTY: &str = "verdict_seal";
/// The property holding the canonical JSON of the landing policy.
pub const LANDING_POLICY_RECORD_PROPERTY: &str = "policy_record";
/// The longest gate id.
pub const MAX_GATE_ID_BYTES: usize = 128;

/// One gate verdict as the gate runner reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GateVerdictRecord {
    /// The id of the changeset the gate judged.
    pub changeset: String,
    /// The gate that ran.
    pub gate_id: String,
    /// SHA-256 of the canonical tree listing of the changeset ([`tree_listing_digest`]).
    pub input_digest: String,
    /// What the gate decided.
    pub verdict: VerificationVerdict,
    /// SHA-256 of the gate's evidence (its log or report).
    pub evidence_digest: String,
    /// How long the gate ran, in milliseconds.
    pub duration_ms: u64,
}

/// What `RecordGateVerdict` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GateVerdictReceipt {
    /// The verdict node id, `gate_verdict:<changeset>:<gate_id>`.
    pub node_id: String,
    /// The seal over the verdict record.
    pub seal: String,
    /// True when the same record was already stored.
    pub replayed: bool,
}

/// The branch a repository lands on and the gates that must pass first. A `Fixed` safety
/// record: the merge publish step reads it and the engine never lowers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LandingPolicy {
    /// The repository.
    pub repo: String,
    /// The branch the policy guards.
    pub branch: String,
    /// The gates whose verdict must be a pass before a landing.
    pub required_gates: Vec<String>,
}

/// The node id of a verdict: `gate_verdict:<changeset>:<gate_id>`.
pub fn gate_verdict_node_id(changeset: &str, gate_id: &str) -> String {
    format!("gate_verdict:{changeset}:{gate_id}")
}

/// The node id of a landing policy.
pub fn landing_policy_node_id(repo: &str, branch: &str) -> String {
    format!("landing_policy:{repo}:{branch}")
}

/// The seal of a verdict record: the SHA-256 of its canonical JSON.
pub fn gate_verdict_seal(record: &GateVerdictRecord) -> String {
    sha256_hex(serde_json::to_string(record).unwrap_or_default().as_bytes())
}

/// The digest of the canonical tree listing of a changeset: one line per file in path order,
/// `path`, `blob digest` and the octal `mode`, tab separated.
pub fn tree_listing_digest<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a str, u32)>,
) -> String {
    let mut listing = String::new();
    for (path, blob_digest, mode) in entries {
        listing.push_str(&format!("{path}\t{blob_digest}\t{mode:o}\n"));
    }
    sha256_hex(listing.as_bytes())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl GateVerdictRecord {
    /// Check the field bounds: ids are 64 lowercase hex, the gate id is 1 to 128 characters of
    /// `[A-Za-z0-9._/-]`.
    pub fn validate(&self) -> Result<(), LifecycleRefusal> {
        let bad = |field: &str| {
            Err(LifecycleRefusal::InvalidArgument(format!(
                "{field} is out of bounds"
            )))
        };
        if !is_digest(&self.changeset) {
            return bad("changeset");
        }
        if !is_digest(&self.input_digest) {
            return bad("input_digest");
        }
        if !is_digest(&self.evidence_digest) {
            return bad("evidence_digest");
        }
        let gate_ok = !self.gate_id.is_empty()
            && self.gate_id.len() <= MAX_GATE_ID_BYTES
            && self
                .gate_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'));
        if !gate_ok {
            return bad("gate_id");
        }
        Ok(())
    }
}

/// The landing decision of one policy over the verdicts of the changeset being landed
/// (`gate id`, `verdict`). A required gate with a failed verdict, or with no verdict, blocks
/// with `LANDING_GATE_BLOCKED`, naming every such gate.
pub fn landing_gate_check(
    policy: &LandingPolicy,
    verdicts: &[(String, VerificationVerdict)],
) -> Result<(), LifecycleRefusal> {
    let blocked: Vec<&str> = policy
        .required_gates
        .iter()
        .filter(|gate| {
            !verdicts
                .iter()
                .any(|(id, verdict)| id == *gate && *verdict == VerificationVerdict::Pass)
        })
        .map(String::as_str)
        .collect();
    if blocked.is_empty() {
        Ok(())
    } else {
        Err(LifecycleRefusal::LandingGateBlocked(format!(
            "{}:{} gates not passed: {}",
            policy.repo,
            policy.branch,
            blocked.join(",")
        )))
    }
}

/// `LandingPolicyPut` request (MEDULLA-SCM-R008): set the landing policy of one branch. A write
/// that lowers the stored policy (drops a required gate, or leaves none) needs an operator
/// approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LandingPolicyPutRequest {
    /// The landing policy to store.
    pub policy: LandingPolicy,
    #[serde(default)]
    /// The operator approval that authorizes lowering the stored policy; absent for a write that does not lower it.
    pub approval: Option<ProjectModeApproval>,
}

/// What `LandingPolicyPut` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LandingPolicyReceipt {
    /// The policy node id, `landing_policy:<repo>:<branch>`.
    pub node_id: String,
    /// True when the stored policy was lowered under an approval.
    pub lowered: bool,
    /// True when the same policy was already stored.
    pub replayed: bool,
}

/// What `LandingPolicyPut` writes (not on the wire).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandingPolicyWrite {
    /// The request that was accepted.
    pub request: LandingPolicyPutRequest,
    /// The policy node id, `landing_policy:<repo>:<branch>`.
    pub node_id: String,
    /// True when the stored policy was lowered under an approval.
    pub lowered: bool,
    /// True when the same policy was already stored.
    pub replayed: bool,
}

fn valid_gate_id(gate: &str) -> bool {
    !gate.is_empty()
        && gate.len() <= MAX_GATE_ID_BYTES
        && gate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
}

/// Decide one `LandingPolicyPut` against the stored policy of the same repository and branch.
/// The required gates are kept sorted and unique so the same set always seals the same record.
pub fn handle_landing_policy_put(
    mut request: LandingPolicyPutRequest,
    stored: Option<&LandingPolicy>,
) -> Result<LandingPolicyWrite, LifecycleRefusal> {
    let invalid = |what: &str| Err(LifecycleRefusal::InvalidArgument(what.to_owned()));
    if request.policy.repo.trim().is_empty() || request.policy.branch.trim().is_empty() {
        return invalid("a landing policy names its repo and branch");
    }
    if !request
        .policy
        .required_gates
        .iter()
        .all(|gate| valid_gate_id(gate))
    {
        return invalid("a required gate id is out of bounds");
    }
    request.policy.required_gates.sort();
    request.policy.required_gates.dedup();
    let node_id = landing_policy_node_id(&request.policy.repo, &request.policy.branch);
    let replayed = stored == Some(&request.policy);
    let lowered = stored.is_some_and(|held| {
        held.required_gates
            .iter()
            .any(|gate| !request.policy.required_gates.contains(gate))
    });
    if lowered {
        let approved = request.approval.as_ref().is_some_and(|approval| {
            !approval.approver.trim().is_empty()
                && !approval.reason.trim().is_empty()
                && approval.approved_at_ms > 0
        });
        if !approved {
            return Err(LifecycleRefusal::LandingPolicyApprovalRequired(node_id));
        }
    }
    Ok(LandingPolicyWrite {
        request,
        node_id,
        lowered,
        replayed,
    })
}
