//! Held-item admission (MEDULLA-DECISION-ENGINE-R038.3).
//!
//! A run holds two kinds of stored item until something admits them: a
//! compiled run program and a run output proposal. Each is a node of the graph
//! the question names. The node records the item's content digest and, by
//! kind, the facts admission reads: who compiled and who verified a program,
//! how many verification passes it earned and whether it was retired; which
//! critique verdict a proposal carries and the digest that verdict is bound
//! to. Admission is advisory and writes nothing.

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::record::outcome::{DecisionRefusal, RefusalCode};

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

/// The family's refusal table, registered in `codes::FAMILY_TABLES`.
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
}
