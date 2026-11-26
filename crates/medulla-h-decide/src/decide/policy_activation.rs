//! Policy activation (MEDULLA-ORCH-VOCAB-R012.7.3, R012.7.4, R012.8, R806).
//!
//! The active rule set of a repository is a `GovernedSet` revision (R805.10): the champion
//! pointer `champion:governed_set:policy:<repository>` names `policyset:<candidate_set_digest>`.
//! A review (`PolicyReview`, the receipt of the arbora approval row) is recorded once under
//! `policy-review:<receipt digest>` after it is checked against the `PolicyCandidate` nodes
//! the repository index wrote (R012.7.2): the receipt must name the digests of the indexed
//! source, no reviewed clause may be inactive, and the invariant generation must be the one
//! in force. The promotion gate (`subject_gate`) then admits the review as `ReviewApproval`
//! evidence only when its reviewer is not its author. The move of the pointer re-runs the
//! guard of [`guard_activation`] against the stored review: the invariant generation, the
//! revision the review was made against and every reviewed rule against the published
//! invariants (`check_rule_against_invariants`, R129.3). Every refusal happens before the
//! pointer is touched, so a refused activation keeps the prior set. Source prose never gains
//! authority to activate itself: only a committed `promote` record of a recorded review moves
//! the pointer, and the guard names the first contradicted invariant.

use medulla_agent_types::agent_guardrail::{
    check_rule_against_invariants, GuardrailRule, RuleInvariant, RulePromotionVerdict,
};
use medulla_decision_outcome_types::decision::promotion::{
    GraderKind, HeldOutEvidence, ReceiptKind, ReviewApproval, RevisionSubject,
    HELD_OUT_EVIDENCE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, PremiseRef};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact, read_pointer};
use medulla_types::contract::Digest256;
use medulla_types::policy_document::{
    candidate_set_digest_of, policy_scope_ref, PolicyBinding, PolicyReview,
    POLICY_CLAUSE_UNSUPPORTED, POLICY_CONTRADICTS_INVARIANT, POLICY_REVIEW_MISMATCH,
    POLICY_REVIEW_MISSING, POLICY_REVIEW_STALE, POLICY_SCOPE_PREFIX,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::stat_executor::ExecutionContext;
use super::stat_support::refusal;
use super::stat_vectors::GraphVectors;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;

/// The decision artifact a recorded review lives under.
const REVIEW_PREFIX: &str = "policy-review:";

/// The label of the candidate nodes the repository index writes (R012.7.2).
const CANDIDATE_LABEL: &str = "PolicyCandidate";

/// The most candidate nodes one review reads.
const MAX_CANDIDATES: usize = 4_096;

/// The request parameter that asks a `Decide` to record the active policy it loaded
/// (MEDULLA-ORCH-VOCAB-R012.7.4): the repository whose policy applies.
pub(super) const PARAM_POLICY_REPOSITORY: &str = "policy_repository";

/// The sealed premise names of R012.7.4.
pub(super) const PREMISE_REVISION: &str = "policy.revision";
pub(super) const PREMISE_CANDIDATE_SET: &str = "policy.candidate_set_digest";

/// The decision artifact key of the review `receipt_digest` names.
pub(super) fn review_key(receipt_digest: &str) -> String {
    format!("{REVIEW_PREFIX}{receipt_digest}")
}

/// One reviewed clause: the rule it became and where it stands in its document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewedRule {
    pub rule_id: String,
    pub scope: String,
    pub severity: String,
    pub clause_digest: String,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub grammar_revision: u32,
}

impl ReviewedRule {
    /// The guarded rule this clause becomes at activation.
    pub(super) fn guardrail(&self) -> GuardrailRule {
        GuardrailRule {
            rule_id: self.rule_id.clone(),
            applies_to_class: self.scope.clone(),
            source_generation: format!("grammar-{}", self.grammar_revision),
        }
    }
}

/// The stored review: the receipt as arbora returned it and the rules it approved, read from
/// the candidate nodes when the review was recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPolicyReview {
    pub review: PolicyReview,
    pub rules: Vec<ReviewedRule>,
    pub recorded_at_ms: u64,
}

/// One refusal: a registered code and its detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Fault {
    pub(super) code: &'static str,
    pub(super) detail: String,
}

impl Fault {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    /// The `CODE: detail` text every served refusal carries.
    pub(super) fn text(&self) -> String {
        format!("{}: {}", self.code, self.detail)
    }

    /// The registered code of the fault.
    pub(super) fn refusal_code(&self) -> medulla_types::refusal_code::RefusalCode {
        medulla_types::refusal_code::RefusalCode::new(self.code)
    }
}

/// A candidate node, read.
#[derive(Debug, Clone)]
pub(super) struct CandidateNode {
    repo: String,
    path: String,
    status: String,
    reason: String,
    rule_id: String,
    scope: String,
    severity: String,
    clause_digest: String,
    candidate_set_digest: String,
    commit: String,
    blob_digest: String,
    start_line: u32,
    end_line: u32,
    grammar_revision: u32,
}

fn text(properties: &Map<String, Value>, name: &str) -> String {
    match properties.get(name) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn number(properties: &Map<String, Value>, name: &str) -> u32 {
    text(properties, name).parse().unwrap_or(0)
}

impl CandidateNode {
    /// The candidate a node's properties describe; `None` when the node is no candidate.
    pub(super) fn read(properties: &Map<String, Value>) -> Option<Self> {
        let node_type = [text(properties, "type"), text(properties, "node_type")];
        if !node_type.iter().any(|kind| kind == CANDIDATE_LABEL) {
            return None;
        }
        Some(Self {
            repo: text(properties, "repo"),
            path: text(properties, "path"),
            status: text(properties, "status"),
            reason: text(properties, "reason"),
            rule_id: text(properties, "rule_id"),
            scope: text(properties, "scope"),
            severity: text(properties, "severity"),
            clause_digest: text(properties, "clause_digest"),
            candidate_set_digest: text(properties, "candidate_set_digest"),
            commit: text(properties, "commit"),
            blob_digest: text(properties, "blob_digest"),
            start_line: number(properties, "start_line"),
            end_line: number(properties, "end_line"),
            grammar_revision: number(properties, "grammar_revision"),
        })
    }
}

fn sha256_hex(text: &str) -> String {
    Digest256::sha256(text.as_bytes()).to_hex()
}

/// The candidate-set digest of R012.7.1 over the clause digests of `nodes`.
fn set_digest(nodes: &[&CandidateNode]) -> String {
    let mut digests: Vec<&str> = nodes.iter().map(|node| node.clause_digest.as_str()).collect();
    digests.sort_unstable();
    sha256_hex(&digests.join("\n"))
}

/// Judge `binding` against the candidate nodes of the graph: the reviewed rules when the
/// receipt names the indexed source, the fault otherwise. `nodes` are every candidate the
/// caller can see; only the ones of the binding's repository and path count.
pub(super) fn judge(
    binding: &PolicyBinding,
    nodes: &[CandidateNode],
) -> Result<Vec<ReviewedRule>, Fault> {
    let of_source: Vec<&CandidateNode> = nodes
        .iter()
        .filter(|node| node.repo == binding.repository && node.path == binding.path)
        .collect();
    if of_source.is_empty() {
        return Err(Fault::new(
            POLICY_REVIEW_MISMATCH,
            format!(
                "no policy candidate of {} {} is indexed",
                binding.repository, binding.path
            ),
        ));
    }
    let reviewed: Vec<&CandidateNode> = of_source
        .iter()
        .copied()
        .filter(|node| node.candidate_set_digest == binding.candidate_set_digest)
        .collect();
    if reviewed.is_empty() {
        // The source was indexed again after the review: its candidates carry another set.
        return Err(Fault::new(
            POLICY_REVIEW_STALE,
            format!(
                "the indexed source {} {} no longer yields the reviewed candidate set",
                binding.repository, binding.path
            ),
        ));
    }
    if let Some(node) = reviewed
        .iter()
        .find(|node| node.blob_digest != binding.blob_digest || node.commit != binding.commit)
    {
        return Err(Fault::new(
            POLICY_REVIEW_MISMATCH,
            format!(
                "the review names commit {} and blob {}, the indexed candidates name {} and {}",
                binding.commit, binding.blob_digest, node.commit, node.blob_digest
            ),
        ));
    }
    if let Some(node) = reviewed.iter().find(|node| node.status != "proposed") {
        return Err(Fault::new(
            POLICY_CLAUSE_UNSUPPORTED,
            format!(
                "the clause at {}:{}-{} is inactive ({})",
                node.path, node.start_line, node.end_line, node.reason
            ),
        ));
    }
    if set_digest(&reviewed) != binding.candidate_set_digest {
        return Err(Fault::new(
            POLICY_REVIEW_MISMATCH,
            "the indexed candidates do not hash to the reviewed candidate-set digest",
        ));
    }
    let mut rules: Vec<ReviewedRule> = reviewed
        .iter()
        .map(|node| ReviewedRule {
            rule_id: node.rule_id.clone(),
            scope: node.scope.clone(),
            severity: node.severity.clone(),
            clause_digest: node.clause_digest.clone(),
            path: node.path.clone(),
            start_line: node.start_line,
            end_line: node.end_line,
            grammar_revision: node.grammar_revision,
        })
        .collect();
    rules.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    if rules
        .iter()
        .any(|rule| rule.rule_id.is_empty() || rule.scope.is_empty())
    {
        return Err(Fault::new(
            POLICY_REVIEW_MISMATCH,
            "a proposed candidate carries no rule identity or scope",
        ));
    }
    Ok(rules)
}

/// The generation of the invariants in force for `tenant`: `sha256:` and the lowercase hex
/// SHA-256 of the published invariants' lines `<iri>\t<forbidden rule>\t<class>`, sorted and
/// joined by `\n` (the empty set hashes the empty text). It moves whenever an invariant is
/// published, changed or retired, so a review made against another generation is stale, and
/// any client can compute it from the invariants it reads.
pub(super) fn invariant_generation(
    store: &AgentLibraryStore,
    tenant: &str,
) -> Result<String, String> {
    let mut lines: Vec<String> = store
        .published_rule_invariants(tenant)?
        .into_iter()
        .map(|inv| format!("{}\t{}\t{}", inv.invariant_iri, inv.forbidden_rule_id, inv.on_class))
        .collect();
    lines.sort();
    Ok(format!("sha256:{}", sha256_hex(&lines.join("\n"))))
}

pub(super) fn load_review(
    store: &AgentLibraryStore,
    tenant: &str,
    receipt_digest: &str,
) -> Result<Option<StoredPolicyReview>, String> {
    store
        .decision_artifact(tenant, &review_key(receipt_digest))?
        .map(|bytes| decode_artifact(&bytes, "policy review"))
        .transpose()
}

/// The held-out evidence of a stored review: a `ReviewApproval` of its two principals, whose
/// challenger is the set it approved.
fn evidence_of(stored: &StoredPolicyReview) -> HeldOutEvidence {
    let binding = &stored.review.binding;
    HeldOutEvidence {
        schema_version: HELD_OUT_EVIDENCE_SCHEMA_VERSION,
        subject: RevisionSubject::GovernedSet {
            scope_ref: binding.scope_ref(),
        },
        challenger_ref: binding.target(),
        champion_ref: binding.expected_policy_revision.clone(),
        receipt_kind: ReceiptKind::ReviewApproval,
        receipt_digest: binding.review_receipt_digest.clone(),
        evaluation_set_digest: format!("sha256:{}", binding.candidate_set_digest),
        challenger_score_ppm: 0,
        champion_score_ppm: None,
        delta_ppm: 0,
        case_count: stored.rules.len() as u64,
        independent: true,
        grader_kind: GraderKind::Human,
        measurement: None,
        review: Some(ReviewApproval {
            author: stored.review.author.clone(),
            reviewer: stored.review.reviewer.clone(),
        }),
    }
}

fn candidates_of_core(vectors: &GraphVectors) -> Vec<CandidateNode> {
    vectors
        .core()
        .get_nodes_by_label(CANDIDATE_LABEL, MAX_CANDIDATES)
        .into_iter()
        .filter_map(|(id, _)| vectors.visible_properties(&id))
        .filter_map(|properties| properties.as_object().and_then(CandidateNode::read))
        .collect()
}

/// The candidates of the caller's view of a graph, for the staleness check of a decision.
pub(super) fn candidates_of_view(view: &medulla_graph_core::graph::GraphView) -> Vec<CandidateNode> {
    view.node_properties
        .iter()
        .filter(|(id, _)| id.starts_with("policycandidate:"))
        .filter_map(|(_, bytes)| medulla_types::msgpack::decode_property_object(bytes).ok())
        .filter_map(|properties| CandidateNode::read(&properties))
        .collect()
}

fn invalid(detail: impl Into<String>) -> String {
    refusal(StatisticalErrorCode::ParameterInvalid, detail.into())
}

/// Record one review (MEDULLA-ORCH-VOCAB-R012.8): check the receipt against the indexed
/// candidates and the invariant generation in force, store it once under its receipt digest
/// and answer the held-out evidence it carries. The same review replays; another review
/// under the same receipt digest is refused `POLICY_REVIEW_MISMATCH`.
pub(super) fn record_review(
    ctx: &ExecutionContext,
    vectors: &GraphVectors,
    review: PolicyReview,
) -> Result<HeldOutEvidence, String> {
    review.check().map_err(invalid)?;
    let binding = &review.binding;
    let rules = judge(binding, &candidates_of_core(vectors)).map_err(|fault| fault.text())?;
    if invariant_generation(ctx.store, ctx.tenant_id)? != binding.invariant_generation {
        return Err(Fault::new(
            POLICY_REVIEW_STALE,
            "the invariant generation changed after the review",
        )
        .text());
    }
    if let Some(existing) = load_review(ctx.store, ctx.tenant_id, &binding.review_receipt_digest)? {
        if existing.review != review {
            return Err(Fault::new(
                POLICY_REVIEW_MISMATCH,
                "the receipt digest is recorded with another review",
            )
            .text());
        }
        return Ok(evidence_of(&existing));
    }
    let stored = StoredPolicyReview {
        review,
        rules,
        recorded_at_ms: ctx.now_ms,
    };
    ctx.store.put_decision_artifacts(
        ctx.tenant_id,
        &[(
            review_key(&stored.review.binding.review_receipt_digest),
            encode_artifact(&stored)?,
        )],
    )?;
    Ok(evidence_of(&stored))
}

/// The evidence of a `GovernedSet` policy scope (MEDULLA-ORCH-VOCAB-R012.7.3): the recorded
/// review `receipt_digest` names. `Ok(None)` when the scope is no policy scope or no review
/// of that scope holds the receipt. With `graph`, the review is judged again against the
/// candidates the caller sees now, so a source indexed again since the review is
/// `POLICY_REVIEW_STALE`.
pub(super) fn review_evidence(
    store: &AgentLibraryStore,
    tenant: &str,
    graph: Option<&medulla_graph_core::graph::GraphView>,
    scope_ref: &str,
    receipt_digest: &str,
) -> Result<Result<Option<HeldOutEvidence>, Fault>, String> {
    if !scope_ref.starts_with(POLICY_SCOPE_PREFIX) {
        return Ok(Ok(None));
    }
    let Some(stored) = load_review(store, tenant, receipt_digest)? else {
        return Ok(Ok(None));
    };
    if stored.review.binding.scope_ref() != scope_ref {
        return Ok(Ok(None));
    }
    if let Some(view) = graph {
        let nodes = candidates_of_view(view);
        if let Err(fault) = judge(&stored.review.binding, &nodes) {
            return Ok(Err(fault));
        }
    }
    Ok(Ok(Some(evidence_of(&stored))))
}

/// The refusal of a review that no longer holds against the store: the checks that need no
/// graph. `target` is the revision the move would activate (`None` for a retirement).
pub(super) fn check_activation(
    store: &AgentLibraryStore,
    tenant: &str,
    stored: &StoredPolicyReview,
    target: Option<&str>,
) -> Result<(), Fault> {
    let binding = &stored.review.binding;
    let store_fault = |error: String| Fault::new(POLICY_REVIEW_MISSING, error);
    if let Some(target) = target {
        if candidate_set_digest_of(target) != Some(binding.candidate_set_digest.as_str()) {
            return Err(Fault::new(
                POLICY_REVIEW_MISMATCH,
                format!("the review approved {}, not {target}", binding.target()),
            ));
        }
    }
    let generation = invariant_generation(store, tenant).map_err(store_fault)?;
    if generation != binding.invariant_generation {
        return Err(Fault::new(
            POLICY_REVIEW_STALE,
            "the invariant generation changed after the review",
        ));
    }
    let pointer = RevisionSubject::GovernedSet {
        scope_ref: binding.scope_ref(),
    }
    .pointer_key();
    let (_, state) = read_pointer(store, tenant, &pointer).map_err(store_fault)?;
    let active = state.target();
    // The set already active replays; any other move must start from the revision reviewed.
    if target.is_none() || active != target {
        if active != binding.expected_policy_revision.as_deref() {
            return Err(Fault::new(
                medulla_decision_types::decision::codes::PROMOTION_CAS_CONFLICT.as_str(),
                format!(
                    "the review was made against {:?}, the active revision is {:?}",
                    binding.expected_policy_revision, active
                ),
            ));
        }
    }
    let invariants: Vec<RuleInvariant> = store
        .published_rule_invariants(tenant)
        .map_err(store_fault)?;
    for rule in &stored.rules {
        if let RulePromotionVerdict::RejectedInvariant { invariant_iri } =
            check_rule_against_invariants(&rule.guardrail(), &invariants)
        {
            return Err(Fault::new(
                POLICY_CONTRADICTS_INVARIANT,
                format!(
                    "invariant {invariant_iri} forbids rule {} (clause {}:{}-{})",
                    rule.rule_id, rule.path, rule.start_line, rule.end_line
                ),
            ));
        }
    }
    Ok(())
}

/// The guard of the champion move of a policy scope (MEDULLA-ORCH-VOCAB-R012.7.3): `record_id`
/// is the committed `promote` record that qualified the move. Refused before the pointer is
/// read for writing, so a refusal keeps the prior set.
pub(super) fn guard_activation(
    store: &AgentLibraryStore,
    tenant: &str,
    scope_ref: &str,
    target: &str,
    record_id: &str,
) -> Result<(), String> {
    if !scope_ref.starts_with(POLICY_SCOPE_PREFIX) {
        return Ok(());
    }
    let missing = |detail: &str| Fault::new(POLICY_REVIEW_MISSING, detail).text();
    let receipt = super::stat_promotion::promoted_receipt(store, tenant, record_id)?
        .ok_or_else(|| missing("the promotion record names no review receipt"))?;
    let stored = load_review(store, tenant, &receipt)?
        .ok_or_else(|| missing("no recorded review holds the receipt"))?;
    if stored.review.binding.scope_ref() != scope_ref {
        return Err(Fault::new(
            POLICY_REVIEW_MISMATCH,
            format!("the review is for {}, not {scope_ref}", stored.review.binding.scope_ref()),
        )
        .text());
    }
    check_activation(store, tenant, &stored, Some(target)).map_err(|fault| fault.text())
}

/// The policy in force for one repository: the revision the pointer names and the review
/// that approved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ActivePolicy {
    pub(super) revision: String,
    pub(super) review: StoredPolicyReview,
}

/// Load the active policy of `repository`; `Ok(None)` when none is active.
pub(super) fn active_policy(
    store: &AgentLibraryStore,
    tenant: &str,
    repository: &str,
) -> Result<Option<ActivePolicy>, String> {
    let subject = RevisionSubject::GovernedSet {
        scope_ref: policy_scope_ref(repository),
    };
    let (_, state) = read_pointer(store, tenant, &subject.pointer_key())?;
    let (Some(active), Some(revision)) = (state.active.as_ref(), state.target()) else {
        return Ok(None);
    };
    let record = active.receipt_digest.clone().unwrap_or_default();
    let receipt = super::stat_promotion::promoted_receipt(store, tenant, &record)?
        .ok_or_else(|| {
            Fault::new(POLICY_REVIEW_MISSING, "the active policy names no review receipt").text()
        })?;
    let review = load_review(store, tenant, &receipt)?.ok_or_else(|| {
        Fault::new(POLICY_REVIEW_MISSING, "no recorded review holds the active receipt").text()
    })?;
    Ok(Some(ActivePolicy {
        revision: revision.to_string(),
        review,
    }))
}

/// The premises a `Decide` that loads the active repository policy adds to its record
/// (MEDULLA-ORCH-VOCAB-R012.7.4): the revision and the candidate-set digest, or `none` for
/// both when no policy is active. A request that names no repository adds nothing.
pub(super) fn policy_premises(
    ctx: &ExecutionContext,
    request: &medulla_decision_types::decision::statistical::DecideRequest,
) -> Result<Vec<PremiseRef>, String> {
    let Some(repository) = super::family::support::text_param(request, PARAM_POLICY_REPOSITORY)?
    else {
        return Ok(Vec::new());
    };
    let (revision, digest, source) = match active_policy(ctx.store, ctx.tenant_id, repository)? {
        Some(active) => (
            active.revision,
            active.review.review.binding.candidate_set_digest.clone(),
            active.review.review.binding.review_receipt_digest.clone(),
        ),
        None => ("none".to_string(), "none".to_string(), String::new()),
    };
    let premise = |name: &str, value: &str| PremiseRef {
        subject: format!("policy:{repository}"),
        fact: format!("{name}={value}"),
        class: PremiseClass::Observation,
        provenance: PremiseProvenance::Observation {
            evaluation_id: source.clone(),
        },
        trust_tier: None,
    };
    Ok(vec![
        premise(PREMISE_REVISION, &revision),
        premise(PREMISE_CANDIDATE_SET, &digest),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(rule: &str, status: &str, set: &str) -> CandidateNode {
        CandidateNode {
            repo: "org/repo".into(),
            path: "constitution.md".into(),
            status: status.into(),
            reason: if status == "proposed" { String::new() } else { "CLAUSE_UNPARSED".into() },
            rule_id: rule.into(),
            scope: "https://example.invalid/class/Repo".into(),
            severity: "block".into(),
            clause_digest: sha256_hex(rule),
            candidate_set_digest: set.into(),
            commit: "c0ffee".into(),
            blob_digest: "ab".repeat(32),
            start_line: 3,
            end_line: 4,
            grammar_revision: 1,
        }
    }

    fn binding(set: &str) -> PolicyBinding {
        PolicyBinding {
            repository: "org/repo".into(),
            path: "constitution.md".into(),
            commit: "c0ffee".into(),
            blob_digest: "ab".repeat(32),
            candidate_set_digest: set.into(),
            review_receipt_digest: format!("sha256:{}", "ef".repeat(32)),
            invariant_generation: format!("sha256:{}", "01".repeat(32)),
            expected_policy_revision: None,
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R012.8
    #[test]
    fn judge_names_the_fault_of_each_binding_that_does_not_hold() {
        let nodes = vec![node("a", "proposed", ""), node("b", "proposed", "")];
        let set = set_digest(&nodes.iter().collect::<Vec<_>>());
        let nodes: Vec<CandidateNode> = nodes
            .into_iter()
            .map(|mut n| {
                n.candidate_set_digest = set.clone();
                n
            })
            .collect();
        let rules = judge(&binding(&set), &nodes).unwrap();
        assert_eq!(rules.iter().map(|r| r.rule_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        // Another candidate-set digest: the source was indexed again.
        let stale = judge(&binding(&"cd".repeat(32)), &nodes).unwrap_err();
        assert_eq!(stale.code, POLICY_REVIEW_STALE);
        // No candidate of the source at all.
        let mismatch = judge(&binding(&set), &[]).unwrap_err();
        assert_eq!(mismatch.code, POLICY_REVIEW_MISMATCH);
        // Another blob digest than the indexed one.
        let mut other = binding(&set);
        other.blob_digest = "00".repeat(32);
        assert_eq!(judge(&other, &nodes).unwrap_err().code, POLICY_REVIEW_MISMATCH);
        // An inactive clause cannot be activated.
        let mut with_inactive = nodes.clone();
        with_inactive.push({
            let mut inactive = node("c", "inactive", &set);
            inactive.candidate_set_digest = set.clone();
            inactive
        });
        assert_eq!(
            judge(&binding(&set), &with_inactive).unwrap_err().code,
            POLICY_CLAUSE_UNSUPPORTED
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R012.7.3
    #[test]
    fn guardrail_carries_the_scope_and_grammar_generation_of_its_clause() {
        let rule = ReviewedRule {
            rule_id: "policy-0123".into(),
            scope: "https://example.invalid/class/Repo".into(),
            severity: "block".into(),
            clause_digest: "ab".repeat(32),
            path: "constitution.md".into(),
            start_line: 3,
            end_line: 3,
            grammar_revision: 1,
        };
        let guard = rule.guardrail();
        assert_eq!(guard.applies_to_class, rule.scope);
        assert_eq!(guard.source_generation, "grammar-1");
        let invariant = RuleInvariant {
            invariant_iri: "inv:no-secrets".into(),
            forbidden_rule_id: "policy-0123".into(),
            on_class: rule.scope.clone(),
        };
        assert_eq!(
            check_rule_against_invariants(&guard, &[invariant]),
            RulePromotionVerdict::RejectedInvariant {
                invariant_iri: "inv:no-secrets".into()
            }
        );
    }
}
