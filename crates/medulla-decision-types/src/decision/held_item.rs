//! Held-item admission (MEDULLA-DECISION-ENGINE-R038.3).
//!
//! A run holds two kinds of stored item until something admits them: a
//! compiled run program and a run output proposal. Each is a node of the graph
//! the question names. The node records the item's content digest and, by
//! kind, the facts admission reads: who compiled and who verified a program,
//! how many verification passes it earned and whether it was retired; which
//! critique verdict a proposal carries and the digest that verdict is bound
//! to. Admission is advisory and writes nothing.

use serde::{Deserialize, Serialize};

use super::digest::HELD_ITEM_CRITIQUE_DIGEST_DOMAIN;
use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::record::outcome::DecisionRefusal;
use medulla_query_types::solve_scalar::digest_text;
use medulla_types::refusal_code::RefusalCode;

/// Text parameter: the kind of the held item.
pub const PARAM_ITEM_KIND: &str = "item_kind";
/// Text parameter: the node id of the held item.
pub const PARAM_ITEM_REF: &str = "item_ref";
/// Text parameter: the digest the caller expects the stored item to have.
pub const PARAM_CONTENT_DIGEST: &str = "content_digest";
/// Text parameter: the graph holding the item.
pub const PARAM_GRAPH: &str = "graph";

/// Node row key: the kind of the stored item.
pub const ROW_ITEM_KIND: &str = "held_item_kind";
/// Node row key: the stored item's content digest.
pub const ROW_CONTENT_DIGEST: &str = "content_digest";
/// Node row key: `active` or `retired`.
pub const ROW_LIFECYCLE: &str = "lifecycle";
/// Node row key: who compiled the program.
pub const ROW_COMPILED_BY: &str = "compiled_by";
/// Node row key: who verified the program.
pub const ROW_VERIFIED_BY: &str = "verified_by";
/// Node row key: the verification record of the program.
pub const ROW_VERIFICATION_REF: &str = "verification_ref";
/// Node row key: the passes the verification earned.
pub const ROW_VERIFICATION_PASSES: &str = "verification_passes";
/// Node row key: the critique record of a proposal.
pub const ROW_CRITIQUE_REF: &str = "critique_ref";
/// Node row key: the digest the critique verdict is bound to.
pub const ROW_CRITIQUE_DIGEST: &str = "critique_digest";
/// Node row key: `accept` or `reject`.
pub const ROW_CRITIQUE_VERDICT: &str = "critique_verdict";

/// Node row key: when the engine recorded the critique, in milliseconds of its clock.
pub const ROW_CRITIQUE_RECORDED_AT: &str = "critique_recorded_at_ms";
/// Node row key: the verified principal the engine recorded the critique for.
pub const ROW_CRITIQUE_RECORDED_BY: &str = "critique_recorded_by";
/// Node row key prefix of one critique answer: `critique_answer_<id>` holds a boolean.
pub const ROW_CRITIQUE_ANSWER_PREFIX: &str = "critique_answer_";

/// The questions a critique of a run output proposal answers, as `(id, question)`
/// (MEDULLA-DECISION-ENGINE-R038.6). A critique answers exactly these ids.
pub const CRITIQUE_CHECKLIST: &[(&str, &str)] = &[
    (
        "grounded",
        "Is every claim in the output supported by evidence the run read?",
    ),
    (
        "complete",
        "Does the output answer everything the task asked?",
    ),
    (
        "scoped",
        "Does the output stay within the task's scope and the caller's permissions?",
    ),
    (
        "safe",
        "Is the output free of secrets, unsupported commitments and unapproved side effects?",
    ),
];

/// One typed question of the critique checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CritiqueQuestion {
    pub id: String,
    pub question: String,
}

/// The checklist of a run output proposal critique, in declared order.
pub fn critique_checklist() -> Vec<CritiqueQuestion> {
    CRITIQUE_CHECKLIST
        .iter()
        .map(|(id, question)| CritiqueQuestion {
            id: (*id).to_string(),
            question: (*question).to_string(),
        })
        .collect()
}

/// The kinds of held item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldItemKind {
    RunOutputProposal,
    StoredRunProgram,
}

impl HeldItemKind {
    /// The kind a wire name denotes; `None` for any other name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "run_output_proposal" => Some(Self::RunOutputProposal),
            "stored_run_program" => Some(Self::StoredRunProgram),
            _ => None,
        }
    }

    /// The wire name of the kind.
    pub fn name(self) -> &'static str {
        match self {
            Self::RunOutputProposal => "run_output_proposal",
            Self::StoredRunProgram => "stored_run_program",
        }
    }
}

/// The two answers of the gate that are not abstentions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldItemVerdict {
    Admitted,
    Rejected,
}

impl HeldItemVerdict {
    /// The option id the verdict is answered as.
    pub fn option_id(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Rejected => "rejected",
        }
    }
}

/// The decision-policy knob of held-item admission (DE-R059.1-KNOBS).
/// How a `held_item_admission` question concluded, as a caller reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum HeldItemOutcome {
    Admitted,
    Rejected,
    /// The question was refused; `reason` names the code.
    Abstained,
}

/// The typed answer of a `held_item_admission` question
/// (MEDULLA-DECISION-ENGINE-R038.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HeldItemAnswer {
    pub outcome: HeldItemOutcome,
    /// The decision record that sealed this answer.
    pub verdict_ref: String,
    /// The verification (a program) or the critique (a proposal) the outcome
    /// rests on; absent when the question was refused before reading one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_ref: Option<String>,
    /// Who verified a program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier: Option<String>,
    /// Who compiled a program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler: Option<String>,
    /// The refusal code when the outcome is `abstained`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The questions the critique of a run output proposal answers; empty for a program
    /// (MEDULLA-DECISION-ENGINE-R038.6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checklist: Vec<CritiqueQuestion>,
    /// The digest the critique the outcome rests on is bound to (a proposal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critique_digest: Option<String>,
    /// The verdict of that critique, `accept` or `reject` (a proposal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critique_verdict: Option<String>,
}

/// One answer of a critique: whether the output satisfies the question `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CritiqueAnswer {
    pub id: String,
    pub passed: bool,
}

/// The body of `DecisionEval` op `held_item_critique`: record the critique of one run output
/// proposal on the proposal's node (MEDULLA-DECISION-ENGINE-R038.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HeldItemCritiqueRequest {
    /// The tenant that owns the proposal; checked against the verified tenant.
    pub tenant_id: String,
    /// The graph holding the proposal.
    pub graph: String,
    /// The node id of the proposal.
    pub item_ref: String,
    /// The digest the caller expects the stored proposal to have.
    pub content_digest: String,
    /// One answer per checklist question, no more and no fewer.
    pub answers: Vec<CritiqueAnswer>,
}

/// What `held_item_critique` answers: the critique as the engine recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HeldItemCritiqueRecorded {
    /// The digest of the critique, written as `critique_ref` on the proposal node.
    pub critique_ref: String,
    pub item_ref: String,
    /// The proposal digest the critique is bound to.
    pub content_digest: String,
    /// `accept` when every question passed, otherwise `reject`.
    pub verdict: String,
}

/// The critique the engine derives from a request: its ref, its verdict and its answers in
/// checklist order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedCritique {
    pub critique_ref: String,
    pub verdict: &'static str,
    pub answers: Vec<CritiqueAnswer>,
}

/// The body a critique ref is the digest of.
#[derive(Serialize)]
struct CritiqueBody<'a> {
    tenant_id: &'a str,
    item_ref: &'a str,
    content_digest: &'a str,
    answers: &'a [CritiqueAnswer],
}

/// Derive the critique of `request`: the answers must be exactly the checklist ids, the verdict
/// is `accept` only when every answer passed, and the ref is the digest of the tenant, the item,
/// its digest and the answers in checklist order. The same request always derives the same ref.
pub fn derive_critique(request: &HeldItemCritiqueRequest) -> Result<DerivedCritique, String> {
    let mut answers = Vec::with_capacity(CRITIQUE_CHECKLIST.len());
    for (id, _) in CRITIQUE_CHECKLIST {
        let mut matching = request.answers.iter().filter(|answer| answer.id == *id);
        match (matching.next(), matching.next()) {
            (Some(answer), None) => answers.push(answer.clone()),
            _ => {
                return Err(format!(
                    "the critique answers the question `{id}` exactly once"
                ))
            }
        }
    }
    if request.answers.len() != answers.len() {
        return Err("the critique answers only the checklist questions".to_string());
    }
    let critique_ref = digest_text(
        HELD_ITEM_CRITIQUE_DIGEST_DOMAIN,
        &CritiqueBody {
            tenant_id: &request.tenant_id,
            item_ref: &request.item_ref,
            content_digest: &request.content_digest,
            answers: &answers,
        },
    );
    let verdict = if answers.iter().all(|answer| answer.passed) {
        "accept"
    } else {
        "reject"
    };
    Ok(DerivedCritique {
        critique_ref,
        verdict,
        answers,
    })
}

pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "held_item.min_passes",
    unit: "count",
    default: KnobValue::Integer(2),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(20),
    safety: KnobSafety::Tunable,
}];

/// The least verification passes that admit a program.
pub fn min_passes(policy: &DecisionPolicy) -> i64 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(v) => v,
        _ => 2,
    }
}

/// Whether `text` is a 64-character lowercase hex digest.
pub fn is_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Why the gate cannot admit or reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeldItemRefusal {
    /// The item is absent from the caller's view or is not of the named kind.
    ItemUnknown { item_ref: String },
    /// The stored digest differs from the one the caller expects.
    DigestMismatch { stored: String, expected: String },
    /// The program has no verification, or the verifier compiled it.
    VerifierNotIndependent { item_ref: String },
    /// The verification earned fewer passes than the floor.
    PassesInsufficient { passes: i64, required: i64 },
    /// The program was retired.
    Retired { item_ref: String },
    /// The proposal has no critique verdict bound to its digest.
    CritiqueMissing { item_ref: String },
}

/// The family's refusal table, registered in `code_registry::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("HELD_ITEM_UNKNOWN"),
    RefusalCode::new("DIGEST_MISMATCH"),
    RefusalCode::new("VERIFIER_NOT_INDEPENDENT"),
    RefusalCode::new("PASSES_INSUFFICIENT"),
    RefusalCode::new("RETIRED"),
    RefusalCode::new("CRITIQUE_MISSING"),
];

impl DecisionRefusal for HeldItemRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::ItemUnknown { .. } => REFUSALS[0],
            Self::DigestMismatch { .. } => REFUSALS[1],
            Self::VerifierNotIndependent { .. } => REFUSALS[2],
            Self::PassesInsufficient { .. } => REFUSALS[3],
            Self::Retired { .. } => REFUSALS[4],
            Self::CritiqueMissing { .. } => REFUSALS[5],
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::ItemUnknown { item_ref } => {
                format!("no held item `{item_ref}` of that kind is readable")
            }
            Self::DigestMismatch { stored, expected } => {
                format!("the stored digest {stored} differs from the expected {expected}")
            }
            Self::VerifierNotIndependent { item_ref } => format!(
                "program `{item_ref}` has no verification by someone other than its compiler"
            ),
            Self::PassesInsufficient { passes, required } => {
                format!("the verification earned {passes} passes, {required} are required")
            }
            Self::Retired { item_ref } => format!("program `{item_ref}` is retired"),
            Self::CritiqueMissing { item_ref } => {
                format!("proposal `{item_ref}` has no critique verdict bound to its digest")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R038.3
    #[test]
    fn kinds_digests_and_knob_are_closed() {
        assert_eq!(
            HeldItemKind::parse("stored_run_program"),
            Some(HeldItemKind::StoredRunProgram)
        );
        assert_eq!(HeldItemKind::parse("program"), None);
        assert!(is_digest(&"a".repeat(64)));
        assert!(!is_digest(&"A".repeat(64)));
        assert!(!is_digest("abc"));
        assert_eq!(min_passes(&DecisionPolicy::engine_default()), 2);
        let retired = HeldItemRefusal::Retired {
            item_ref: "p".into(),
        };
        assert_eq!(retired.code().as_str(), "RETIRED");
    }

    // spec: MEDULLA-DECISION-ENGINE-R038.6
    #[test]
    fn a_critique_answers_the_checklist_and_derives_its_verdict() {
        let request = |passes: &[bool]| HeldItemCritiqueRequest {
            tenant_id: "tenant-a".into(),
            graph: "held".into(),
            item_ref: "prop-1".into(),
            content_digest: "b".repeat(64),
            answers: CRITIQUE_CHECKLIST
                .iter()
                .zip(passes)
                .map(|((id, _), passed)| CritiqueAnswer {
                    id: (*id).to_string(),
                    passed: *passed,
                })
                .collect(),
        };
        let accepted = derive_critique(&request(&[true; 4])).unwrap();
        assert_eq!(accepted.verdict, "accept");
        assert!(accepted.critique_ref.starts_with("sha256:"));
        assert_eq!(
            accepted,
            derive_critique(&request(&[true; 4])).unwrap(),
            "the same request derives the same critique"
        );
        let rejected = derive_critique(&request(&[true, true, false, true])).unwrap();
        assert_eq!(rejected.verdict, "reject");
        assert_ne!(rejected.critique_ref, accepted.critique_ref);

        // The answers are exactly the checklist: a missing, a repeated and an unknown id refuse.
        let mut missing = request(&[true; 4]);
        missing.answers.pop();
        assert!(derive_critique(&missing).is_err());
        let mut repeated = request(&[true; 4]);
        repeated.answers[3] = repeated.answers[0].clone();
        assert!(derive_critique(&repeated).is_err());
        let mut extra = request(&[true; 4]);
        extra.answers.push(CritiqueAnswer {
            id: "other".into(),
            passed: true,
        });
        assert!(derive_critique(&extra).is_err());

        // Answer order does not change the ref.
        let mut shuffled = request(&[true; 4]);
        shuffled.answers.reverse();
        assert_eq!(derive_critique(&shuffled).unwrap(), accepted);
        assert_eq!(critique_checklist().len(), CRITIQUE_CHECKLIST.len());
    }
}
