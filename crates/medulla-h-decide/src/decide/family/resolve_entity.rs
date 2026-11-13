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

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, TypedParam, TypedValue,
};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, PremiseRef};
use medulla_index_types::entity_resolution::{
    decide_merge, identity_evidence, ApprovalRefusal, IdentityEvidence, MergeDecision,
};

use super::DecisionFamily;

/// Most identity premises one decision records.
const MAX_IDENTITY_PREMISES: usize = 512;
/// Fixed-point scale of an embedding cosine param: Q32, as the declared numbers.
const Q32: f64 = 4_294_967_296.0;

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

impl DecisionFamily for ResolveEntityFamily {
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

    fn premises(&self, request: &DecideRequest) -> Vec<PremiseRef> {
        identity_premises(request)
    }
}
