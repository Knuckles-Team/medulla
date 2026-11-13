//! Corpus re-embedding as a governed generation swap (EH-397).
//!
//! Stored vectors are never mutated in place, and an embedding space is fixed
//! for the life of a store. A re-embed with a new (fine-tuned) model is
//! therefore a NEW graph generation: AU trains the model under a capacity
//! lease, re-ingests the corpus into a shadow graph whose store declares the
//! new space, and the engine builds that graph's own ANN generation like any
//! other. Before a logical graph may resolve to the shadow, the engine
//! dual-serves the judged retrieval runs of the log against both generations
//! (AU supplies each run's query embedded in BOTH spaces -- the engine has no
//! model) and issues a receipt:
//!
//! * coverage: the shadow embeds at least as many rows as the active one;
//! * quality: a paired sign test of the reciprocal rank of the best cited
//!   unit, shadow against active, over independently judged runs only;
//! * score stability: the PSI of the top-1 similarity distribution. A large
//!   shift means every score threshold calibrated on the old space is void, so
//!   it gates activation (`max_score_psi_q16`).
//!
//! Activation moves the logical graph's pointer to the shadow only with the
//! passing receipt that measured it against the CURRENTLY active generation;
//! rollback returns to the generation before, which is never deleted by it.

use serde::{Deserialize, Serialize};

use super::retrieval::MAX_QUERY_DIMENSIONS;
use crate::decision::record::outcome::RefusalCode;
use medulla_types::contract::BoundedVec;

/// Format identity of a generation receipt.
pub const GENERATION_RECEIPT_SCHEMA_VERSION: u16 = 1;
/// Most judged runs one evaluation replays.
pub const MAX_GENERATION_EVAL_ITEMS: usize = 1_024;
/// Largest top-k a generation evaluation probes.
pub const MAX_GENERATION_TOP_K: u16 = 256;

/// One judged run, its query embedded in both spaces (`q16`, `x / 2^16`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GenerationEvalItem {
    pub record_id: String,
    pub active_q16: BoundedVec<i32, MAX_QUERY_DIMENSIONS>,
    pub shadow_q16: BoundedVec<i32, MAX_QUERY_DIMENSIONS>,
}

/// Evaluate a shadow generation against the active one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GenerationEvalRequest {
    /// The logical graph callers name.
    pub logical: String,
    /// The generation the logical graph resolves to now.
    pub active_graph: String,
    /// The embedding model identity of the active generation.
    pub active_space: String,
    pub shadow_graph: String,
    /// The (new) embedding model identity of the shadow generation.
    pub shadow_space: String,
    pub items: BoundedVec<GenerationEvalItem, MAX_GENERATION_EVAL_ITEMS>,
    pub top_k: u16,
    pub min_eval_items: u32,
    /// Largest admissible top-1 score PSI, on `Q16` (0.25 = 16384).
    pub max_score_psi_q16: i64,
}

/// The receipt a generation activation must present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GenerationReceipt {
    pub schema_version: u16,
    pub logical: String,
    pub active_graph: String,
    pub active_space: String,
    pub shadow_graph: String,
    pub shadow_space: String,
    pub active_embedded: u64,
    pub shadow_embedded: u64,
    pub n_eval: u64,
    pub wins: u64,
    pub losses: u64,
    pub ties: u64,
    pub active_mrr_q16: i64,
    pub shadow_mrr_q16: i64,
    pub win_rate_lower_q16: i64,
    /// `None` when this build cannot compute it (no `ann`); then never passed.
    #[serde(default)]
    pub score_psi_q16: Option<i64>,
    pub passed: bool,
}

/// A stored receipt and the digest it is pinned by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GenerationEvaluated {
    pub receipt_digest: String,
    pub receipt: GenerationReceipt,
}

// ---------------------------------------------------------------------------
// The generation cycle (MEDULLA-DECISION-ENGINE-R174): publish a model,
// re-embed the shadow graph in checkpointed batches, evaluate, activate.
// ---------------------------------------------------------------------------

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// Format identity of a generation cycle and its job record.
pub const GENERATION_CYCLE_SCHEMA_VERSION: u16 = 1;

/// Refusal: the model dimension is not the shadow generation's.
pub const GENERATION_DIMENSION_MISMATCH: RefusalCode =
    RefusalCode::new("GENERATION_DIMENSION_MISMATCH");
/// Recorded on a cycle whose evaluation receipt failed.
pub const GENERATION_RECEIPT_FAILED: RefusalCode = RefusalCode::new("GENERATION_RECEIPT_FAILED");
/// Refusal: the pointer moved while the cycle activated.
pub const GENERATION_POINTER_CONFLICT: RefusalCode =
    RefusalCode::new("GENERATION_POINTER_CONFLICT");

/// The refusal codes of the generation cycle (MEDULLA-DECISION-ENGINE-R174.4),
/// in the retrieval-learning table.
pub const REFUSALS: &[RefusalCode] = &[
    GENERATION_DIMENSION_MISMATCH,
    GENERATION_RECEIPT_FAILED,
    GENERATION_POINTER_CONFLICT,
];

/// The embedding model a cycle publishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ModelRef {
    pub id: String,
    pub revision: String,
    pub dimension: u32,
}

/// The closed states of `GenerationCycleMachine`; the stored text is the
/// snake_case name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GenerationCycleState {
    Published,
    Reembedding,
    Evaluated,
    Activated,
    Rejected,
}

impl MachineState for GenerationCycleState {
    fn stored(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Reembedding => "reembedding",
            Self::Evaluated => "evaluated",
            Self::Activated => "activated",
            Self::Rejected => "rejected",
        }
    }
}

/// The closed events of `GenerationCycleMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationCycleEvent {
    /// The first re-embedding batch starts.
    ReembedBegun,
    /// Every node is re-embedded and the receipt passed.
    EvaluationPassed,
    /// Every node is re-embedded and the receipt did not pass.
    EvaluationFailed,
    /// The generation pointer moved to the shadow graph.
    PointerMoved,
}

impl MachineEvent for GenerationCycleEvent {
    fn name(self) -> &'static str {
        match self {
            Self::ReembedBegun => "ReembedBegun",
            Self::EvaluationPassed => "EvaluationPassed",
            Self::EvaluationFailed => "EvaluationFailed",
            Self::PointerMoved => "PointerMoved",
        }
    }
}

/// `GenerationCycleMachine`: the closed edge table. Every other pair of state
/// and event is refused with `TRANSITION_REFUSED`.
pub static GENERATION_CYCLE_MACHINE: MachineDef<
    GenerationCycleState,
    GenerationCycleEvent,
    &'static str,
> = MachineDef {
    name: "GenerationCycleMachine",
    states: &[
        GenerationCycleState::Published,
        GenerationCycleState::Reembedding,
        GenerationCycleState::Evaluated,
        GenerationCycleState::Activated,
        GenerationCycleState::Rejected,
    ],
    initial: GenerationCycleState::Published,
    terminals: &[
        GenerationCycleState::Activated,
        GenerationCycleState::Rejected,
    ],
    events: &[
        GenerationCycleEvent::ReembedBegun,
        GenerationCycleEvent::EvaluationPassed,
        GenerationCycleEvent::EvaluationFailed,
        GenerationCycleEvent::PointerMoved,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "G01",
            from: GenerationCycleState::Published,
            event: GenerationCycleEvent::ReembedBegun,
            guard: None,
            to: GenerationCycleState::Reembedding,
        },
        Edge {
            id: "G02",
            from: GenerationCycleState::Reembedding,
            event: GenerationCycleEvent::EvaluationPassed,
            guard: None,
            to: GenerationCycleState::Evaluated,
        },
        Edge {
            id: "G03",
            from: GenerationCycleState::Reembedding,
            event: GenerationCycleEvent::EvaluationFailed,
            guard: None,
            to: GenerationCycleState::Rejected,
        },
        Edge {
            id: "G04",
            from: GenerationCycleState::Evaluated,
            event: GenerationCycleEvent::PointerMoved,
            guard: None,
            to: GenerationCycleState::Activated,
        },
    ],
};

impl GenerationCycleState {
    /// Whether no event leaves this state.
    pub fn is_terminal(self) -> bool {
        GENERATION_CYCLE_MACHINE.is_terminal(self)
    }
}

/// One re-embedding cycle of a logical graph under a new model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GenerationCycle {
    pub logical_graph: String,
    pub model_ref: ModelRef,
    /// The one persisted state field; only [`GenerationCycle::apply`] writes it.
    state: GenerationCycleState,
    pub shadow_graph: String,
    /// The digest of the evaluation receipt, once the cycle has one.
    #[serde(default)]
    pub receipt_digest: Option<String>,
    pub schema_version: u16,
}

impl GenerationCycle {
    /// A newly published cycle, in `published`.
    pub fn publish(
        logical_graph: &str,
        model_ref: ModelRef,
        shadow_graph: &str,
    ) -> Result<Self, String> {
        let named = !logical_graph.is_empty()
            && !shadow_graph.is_empty()
            && logical_graph != shadow_graph
            && !model_ref.id.is_empty()
            && !model_ref.revision.is_empty()
            && model_ref.dimension > 0;
        if !named {
            return Err(
                "a generation cycle names a logical graph, a distinct shadow graph \
                        and a model with an id, a revision and a dimension"
                    .to_string(),
            );
        }
        Ok(Self {
            logical_graph: logical_graph.to_string(),
            model_ref,
            state: GENERATION_CYCLE_MACHINE.initial,
            shadow_graph: shadow_graph.to_string(),
            receipt_digest: None,
            schema_version: GENERATION_CYCLE_SCHEMA_VERSION,
        })
    }

    /// The current state.
    pub fn state(&self) -> GenerationCycleState {
        self.state
    }

    /// The one transition function: fire `event` and, when given, pin the
    /// evaluation receipt digest. An illegal event is refused and changes nothing.
    pub fn apply(
        &mut self,
        event: GenerationCycleEvent,
        receipt_digest: Option<String>,
    ) -> Result<(), TransitionRefused> {
        let edge = transition(&GENERATION_CYCLE_MACHINE, self.state, event, |_| true)?;
        self.state = edge.to;
        if receipt_digest.is_some() {
            self.receipt_digest = receipt_digest;
        }
        Ok(())
    }
}

/// Run one generation cycle: the model, the evaluation it must pass and the
/// capacity it may spend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GenerationCycleRequest {
    pub model_ref: ModelRef,
    /// Names the logical graph, the active and shadow generations and the
    /// judged runs the receipt replays.
    pub evaluation: GenerationEvalRequest,
    /// The capacity cell the background lease is drawn from.
    pub capacity_cell: String,
    /// Most batches this submission may run (`None` runs to the end); a
    /// cycle that stops here resumes from its checkpoint on the next submit.
    #[serde(default)]
    pub max_batches: Option<u32>,
}

/// What one cycle recorded when it activated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GenerationCycleReceipt {
    pub schema_version: u16,
    pub model_ref: ModelRef,
    /// Nodes re-embedded into the shadow graph.
    pub reembedded_nodes: u64,
    /// The comparison metrics and their digest.
    pub evaluation: GenerationEvaluated,
    /// The generation the logical graph resolved to before the move.
    pub pointer_before: Option<String>,
    /// The generation it resolves to after the move.
    pub pointer_after: Option<String>,
}

/// The job record of one cycle, keyed by logical graph and model revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GenerationCycleJob {
    pub schema_version: u16,
    pub job_id: String,
    pub request_digest: String,
    pub cycle: GenerationCycle,
    /// The background capacity lease the job requested before its first batch.
    #[serde(default)]
    pub lease_id: Option<String>,
    /// Index of the next node to re-embed (the checkpoint).
    pub next_node: u64,
    pub reembedded_nodes: u64,
    pub batches: u64,
    #[serde(default)]
    pub receipt: Option<GenerationCycleReceipt>,
    /// The refusal code recorded when the cycle ended `rejected`
    /// (`GENERATION_RECEIPT_FAILED`); absent in every other state.
    #[serde(default)]
    pub rejection: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelRef {
        ModelRef {
            id: "m".into(),
            revision: "2".into(),
            dimension: 8,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.1
    #[test]
    fn cycle_round_trips() {
        let cycle = GenerationCycle::publish("kg", model(), "kg-2").unwrap();
        let json = serde_json::to_string(&cycle).unwrap();
        assert!(json.contains("\"state\":\"published\""));
        let back: GenerationCycle = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cycle);
        let unknown = json.replace("published", "paused");
        assert!(serde_json::from_str::<GenerationCycle>(&unknown).is_err());
        assert!(GenerationCycle::publish("kg", model(), "kg").is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.1
    #[test]
    fn every_legal_transition_moves_and_every_other_is_refused() {
        use GenerationCycleEvent as E;
        use GenerationCycleState as S;
        let legal = [
            (S::Published, E::ReembedBegun, S::Reembedding),
            (S::Reembedding, E::EvaluationPassed, S::Evaluated),
            (S::Reembedding, E::EvaluationFailed, S::Rejected),
            (S::Evaluated, E::PointerMoved, S::Activated),
        ];
        let events = [
            E::ReembedBegun,
            E::EvaluationPassed,
            E::EvaluationFailed,
            E::PointerMoved,
        ];
        for state in GENERATION_CYCLE_MACHINE.states {
            for event in events {
                let mut cycle = GenerationCycle::publish("kg", model(), "kg-2").unwrap();
                cycle.state = *state;
                let result = cycle.apply(event, None);
                match legal
                    .iter()
                    .find(|(from, on, _)| from == state && *on == event)
                {
                    Some((_, _, to)) => {
                        result.unwrap();
                        assert_eq!(cycle.state(), *to);
                    }
                    None => {
                        let refused = result.unwrap_err();
                        assert_eq!(refused.machine, "GenerationCycleMachine");
                        assert_eq!(refused.from, state.stored());
                        assert_eq!(cycle.state(), *state, "a refused event changes nothing");
                    }
                }
            }
        }
    }
}
