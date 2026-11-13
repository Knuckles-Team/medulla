//! The `resolve_entity` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder. The Record stage adds identity
//! evidence (MEDULLA-UNIFIED-DATA-PLANE-R030.701): each pair of declared
//! options contributes one premise `identity:<kind>` per piece of evidence and
//! one `identity:decision:<merge_decision>` premise. Names are the option texts
//! `name` and `key`; an embedding cosine of a pair is the request param
//! `embedding:<left option>:<right option>`. Evidence thresholds are the
//! registry knobs `entity_link.entropy_bits` and `entity_link.embedding_min`.
//! The merge decision is always a proposal. A request that declares an
//! approval (`approve_link`) without a named `approver` refuses with
//! `LINK_APPROVER_MISSING`.
//!
//! The served pair decision (MEDULLA-DECISION-ENGINE-R032.2.2): every option
//! is one entity pair, its two sides the texts `entity.left` and
//! `entity.right`, each an IRI (`ENTITY_PAIR_INVALID` when one is missing or
//! is not an IRI, `ENTITY_PAIR_SELF` when both sides are equal). The decision
//! acts on a pair only when the pinned head's conformal prediction set is
//! exactly that one pair and the head was calibrated at a miscoverage no
//! looser than the decision-policy knob `resolve_entity.alpha`; otherwise it
//! abstains with `AbstainReason::InsufficientConfidence`. The answer proposes
//! a link and never writes `owl:sameAs`.

use medulla_decision_stats::decision::candidate::CandidateView;
use medulla_decision_types::decision::knob_families::resolve_entity as resolve_entity_knobs;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::StatisticalOutcome;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, TypedParam, TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef,
};
use medulla_index_types::entity_resolution::{
    decide_merge, identity_evidence, ApprovalRefusal, IdentityEvidence, MergeDecision,
};
use medulla_types::contract::BoundedVec;

use super::DecisionFamily;
use crate::decide::stat_executor::{Executed, Pinned};

/// Most identity premises one decision records.
const MAX_IDENTITY_PREMISES: usize = 512;
/// Fixed-point scale of an embedding cosine param: Q32, as the declared numbers.
const Q32: f64 = 4_294_967_296.0;

/// Option text naming the left side of a pair.
pub const LEFT_KEY: &str = "entity.left";
/// Option text naming the right side of a pair.
pub const RIGHT_KEY: &str = "entity.right";

/// Whether `value` is an absolute IRI: a scheme, a colon, a non-empty
/// remainder, and no whitespace or angle brackets.
fn is_iri(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c == '<' || c == '>')
}

/// The family of the `resolve_entity` question kind.
pub struct ResolveEntityFamily;

fn text<'a>(option: &'a DeclaredOption, key: &str) -> Option<&'a str> {
    option
        .texts
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| entry.text.as_str())
}

fn param<'a>(request: &'a DecideRequest, name: &str) -> Option<&'a TypedParam> {
    request.params.iter().find(|param| param.name == name)
}

fn cosine(request: &DecideRequest, left: &str, right: &str) -> Option<f64> {
    let named = param(request, &format!("embedding:{left}:{right}"))?;
    match &named.value {
        TypedValue::Int(q32) => Some(*q32 as f64 / Q32),
        _ => None,
    }
}

fn evidence_of(
    request: &DecideRequest,
    left: &DeclaredOption,
    right: &DeclaredOption,
    entropy_bits: f64,
) -> Vec<IdentityEvidence> {
    let mut evidence = identity_evidence(
        text(left, "name").unwrap_or_default(),
        text(right, "name").unwrap_or_default(),
        text(left, "key"),
        text(right, "key"),
        entropy_bits,
    );
    if let Some(cosine) = cosine(request, &left.option_id, &right.option_id) {
        evidence.push(IdentityEvidence::Embedding { cosine });
    }
    evidence
}

fn decision_fact(decision: MergeDecision) -> &'static str {
    match decision {
        MergeDecision::ProposeMerge => "identity:decision:propose_merge",
        MergeDecision::ProposeLink => "identity:decision:propose_link",
        MergeDecision::NoMerge => "identity:decision:no_merge",
    }
}

/// The identity premises of every pair of declared options.
pub fn identity_premises(request: &DecideRequest) -> Vec<PremiseRef> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return Vec::new();
    };
    let knobs = medulla_server_api::knob_config::knobs();
    let mut out = Vec::new();
    let provenance = || PremiseProvenance::Request {
        field: "candidates.options".to_string(),
    };
    for (i, left) in options.iter().enumerate() {
        for right in options.iter().skip(i + 1) {
            let evidence = evidence_of(request, left, right, knobs.entity_link_entropy_bits);
            let decision = decide_merge(&evidence, knobs.entity_link_embedding_min);
            let subject = format!("{}|{}", left.option_id, right.option_id);
            for piece in &evidence {
                out.push(PremiseRef {
                    subject: subject.clone(),
                    fact: piece.premise_fact().to_string(),
                    class: PremiseClass::Claim,
                    provenance: provenance(),
                });
            }
            out.push(PremiseRef {
                subject,
                fact: decision_fact(decision).to_string(),
                class: PremiseClass::Claim,
                provenance: provenance(),
            });
            if out.len() >= MAX_IDENTITY_PREMISES {
                out.truncate(MAX_IDENTITY_PREMISES);
                return out;
            }
        }
    }
    out
}

/// The `(left, right)` IRIs of one pair option.
fn pair_of(option: &DeclaredOption) -> Option<(&str, &str)> {
    Some((text(option, LEFT_KEY)?, text(option, RIGHT_KEY)?))
}

/// Whether the head's calibration was made at a miscoverage no looser than
/// `alpha_micros` (a fraction times 1,000,000).
fn calibrated_within(pinned: &Pinned, alpha_micros: i64) -> bool {
    pinned
        .head
        .as_ref()
        .and_then(|(head, _)| head.calibration.as_ref())
        .is_some_and(|calibration| {
            // numerator / denominator <= alpha_micros / 1e6, in integers.
            u128::from(calibration.alpha.numerator()) * 1_000_000
                <= u128::from(alpha_micros.max(0) as u64)
                    * u128::from(calibration.alpha.denominator())
        })
}

impl DecisionFamily for ResolveEntityFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        for option in options {
            let (left, right) = match pair_of(option) {
                Some((left, right)) if is_iri(left) && is_iri(right) => (left, right),
                _ => {
                    return Err(format!(
                        "{}: option '{}' must declare the texts {LEFT_KEY} and {RIGHT_KEY}, \
                         each an IRI",
                        medulla_decision_types::decision::codes::ENTITY_PAIR_INVALID.as_str(),
                        option.option_id
                    ))
                }
            };
            if left == right {
                return Err(format!(
                    "{}: option '{}' pairs an entity with itself",
                    medulla_decision_types::decision::codes::ENTITY_PAIR_SELF.as_str(),
                    option.option_id
                ));
            }
        }
        Ok(())
    }

    fn refine(
        &self,
        _request: Option<&DecideRequest>,
        pinned: &Pinned,
        _views: &[CandidateView],
        policy: &DecisionPolicy,
        _policy_digest: &str,
        mut executed: Executed,
    ) -> Result<Executed, String> {
        // Without a pinned head nothing was read and the ladder's own answer stands.
        if pinned.head.is_none() {
            return Ok(executed);
        }
        let alpha = resolve_entity_knobs::alpha_micros(policy);
        let abstain = match &executed.ladder.outcome {
            StatisticalOutcome::Acted { prediction_set, .. } => {
                prediction_set.len() != 1 || !calibrated_within(pinned, alpha)
            }
            StatisticalOutcome::Advisory { .. } => true,
            StatisticalOutcome::Explored { .. } | StatisticalOutcome::Abstained { .. } => false,
        };
        if abstain {
            executed.ladder.outcome = StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(vec![AbstainReason::InsufficientConfidence])?,
            };
        }
        Ok(executed)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let approving = matches!(
            param(request, "approve_link").map(|p| &p.value),
            Some(TypedValue::Bool(true))
        );
        let approver = match param(request, "approver").map(|p| &p.value) {
            Some(TypedValue::Text(name)) => name.as_str(),
            _ => "",
        };
        if approving && approver.trim().is_empty() {
            return Err(format!(
                "{}: an entity link approval names its approver",
                ApprovalRefusal::ApproverRequired.code()
            ));
        }
        Ok(())
    }

    fn premises(
        &self,
        request: &DecideRequest,
        _executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        let mut out = identity_premises(request);
        if let CandidateSource::Declared { options } = &request.candidates {
            for option in options.iter() {
                if let Some((left, right)) = pair_of(option) {
                    out.push(PremiseRef {
                        subject: option.option_id.clone(),
                        fact: format!("entity_pair:{left}={right}"),
                        class: PremiseClass::Claim,
                        provenance: PremiseProvenance::Request {
                            field: "candidates.options".to_string(),
                        },
                    });
                }
            }
        }
        out.push(PremiseRef {
            subject: "head".to_string(),
            fact: "calibration".to_string(),
            class: PremiseClass::Observation,
            provenance: PremiseProvenance::Observation {
                evaluation_id: request
                    .head
                    .as_ref()
                    .map(|pin| pin.definition_digest.clone())
                    .unwrap_or_default(),
            },
        });
        out
    }
}
