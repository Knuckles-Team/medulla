//! `DecisionLog.promote_rule`: the attach point of rule promotion
//! (MEDULLA-DECISION-ENGINE-R129.1).
//!
//! A rule backed by stored abstention resolutions is checked against the
//! tenant's published `RuleInvariant` components with the pure invariant gate
//! (R129.3) before anything else. A rule that passes is published as an
//! active `GuardrailRule` component; a rule that contradicts an invariant is
//! refused `RULE_CONTRADICTS_INVARIANT` and nothing is activated. No confidence
//! input is read here, so no agreement level can override an invariant.
//!
//! Agreement is the share of the named stored resolutions a human gave
//! (MEDULLA-DECISION-ENGINE-R129.5). Fewer than `rule_promotion.min_overrides`
//! human resolutions refuse `RULE_PROMOTION_TOO_FEW_OVERRIDES`; agreement
//! below `rule_promotion.min_agreement` refuses `RULE_PROMOTION_AGREEMENT_LOW`.
//! A rejection and a promotion each store one [`RulePromotionRecord`].

use medulla_agent_types::agent_component::{
    AgentComponentDraft, AgentComponentKind, AgentComponentPublishRequest, AgentComponentRetireRequest,
    DraftPublication, DraftSubject,
};
use medulla_agent_types::agent_guardrail::{
    check_rule_against_invariants, promote_on_consensus, GuardrailRule, RuleLifecycle,
    RulePromotionVerdict, RuleVersion,
};
use medulla_agent_types::agent_library::{AgentLibraryLifecycle, AgentLibraryMutationContext};
use medulla_decision_types::decision::statistical::body::{
    canonical_body_bytes, content_digest_of,
};
use medulla_decision_types::decision::statistical::log::{
    AbstentionResolver, RulePromotionResult, StoredResolution,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::agent_library::current_agent_library_policy_digest;
use medulla_types::contract::{BoundedVec, Nonce};

use super::stat_executor::ExecutionContext;
use super::stat_log::{store_once, visible_entry, LogReader, MAX_LOG_ROWS};
use super::stat_support::refusal;

/// One audited promotion decision (MEDULLA-DECISION-ENGINE-R129.5): the rule,
/// what the invariant gate said, the resolutions it rested on and their
/// human agreement. `verdict` names the invariant a rejection contradicted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RulePromotionRecord {
    pub rule: GuardrailRule,
    pub verdict: RulePromotionVerdict,
    pub resolution_ids: Vec<String>,
    pub agreement: f64,
}

/// The stored resolutions `resolution_ids` name, each on a record the reader
/// sees; an id that names none is refused.
fn named_resolutions(
    ctx: &ExecutionContext,
    reader: &LogReader,
    resolution_ids: &BoundedVec<String, 64>,
) -> Result<Vec<StoredResolution>, String> {
    let stored: Vec<StoredResolution> = ctx
        .store
        .decision_artifacts_with_prefix(ctx.tenant_id, "resolution:", MAX_LOG_ROWS)?
        .into_iter()
        .map(|(_, bytes)| {
            medulla_persistence_records::decision_jobs::decode_artifact(&bytes, "resolution")
        })
        .collect::<Result<_, String>>()?;
    let mut named = Vec::new();
    for id in resolution_ids.iter() {
        let found = stored
            .iter()
            .find(|row| row.resolution.resolution_id == *id)
            .filter(|row| {
                matches!(
                    visible_entry(ctx.store, reader, &row.resolution.record_id),
                    Ok(Some(_))
                )
            })
            .ok_or_else(|| invalid(format!("no visible stored resolution named {id}")))?;
        named.push(found.clone());
    }
    Ok(named)
}

fn invalid(detail: impl Into<String>) -> String {
    refusal(StatisticalErrorCode::ParameterInvalid, detail.into())
}

fn nonce_of(digest: &str) -> Result<Nonce, String> {
    let raw = hex::decode(digest.trim_start_matches("sha256:")).map_err(|e| e.to_string())?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| "rule digest is not 32 bytes".to_string())?;
    Ok(Nonce::from_bytes(bytes))
}

/// Serve one `PromoteRule` op.
pub(super) fn promote(
    ctx: &ExecutionContext,
    reader: &LogReader,
    rule: GuardrailRule,
    resolution_ids: &BoundedVec<String, 64>,
) -> Result<RulePromotionResult, String> {
    if resolution_ids.is_empty() || resolution_ids.iter().any(|id| id.trim().is_empty()) {
        return Err(invalid(
            "a promoted rule names at least one stored resolution",
        ));
    }
    // A rule that would not decode back from its component is refused up front.
    GuardrailRule::from_attributes(&rule.rule_id, &rule.to_attributes()).map_err(invalid)?;
    let named = named_resolutions(ctx, reader, resolution_ids)?;
    let human = named
        .iter()
        .filter(|row| matches!(row.resolution.resolver, AbstentionResolver::Human))
        .count();
    let agreement = human as f64 / named.len() as f64;
    // Only this tenant's invariants are read.
    let invariants = ctx.store.published_rule_invariants(ctx.tenant_id)?;
    let knobs = medulla_server_api::knob_config::knobs();
    let verdict = promote_on_consensus(
        &rule,
        &invariants,
        agreement,
        knobs.rule_promotion_min_agreement,
    );
    let mut ids: Vec<String> = resolution_ids.iter().cloned().collect();
    ids.sort();
    ids.dedup();
    let record_key = format!(
        "rule_promotion:{}:{}",
        rule.rule_id,
        content_digest_of(&canonical_body_bytes(&ids)?)
    );
    if let Some(RulePromotionVerdict::RejectedInvariant { invariant_iri }) = &verdict {
        let record = RulePromotionRecord {
            rule: rule.clone(),
            verdict: RulePromotionVerdict::RejectedInvariant {
                invariant_iri: invariant_iri.clone(),
            },
            resolution_ids: ids,
            agreement,
        };
        store_once(ctx, record_key, "rule promotion", record, |a, b| a == b)?;
        return Err(refusal(
            StatisticalErrorCode::RuleContradictsInvariant,
            format!("the rule contradicts invariant {invariant_iri}"),
        ));
    }
    if (human as u64) < knobs.rule_promotion_min_overrides {
        return Err(refusal(
            StatisticalErrorCode::RulePromotionTooFewOverrides,
            format!(
                "{human} human resolutions; at least {} are needed",
                knobs.rule_promotion_min_overrides
            ),
        ));
    }
    let Some(verdict) = verdict else {
        return Err(refusal(
            StatisticalErrorCode::RulePromotionAgreementLow,
            format!(
                "human agreement {agreement:.3} is below {}",
                knobs.rule_promotion_min_agreement
            ),
        ));
    };
    let record = RulePromotionRecord {
        rule: rule.clone(),
        verdict: verdict.clone(),
        resolution_ids: ids,
        agreement,
    };
    store_once(ctx, record_key, "rule promotion", record, |a, b| a == b)?;

    let definition_digest = publish_active(ctx, reader, &rule, &format!("promotion:{}", resolution_ids.len()))?;
    Ok(RulePromotionResult {
        rule_id: rule.rule_id,
        verdict,
        published: true,
        definition_digest: Some(definition_digest),
    })
}

/// Publish `rule` as the active `GuardrailRule` component of the tenant and return the
/// published definition digest; the same rule already active is an idempotent replay.
fn publish_active(
    ctx: &ExecutionContext,
    reader: &LogReader,
    rule: &GuardrailRule,
    source_revision: &str,
) -> Result<String, String> {
    let content_digest = content_digest_of(&canonical_body_bytes(rule)?);
    let current = ctx.store.current_component(ctx.tenant_id, &rule.rule_id)?;
    if let Some(entry) = &current {
        if entry.kind == AgentComponentKind::GuardrailRule
            && entry.lifecycle == AgentLibraryLifecycle::Published
            && entry.content_digest == content_digest
        {
            return Ok(entry.definition_digest.clone());
        }
    }
    let policy_digest = current_agent_library_policy_digest()?;
    let mut draft = AgentComponentDraft::bare(
        DraftSubject {
            component_id: &rule.rule_id,
            kind: AgentComponentKind::GuardrailRule,
            version: &rule.source_generation,
            content_digest: &content_digest,
            summary: &format!("promoted guardrail rule {}", rule.rule_id),
        },
        DraftPublication {
            tenant_id: ctx.tenant_id,
            actor_scope: "decision-log:promote-rule",
            purpose_id: "agent-component:publish",
            policy_digest: &policy_digest,
            source_revision,
            source_revision_digest: &content_digest,
        },
    );
    draft.attributes = rule.to_attributes();
    let context = library_context(
        ctx,
        reader,
        ("agent-component:publish", format!("promote-rule:{}:{content_digest}", rule.rule_id)),
        (&content_digest, current.as_ref().map_or(0, |entry| entry.entry_revision)),
        policy_digest,
    )?;
    let committed = ctx.store.publish_component(AgentComponentPublishRequest {
        context,
        evaluation_receipt_digest: None,
        component: draft,
        prompt_blueprint: None,
    }, None)?;
    Ok(committed.result.component.definition_digest)
}

/// The mutation context of one agent-library write made on behalf of the decision log:
/// `(purpose, idempotency key)`, `(nonce digest, expected revision)` and the policy digest.
fn library_context(
    ctx: &ExecutionContext,
    reader: &LogReader,
    (purpose_id, idempotency_key): (&str, String),
    (nonce_digest, expected_revision): (&str, u64),
    policy_digest: String,
) -> Result<AgentLibraryMutationContext, String> {
    Ok(AgentLibraryMutationContext {
        request_id: 0,
        principal: ctx.store.owner_principal().to_string(),
        caller_principal: reader.principal.clone(),
        attempt_nonce: nonce_of(nonce_digest)?,
        tenant_id: ctx.tenant_id.to_string(),
        actor_scope: "decision-log:promote-rule".to_string(),
        purpose_id: purpose_id.to_string(),
        policy_revision: "policy-v1".to_string(),
        policy_digest,
        policy_decision_id: "agent-component:decision:policy-v1".to_string(),
        idempotency_key,
        expected_revision: Some(expected_revision),
        trace_id: None,
        created_at_ms: ctx.now_ms,
    })
}

/// The proposal row of one rule version: the rule it would publish and its version at
/// proposal. The version's state is folded from its transitions.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct RuleProposal {
    rule: GuardrailRule,
    version: RuleVersion,
}

/// One stored transition of a rule version (MEDULLA-DECISION-ENGINE-R129.100): the actor, the
/// version and the source-output digest it acted on, and the state the version took.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuleTransition {
    pub rule_id: String,
    pub version: u64,
    pub state: RuleLifecycle,
    pub actor: String,
    pub source_output_digest: String,
    pub at_ms: u64,
    /// True for the first approval, which leaves the version `Proposed`.
    pub first_approval: bool,
}

fn proposal_key(rule_id: &str, version: u64) -> String {
    format!("rule-version:{rule_id}:{version:010}")
}

fn transition_key(rule_id: &str, sequence: u64) -> String {
    format!("rule-transition:{rule_id}:{sequence:010}")
}

fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Every proposal and transition of `rule_id` the tenant holds, proposals by version and
/// transitions in sequence.
fn load_history(
    ctx: &ExecutionContext,
    rule_id: &str,
) -> Result<(Vec<RuleProposal>, Vec<RuleTransition>), String> {
    let rows = |prefix: String| -> Result<Vec<Vec<u8>>, String> {
        Ok(ctx
            .store
            .decision_artifacts_with_prefix(ctx.tenant_id, &prefix, MAX_LOG_ROWS)?
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect())
    };
    let proposals = rows(format!("rule-version:{rule_id}:"))?
        .iter()
        .map(|bytes| medulla_persistence_records::decision_jobs::decode_artifact(bytes, "rule proposal"))
        .collect::<Result<Vec<RuleProposal>, String>>()?;
    let transitions = rows(format!("rule-transition:{rule_id}:"))?
        .iter()
        .map(|bytes| medulla_persistence_records::decision_jobs::decode_artifact(bytes, "rule transition"))
        .collect::<Result<Vec<RuleTransition>, String>>()?;
    // A rule id may hold the colon of another's prefix; keep only this rule's rows.
    let proposals = proposals.into_iter().filter(|row| row.version.rule_id == rule_id).collect();
    let transitions = transitions.into_iter().filter(|step| step.rule_id == rule_id).collect();
    Ok((proposals, transitions))
}

/// A proposal with its transitions folded in: its state and approvers now.
fn folded(proposal: &RuleProposal, transitions: &[RuleTransition]) -> RuleVersion {
    let mut version = proposal.version.clone();
    for step in transitions.iter().filter(|step| step.version == version.version) {
        if step.first_approval {
            version.approver = Some(step.actor.clone());
        } else {
            if step.state == RuleLifecycle::Active {
                version.approver_second = Some(step.actor.clone());
            }
            version.state = step.state;
        }
    }
    version
}

fn find_version(
    proposals: &[RuleProposal],
    transitions: &[RuleTransition],
    version: u64,
) -> Option<(RuleProposal, RuleVersion)> {
    let proposal = proposals.iter().find(|row| row.version.version == version)?;
    Some((proposal.clone(), folded(proposal, transitions)))
}

fn no_such_version(rule_id: &str, version: u64) -> String {
    invalid(format!("rule {rule_id} has no version {version}"))
}

/// Serve one `ProposeRule` op: store a new `Proposed` version bound to its run output.
pub(super) fn propose(
    ctx: &ExecutionContext,
    rule: GuardrailRule,
    scope: String,
    source_output_digest: String,
    predecessor: Option<u64>,
) -> Result<RuleVersion, String> {
    if scope.trim().is_empty() {
        return Err(invalid("a rule version names its scope"));
    }
    if !is_digest(&source_output_digest) {
        return Err(invalid("source_output_digest is not 64 lowercase hex characters"));
    }
    GuardrailRule::from_attributes(&rule.rule_id, &rule.to_attributes()).map_err(invalid)?;
    let (proposals, transitions) = load_history(ctx, &rule.rule_id)?;
    if let Some(wanted) = predecessor {
        let active = find_version(&proposals, &transitions, wanted)
            .is_some_and(|(_, version)| version.state == RuleLifecycle::Active);
        if !active {
            return Err(refusal(
                StatisticalErrorCode::RulePredecessorStale,
                format!("version {wanted} of {} is not the active version", rule.rule_id),
            ));
        }
    }
    let next = proposals.iter().map(|row| row.version.version).max().unwrap_or(0) + 1;
    let version = RuleVersion {
        rule_id: rule.rule_id.clone(),
        version: next,
        scope,
        source_output_digest,
        predecessor,
        state: RuleLifecycle::Proposed,
        approver: None,
        approver_second: None,
    };
    let row = RuleProposal {
        rule: rule.clone(),
        version: version.clone(),
    };
    ctx.store.put_decision_artifacts(
        ctx.tenant_id,
        &[(
            proposal_key(&rule.rule_id, next),
            medulla_persistence_records::decision_jobs::encode_artifact(&row)?,
        )],
    )?;
    Ok(version)
}

/// Serve one `ApproveRule` op. The first approval records its approver and leaves the version
/// `Proposed`; a second approval from another principal passes the invariant gate and the
/// predecessor check, activates the version and supersedes its predecessor in one write.
pub(super) fn approve(
    ctx: &ExecutionContext,
    reader: &LogReader,
    rule_id: &str,
    version: u64,
    source_output_digest: &str,
) -> Result<RuleVersion, String> {
    let (proposals, transitions) = load_history(ctx, rule_id)?;
    let (proposal, current) = find_version(&proposals, &transitions, version)
        .ok_or_else(|| no_such_version(rule_id, version))?;
    if current.state != RuleLifecycle::Proposed {
        return Err(invalid(format!(
            "version {version} of {rule_id} is not awaiting approval"
        )));
    }
    if current.source_output_digest != source_output_digest {
        return Err(refusal(
            StatisticalErrorCode::RuleOutputMismatch,
            format!("the approval names another output than version {version} was proposed against"),
        ));
    }
    let actor = reader.principal.clone();
    let sequence = transitions.len() as u64 + 1;
    let step = |state: RuleLifecycle, version: u64, first_approval: bool| RuleTransition {
        rule_id: rule_id.to_string(),
        version,
        state,
        actor: actor.clone(),
        source_output_digest: source_output_digest.to_string(),
        at_ms: ctx.now_ms,
        first_approval,
    };
    let Some(first) = &current.approver else {
        let transition = step(RuleLifecycle::Proposed, version, true);
        ctx.store.put_decision_artifacts(
            ctx.tenant_id,
            &[(
                transition_key(rule_id, sequence),
                medulla_persistence_records::decision_jobs::encode_artifact(&transition)?,
            )],
        )?;
        return Ok(folded(&proposal, &[transitions.as_slice(), &[transition]].concat()));
    };
    if *first == actor {
        return Err(refusal(
            StatisticalErrorCode::RuleApproverNotDistinct,
            "the second approval must come from a different principal than the first",
        ));
    }
    // Approval never replaces the invariant gate.
    let invariants = ctx.store.published_rule_invariants(ctx.tenant_id)?;
    if let RulePromotionVerdict::RejectedInvariant { invariant_iri } =
        check_rule_against_invariants(&proposal.rule, &invariants)
    {
        return Err(refusal(
            StatisticalErrorCode::RuleContradictsInvariant,
            format!("the rule contradicts invariant {invariant_iri}"),
        ));
    }
    let mut rows = Vec::new();
    let mut added = Vec::new();
    if let Some(predecessor) = current.predecessor {
        let still_active = find_version(&proposals, &transitions, predecessor)
            .is_some_and(|(_, held)| held.state == RuleLifecycle::Active);
        if !still_active {
            return Err(refusal(
                StatisticalErrorCode::RulePredecessorStale,
                format!("version {predecessor} of {rule_id} is no longer the active version"),
            ));
        }
        added.push(step(RuleLifecycle::Superseded, predecessor, false));
    }
    added.push(step(RuleLifecycle::Active, version, false));
    for (offset, transition) in added.iter().enumerate() {
        rows.push((
            transition_key(rule_id, sequence + offset as u64),
            medulla_persistence_records::decision_jobs::encode_artifact(transition)?,
        ));
    }
    publish_active(ctx, reader, &proposal.rule, &format!("rule-version:{version}"))?;
    ctx.store.put_decision_artifacts(ctx.tenant_id, &rows)?;
    Ok(folded(&proposal, &[transitions.as_slice(), added.as_slice()].concat()))
}

/// Serve one `RevokeRule` op: the named version must be the active one.
pub(super) fn revoke(
    ctx: &ExecutionContext,
    reader: &LogReader,
    rule_id: &str,
    version: u64,
) -> Result<RuleVersion, String> {
    let (proposals, transitions) = load_history(ctx, rule_id)?;
    let (proposal, current) = find_version(&proposals, &transitions, version)
        .ok_or_else(|| no_such_version(rule_id, version))?;
    if current.state != RuleLifecycle::Active {
        return Err(refusal(
            StatisticalErrorCode::RuleNotActive,
            format!("version {version} of {rule_id} is not active"),
        ));
    }
    // The published component is retired first, so a revoke takes effect: a retired revision
    // is left out of every component lookup and of `GuardrailEntailment`. A retry after the
    // transition row failed finds the component already retired and only writes the row.
    if let Some(entry) = ctx.store.current_component(ctx.tenant_id, rule_id)? {
        if entry.kind == AgentComponentKind::GuardrailRule
            && entry.lifecycle == AgentLibraryLifecycle::Published
        {
            let context = library_context(
                ctx,
                reader,
                ("agent-component:retire", format!("revoke-rule:{rule_id}:{version}")),
                (&entry.content_digest, entry.entry_revision),
                current_agent_library_policy_digest()?,
            )?;
            ctx.store.retire_component(AgentComponentRetireRequest {
                context,
                component_id: rule_id.to_string(),
            })?;
        }
    }
    let transition = RuleTransition {
        rule_id: rule_id.to_string(),
        version,
        state: RuleLifecycle::Revoked,
        actor: reader.principal.clone(),
        source_output_digest: current.source_output_digest.clone(),
        at_ms: ctx.now_ms,
        first_approval: false,
    };
    ctx.store.put_decision_artifacts(
        ctx.tenant_id,
        &[(
            transition_key(rule_id, transitions.len() as u64 + 1),
            medulla_persistence_records::decision_jobs::encode_artifact(&transition)?,
        )],
    )?;
    Ok(folded(&proposal, &[transitions.as_slice(), &[transition]].concat()))
}
