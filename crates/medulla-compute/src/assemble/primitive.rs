//! Edge type-check and substitution rule for typed primitives
//! (MEDULLA-DECISION-ENGINE-R036.100.2).
//!
//! A harness graph is composed of primitives whose [`PrimitiveContract`]s declare
//! the digest of their input and output schemas, their effects and their
//! capabilities. This module is pure: it reads two contracts (or an edge count)
//! and answers legal or a typed [`PrimitiveRefusal`]. The `assemble` family hook
//! calls it before the solver runs, so an ill-typed edge or an unsafe replacement
//! never reaches `decide_one`.

use medulla_decision_types::decision::primitive::PrimitiveContract;
use medulla_decision_types::decision::record::outcome::{
    AbstainReason, DecisionRefusal, RefusalCode, RefusalOrigin,
};
use medulla_decision_types::decision::{
    CandidateFacts, DecisionErrorCode, DecisionInputs, Elimination, Violation,
};

use super::AssembleError;

/// Why an edge or a replacement is refused. Each variant is one registered
/// refusal code of the `assemble` family; [`REFUSALS`] lists them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrimitiveRefusal {
    /// The producer's output schema is not the consumer's input schema.
    EdgeMismatch,
    /// A replacement is of another primitive kind.
    KindMismatch,
    /// A replacement changes the input or the output schema digest.
    SchemaChanged,
    /// A replacement drops a capability the old primitive had.
    CapabilityLost,
    /// A replacement has an effect the old primitive did not have.
    EffectWidened,
    /// The graph has more edges than the `primitive.max_edges` knob admits.
    GraphTooLarge,
}

/// Every refusal of the primitive rules, in the order [`PrimitiveRefusal`] declares them.
pub const REFUSALS: [PrimitiveRefusal; 6] = [
    PrimitiveRefusal::EdgeMismatch,
    PrimitiveRefusal::KindMismatch,
    PrimitiveRefusal::SchemaChanged,
    PrimitiveRefusal::CapabilityLost,
    PrimitiveRefusal::EffectWidened,
    PrimitiveRefusal::GraphTooLarge,
];

impl PrimitiveRefusal {
    /// The registered refusal code a caller branches on.
    pub const fn code(self) -> &'static str {
        match self {
            Self::EdgeMismatch => "PRIMITIVE_EDGE_MISMATCH",
            Self::KindMismatch => "PRIMITIVE_KIND_MISMATCH",
            Self::SchemaChanged => "PRIMITIVE_SCHEMA_CHANGED",
            Self::CapabilityLost => "PRIMITIVE_CAPABILITY_LOST",
            Self::EffectWidened => "PRIMITIVE_EFFECT_WIDENED",
            Self::GraphTooLarge => "PRIMITIVE_GRAPH_TOO_LARGE",
        }
    }
}

impl std::fmt::Display for PrimitiveRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// An edge type-checks when the producer's output schema digest equals the
/// consumer's input schema digest.
pub fn edge_type_checks(producer: &PrimitiveContract, consumer: &PrimitiveContract) -> bool {
    producer.output_schema_digest == consumer.input_schema_digest
}

/// The edge's refusal: `Ok` when it type-checks, else `PRIMITIVE_EDGE_MISMATCH`.
pub fn require_edge(
    producer: &PrimitiveContract,
    consumer: &PrimitiveContract,
) -> Result<(), PrimitiveRefusal> {
    if edge_type_checks(producer, consumer) {
        Ok(())
    } else {
        Err(PrimitiveRefusal::EdgeMismatch)
    }
}

/// Whether `new` may replace `old` in a graph.
///
/// Legal when the kinds and both schema digests are equal, the new capabilities
/// are a superset of the old and the new effects a subset of the old. The first
/// failing condition is the refusal, checked in that order.
pub fn substitutes(
    old: &PrimitiveContract,
    new: &PrimitiveContract,
) -> Result<(), PrimitiveRefusal> {
    if old.kind != new.kind {
        return Err(PrimitiveRefusal::KindMismatch);
    }
    if old.input_schema_digest != new.input_schema_digest
        || old.output_schema_digest != new.output_schema_digest
    {
        return Err(PrimitiveRefusal::SchemaChanged);
    }
    if !old.capabilities.is_subset(&new.capabilities) {
        return Err(PrimitiveRefusal::CapabilityLost);
    }
    if !new.effects.is_subset(&old.effects) {
        return Err(PrimitiveRefusal::EffectWidened);
    }
    Ok(())
}

/// Refuse a graph with more edges than `max_edges`, the resolved value of the
/// `primitive.max_edges` knob (the caller reads the knob; this crate never does).
pub fn check_graph_size(edges: usize, max_edges: u64) -> Result<(), PrimitiveRefusal> {
    if u64::try_from(edges).map_or(true, |count| count > max_edges) {
        Err(PrimitiveRefusal::GraphTooLarge)
    } else {
        Ok(())
    }
}

impl PrimitiveRefusal {
    /// The registered refusal code (`medulla_decision_types::decision::codes`).
    pub const fn refusal_code(self) -> RefusalCode {
        RefusalCode::new(self.code())
    }
}

/// One primitive refusal with the ids it concerns, folded into an abstention
/// under the assemble origin.
struct Refused {
    refusal: PrimitiveRefusal,
    detail: String,
}

impl DecisionRefusal for Refused {
    fn code(&self) -> RefusalCode {
        self.refusal.refusal_code()
    }

    fn detail(&self) -> String {
        self.detail.clone()
    }
}

/// What the primitive rules removed from a request: the eliminated option of
/// each failed check and the abstention reason that reports it.
pub(super) struct PrimitiveScreen {
    pub eliminated: Vec<Elimination>,
    pub reasons: Vec<AbstainReason>,
}

fn candidate<'a>(
    inputs: &'a DecisionInputs,
    component_id: &str,
) -> Result<&'a CandidateFacts, AssembleError> {
    inputs
        .candidates
        .iter()
        .find(|candidate| candidate.component_id == component_id)
        .ok_or_else(|| {
            AssembleError::new(
                DecisionErrorCode::AssemblyInputsInvalid,
                format!("primitive wiring names `{component_id}`, which is not a candidate"),
            )
        })
}

fn refuse(screen: &mut PrimitiveScreen, removed: &str, refusal: PrimitiveRefusal, detail: String) {
    screen.eliminated.push(Elimination {
        component_id: removed.to_string(),
        violation: Violation::PrimitiveContract {
            code: refusal.code().to_string(),
        },
    });
    screen.reasons.push(AbstainReason::from_refusal(
        RefusalOrigin::ASSEMBLE,
        &Refused { refusal, detail },
    ));
}

/// Check every edge and every replacement the request names against the
/// candidates' published contracts. A candidate with no contract skips the
/// checks and stays legal. `None` when nothing is refused; a name outside the
/// candidate set is an invalid input, not a refusal of a primitive.
pub(super) fn screen_request(
    inputs: &DecisionInputs,
) -> Result<Option<PrimitiveScreen>, AssembleError> {
    let mut screen = PrimitiveScreen {
        eliminated: Vec::new(),
        reasons: Vec::new(),
    };
    for edge in &inputs.request.primitive_edges {
        let producer = candidate(inputs, &edge.producer)?;
        let consumer = candidate(inputs, &edge.consumer)?;
        let (Some(from), Some(to)) = (&producer.primitive, &consumer.primitive) else {
            continue;
        };
        if let Err(refusal) = require_edge(from, to) {
            let detail = format!(
                "`{}` output does not match `{}` input",
                edge.producer, edge.consumer
            );
            refuse(&mut screen, &edge.consumer, refusal, detail);
        }
    }
    for replacement in &inputs.request.replacements {
        let old = candidate(inputs, &replacement.old)?;
        let new = candidate(inputs, &replacement.new)?;
        let (Some(old_contract), Some(new_contract)) = (&old.primitive, &new.primitive) else {
            continue;
        };
        if let Err(refusal) = substitutes(old_contract, new_contract) {
            let detail = format!(
                "`{}` may not replace `{}`",
                replacement.new, replacement.old
            );
            refuse(&mut screen, &replacement.new, refusal, detail);
        }
    }
    Ok((!screen.reasons.is_empty()).then_some(screen))
}
