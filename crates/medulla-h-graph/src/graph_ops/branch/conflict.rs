//! Epistemic conflict resolution (MEDULLA-BRANCH-MERGE-R005).
//!
//! `handle_branch_merge` (R003) runs [`super::merge::merge_entities`] (R004),
//! which returns `MergePlan.conflicts`; [`merge_conflicts`] turns each into a
//! [`ConflictRecord`]. It adds no formula: the evidence weighting is the
//! `ResolveConflict` family of MEDULLA-DECISION-ENGINE-R828, reached only
//! through the installed `ConflictDecider` seam (a handler crate never calls a
//! sibling handler crate). This module reads the two candidates' provenance
//! from the entity properties, feeds the family one `DecideRequest` per
//! conflict, and copies the family's answer: a winner, or the abstention code.
//! It never compares scores, times or sides itself, so the rule exists once.
//!
//! Writes: none here. The conflict records are written by the merge
//! publication of R007.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_agent_types::agent_component::ComponentDependency;
use medulla_decision_types::decision::resolve_conflict::{
    CONFLICT_OPTION, PARAM_CONFLICT, WINNER_A, WINNER_B,
};
use medulla_decision_types::decision::statistical::declared::{
    ConflictCandidate, ConflictCandidates, DeclaredOption,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, StatisticalOutcome,
    StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_decision_types::decision::{AbstainReason, DecisionPolicyRef};
use medulla_mutation_types::branch::{
    ConflictOutcome, ConflictRecord, ConflictSide, CONFLICT_PROVENANCE_KEYS,
};
use medulla_server_api::api::conflict_decider::decide_conflict;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::{BoundedVec, ServerErrorCode};
use tokio::sync::RwLock;

use super::merge::{EntityIdentity, EntitySnapshot, EntityVersion};

/// The question id every merge conflict is asked under.
pub const CONFLICT_QUESTION_ID: &str = "branch.merge.conflict";

fn property<'a>(version: &'a EntityVersion, key: &str) -> Result<&'a str, String> {
    version
        .properties
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("the version has no `{key}` property"))
}

fn unit_interval(version: &EntityVersion, key: &str) -> Result<f64, String> {
    let text = property(version, key)?;
    let value: f64 = text
        .parse()
        .map_err(|_| format!("`{key}` is not a decimal number: {text}"))?;
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(format!("`{key}` must be finite and in [0, 1]: {text}"))
    }
}

fn millis(version: &EntityVersion, key: &str) -> Result<u64, String> {
    let text = property(version, key)?;
    text.parse()
        .map_err(|_| format!("`{key}` is not a decimal u64 millisecond stamp: {text}"))
}

/// One candidate read from the provenance properties of one entity version.
pub fn candidate_from(version: &EntityVersion) -> Result<ConflictCandidate, String> {
    let keys = &CONFLICT_PROVENANCE_KEYS;
    let valid_until = match version.properties.get(keys.valid_until) {
        Some(_) => Some(millis(version, keys.valid_until)?),
        None => None,
    };
    let evidence_refs = match version.properties.get(keys.evidence_refs) {
        Some(text) => serde_json::from_str::<Vec<String>>(text)
            .map_err(|error| format!("`{}` is not a JSON array of strings: {error}", keys.evidence_refs))?,
        None => Vec::new(),
    };
    Ok(ConflictCandidate {
        source_reliability: unit_interval(version, keys.source_reliability)?,
        confidence: unit_interval(version, keys.confidence)?,
        valid_from: millis(version, keys.valid_from)?,
        valid_until,
        tx_from: millis(version, keys.tx_from)?,
        evidence_refs,
    })
}

/// The digest of the pinned policy; `None` for the engine default.
fn policy_digest(policy: &DecisionPolicyRef) -> Option<String> {
    match policy {
        DecisionPolicyRef::Default => None,
        DecisionPolicyRef::Pinned { component } => Some(component.definition_digest.clone()),
    }
}

fn properties_of(version: Option<&EntityVersion>) -> BTreeMap<String, String> {
    version.map(|v| v.properties.clone()).unwrap_or_default()
}

fn open(code: &str) -> ConflictOutcome {
    ConflictOutcome::Open {
        code: code.to_string(),
    }
}

/// The resolution a decision outcome carries: the winner of an advisory, or
/// the abstention's registered code. Any other outcome is not a conflict
/// answer and refuses the whole merge.
pub fn outcome_of(outcome: &StatisticalOutcome, decision_id: &str) -> Result<ConflictOutcome, String> {
    match outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            let winner = match scores.as_slice().first().map(|s| s.option_id.as_str()) {
                Some(WINNER_A) => ConflictSide::A,
                Some(WINNER_B) => ConflictSide::B,
                other => return Err(format!("the conflict decision named no winner: {other:?}")),
            };
            Ok(ConflictOutcome::Resolved {
                winner,
                decision_id: decision_id.to_string(),
            })
        }
        StatisticalOutcome::Abstained { reasons } => match reasons.as_slice().first() {
            Some(AbstainReason::Family { code, .. }) => Ok(open(code.as_str())),
            other => Err(format!("the conflict decision abstained without a family code: {other:?}")),
        },
        other => Err(format!("a conflict decision answers with a winner or an abstention, not {other:?}")),
    }
}

/// The one `DecideRequest` of a conflict (candidate A is the target version,
/// candidate B the source version).
pub fn conflict_request(
    tenant: &str,
    identity: &EntityIdentity,
    candidate_a: ConflictCandidate,
    candidate_b: ConflictCandidate,
    policy: &DecisionPolicyRef,
    feature_schema: &ComponentDependency,
) -> Result<DecideRequest, String> {
    let option = DeclaredOption {
        option_id: CONFLICT_OPTION.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::default(),
    };
    let body = ConflictCandidates {
        entity: identity.describe(),
        candidate_a,
        candidate_b,
    };
    Ok(DecideRequest {
        tenant_id: tenant.to_string(),
        question: StatisticalQuestion {
            question_id: CONFLICT_QUESTION_ID.to_string(),
            kind: QuestionKind::ResolveConflict,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![option])?,
        },
        feature_schema: feature_schema.clone(),
        head: None,
        policy: policy.clone(),
        params: BoundedVec::new(vec![TypedParam {
            name: PARAM_CONFLICT.to_string(),
            value: TypedValue::ResolveConflict(Box::new(body)),
        }])?,
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    })
}

/// Resolve every conflicting identity from evidence under `policy`.
///
/// Candidate A is the target version and candidate B the source version. A
/// deleted side, or a side whose provenance does not parse, leaves the record
/// open with `CONFLICT_EVIDENCE_MISSING` and runs no decision. A seam failure
/// (`CONFLICT_DECIDER_MISSING`, or any refusal of the decision) refuses the
/// whole merge: the error is returned and nothing publishes. An open conflict
/// keeps the target's version of the entity.
pub async fn merge_conflicts(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    conflicts: &[EntityIdentity],
    source: &EntitySnapshot,
    target: &EntitySnapshot,
    policy: DecisionPolicyRef,
    feature_schema: &ComponentDependency,
) -> Result<Vec<ConflictRecord>, String> {
    let digest = policy_digest(&policy);
    let mut records = Vec::with_capacity(conflicts.len());
    for identity in conflicts {
        let (a, b) = (target.get(identity), source.get(identity));
        let record = |outcome| ConflictRecord {
            entity: identity.describe(),
            candidate_a: properties_of(a),
            candidate_b: properties_of(b),
            outcome,
            policy_digest: digest.clone(),
        };
        let candidates = match (a.map(candidate_from), b.map(candidate_from)) {
            (Some(Ok(a)), Some(Ok(b))) => (a, b),
            _ => {
                records.push(record(open(ServerErrorCode::ConflictEvidenceMissing.as_str())));
                continue;
            }
        };
        let request = conflict_request(
            verified.tenant(),
            identity,
            candidates.0,
            candidates.1,
            &policy,
            feature_schema,
        )?;
        let decided = decide_conflict(state, verified, request).await?;
        let outcome = outcome_of(&decided.outcome, &decided.record_id)?;
        records.push(record(outcome));
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
    use medulla_decision_types::decision::statistical::ScoredOption;
    use medulla_types::refusal_code::RefusalCode;

    fn version(pairs: &[(&str, &str)]) -> EntityVersion {
        EntityVersion {
            properties: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn evidence(reliability: &str, confidence: &str) -> EntityVersion {
        version(&[
            ("source_reliability", reliability),
            ("confidence", confidence),
            ("valid_from", "100"),
            ("tx_from", "110"),
            ("evidence_refs", "[\"doc:1\"]"),
        ])
    }

    fn identity() -> EntityIdentity {
        EntityIdentity::Node {
            tenant: "t".into(),
            node_type: "Person".into(),
            key: "42".into(),
        }
    }

    fn winner_outcome(id: &str) -> StatisticalOutcome {
        let scores = BoundedVec::new(vec![ScoredOption {
            option_id: id.to_string(),
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: 1,
            },
            probability: None,
        }])
        .unwrap();
        StatisticalOutcome::advisory(scores, false)
    }

    // spec: MEDULLA-BRANCH-MERGE-R005
    #[test]
    fn candidates_come_from_provenance_and_missing_evidence_stays_open() {
        let parsed = candidate_from(&evidence("0.5", "0.8")).unwrap();
        assert_eq!(parsed.source_reliability, 0.5);
        assert_eq!(parsed.confidence, 0.8);
        assert_eq!((parsed.valid_from, parsed.tx_from), (100, 110));
        assert_eq!(parsed.valid_until, None);
        assert_eq!(parsed.evidence_refs, vec!["doc:1".to_string()]);
        let closed = version(&[
            ("source_reliability", "1"),
            ("confidence", "1"),
            ("valid_from", "1"),
            ("valid_until", "9"),
            ("tx_from", "2"),
        ]);
        let closed = candidate_from(&closed).unwrap();
        assert_eq!(closed.valid_until, Some(9));
        assert!(closed.evidence_refs.is_empty());
        assert!(candidate_from(&evidence("0.5", "not-a-number")).is_err());
        assert!(candidate_from(&evidence("0.5", "1.5")).is_err());
        assert!(candidate_from(&evidence("NaN", "0.5")).is_err());
        assert!(candidate_from(&version(&[("confidence", "0.5")])).is_err());

        // The outcome copies the decision's winner or abstention code unchanged.
        assert_eq!(
            outcome_of(&winner_outcome(WINNER_A), "rec-1").unwrap(),
            ConflictOutcome::Resolved {
                winner: ConflictSide::A,
                decision_id: "rec-1".into()
            }
        );
        assert_eq!(
            outcome_of(&winner_outcome(WINNER_B), "rec-2").unwrap(),
            ConflictOutcome::Resolved {
                winner: ConflictSide::B,
                decision_id: "rec-2".into()
            }
        );
        let abstained = StatisticalOutcome::Abstained {
            reasons: BoundedVec::new(vec![AbstainReason::Family {
                family: QuestionKind::ResolveConflict,
                code: RefusalCode::new("CONFLICT_UNRESOLVED"),
                detail: String::new(),
            }])
            .unwrap(),
        };
        assert_eq!(
            outcome_of(&abstained, "rec-3").unwrap(),
            ConflictOutcome::Open {
                code: "CONFLICT_UNRESOLVED".into()
            }
        );
        assert!(outcome_of(&winner_outcome("other"), "rec-4").is_err());
    }

    // spec: MEDULLA-BRANCH-MERGE-R005
    #[tokio::test]
    async fn a_deleted_or_unparseable_side_is_open_and_runs_no_decision() {
        use medulla_server_api::authority_context::VerifiedRequestContext;
        let state = Arc::new(RwLock::new(ServerState::new_for_test(
            "conflict-test-secret",
            ServerState::test_isolation("agent"),
        )));
        let verified = VerifiedRequestContext::verified_for_test_in_tenant("agent", "t");
        let schema = ComponentDependency {
            component_id: "schema".into(),
            kind: medulla_agent_types::agent_component::AgentComponentKind::FeatureSchema,
            definition_digest: "d".into(),
        };
        let id = identity();
        let present: EntitySnapshot = [(id.clone(), evidence("0.9", "0.9"))].into();
        let absent = EntitySnapshot::new();
        let broken: EntitySnapshot = [(id.clone(), evidence("0.9", "oops"))].into();
        // No decider is installed: any decide call would be an Err, so an Ok
        // result proves no decision ran for these conflicts.
        for (source, target) in [(&absent, &present), (&present, &absent), (&broken, &present)] {
            let records = merge_conflicts(
                &state,
                &verified,
                std::slice::from_ref(&id),
                source,
                target,
                DecisionPolicyRef::Default,
                &schema,
            )
            .await
            .unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(
                records[0].outcome,
                ConflictOutcome::Open {
                    code: "CONFLICT_EVIDENCE_MISSING".into()
                }
            );
            assert_eq!(records[0].policy_digest, None);
        }
        // Both sides readable: the missing decider refuses the whole merge.
        let refused = merge_conflicts(
            &state,
            &verified,
            std::slice::from_ref(&id),
            &present,
            &present,
            DecisionPolicyRef::Default,
            &schema,
        )
        .await;
        assert!(refused.unwrap_err().contains("conflict decider is not installed"));
    }
}
