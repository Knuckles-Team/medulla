//! The decision executor behind `Method::Decide` (EH-059, EH-005/006/023–025).
//!
//! It never builds a `RowSet` and never touches `wire::Op`: the candidates are
//! already the visible options, the feature schema and head are pinned
//! component bodies, and the whole answer is one schematised record. Order of
//! work, per the composition rule (§3.4): visibility (the candidate read) →
//! features over exactly those options → the head's advisory reading → the
//! act/abstain rule and any budgeted exploration inside the legal set.

use medulla_agent_types::{
    agent_component::AgentComponentKind, agent_component::ComponentDependency,
};
use medulla_decision_stats::decision::candidate::CandidateView;
use medulla_decision_stats::decision::exploration::permit;
use medulla_decision_stats::{
    decision::features::feature_matrix, decision::features::FeatureInputs,
    decision::features::FeatureMatrix, decision::features::MatrixOutcome,
};
use medulla_decision_stats::{
    decision::head_eval::check_compatible, decision::head_eval::explanation,
    decision::head_eval::read_head, decision::head_eval::Evaluated,
    decision::head_eval::HeadReading,
};
use medulla_decision_stats::{
    decision::ladder::decide, decision::ladder::LadderInputs, decision::ladder::LadderResult,
};
use medulla_decision_types::decision::statistical::features::FeatureSchemaBody;
use medulla_decision_types::decision::statistical::head::DecisionHeadBody;
use medulla_decision_types::decision::AbstainReason;
use medulla_decision_types::{
    decision::statistical::keyed::decision_seed, decision::statistical::keyed::exploration_key,
};
use medulla_decision_types::{
    decision::statistical::DecideRequest, decision::statistical::StatisticalErrorCode,
    decision::statistical::StatisticalOutcome, decision::statistical::StatisticalQuestion,
    decision::statistical::TypedParam,
};
use medulla_types::contract::BoundedVec;

use super::stat_support::{pinned_body, refusal, ResolvedPolicy};
use medulla_persistence_records::agent_library::AgentLibraryStore;

/// What the executor needs besides the request.
#[derive(Clone, Copy)]
pub(super) struct ExecutionContext<'a> {
    pub(super) store: &'a AgentLibraryStore,
    pub(super) tenant_id: &'a str,
    pub(super) now_ms: u64,
    pub(super) server_secret: &'a [u8],
    /// The caller's own earlier executed decisions that share the question's
    /// context (MEDULLA-DECISION-ENGINE-R020.1.1), where a decision reads them;
    /// empty for every job that decides nothing.
    pub(super) matched: &'a [super::stat_log::MatchedTrajectory],
    /// What a `capability_designation` question read beside the library: the
    /// resident index and the schema closure (MEDULLA-DECISION-ENGINE-R172.7).
    pub(super) designation: Option<&'a super::stat_designation::DesignationRuntime>,
    /// The caller's reader over the decision log, for a family that reads the
    /// caller's own earlier records (the same visibility rule as the log).
    /// `None` where a decision runs without a served caller.
    pub(super) reader: Option<&'a super::stat_log::LogReader>,
}

/// The pinned inputs of one decision.
pub(super) struct Pinned {
    pub(super) schema: FeatureSchemaBody,
    pub(super) schema_digest: String,
    pub(super) head: Option<(DecisionHeadBody, String)>,
}

/// What the executor concluded, before it is sealed into a record.
pub(super) struct Executed {
    pub(super) matrix: Option<FeatureMatrix>,
    pub(super) ladder: LadderResult,
    pub(super) reading: Option<Evaluated>,
    pub(super) seed: [u8; 32],
    /// The contrast decision a `HarnessContrast` question sealed
    /// (MEDULLA-DECISION-ENGINE-R803); `None` for every other kind.
    pub(super) harness_contrast:
        Option<medulla_decision_types::decision::harness_contrast::HarnessContrastDecision>,
    /// The proven plan and premises a `Topology` question sealed
    /// (MEDULLA-DECISION-ENGINE-R111.1); `None` for every other kind and for
    /// a topology abstention.
    pub(super) topology: Option<TopologyExecuted>,
    /// The expansion choice or stop a `ThoughtSearch` question sealed
    /// (MEDULLA-DECISION-ENGINE-R750.754); `None` for every other kind and
    /// for a thought-search abstention.
    pub(super) thought: Option<ThoughtExecuted>,
    /// The selected and omitted context sources a `ContextBudget` question
    /// sealed (MEDULLA-DECISION-ENGINE-R029.2.2); `None` for every other kind
    /// and when the head did not read.
    pub(super) context_selection:
        Option<medulla_decision_outcome_types::decision::context_budget::ContextSourceSelection>,
    /// The premises a family computed while it concluded, where a family's
    /// premises need what only the conclude stage reads (a profile lookup, the
    /// pinned policy's knobs); the family's `premises` hook seals them.
    pub(super) family_premises: Vec<FamilyPremise>,
    /// The route stage that answered a `Route` question
    /// (MEDULLA-DECISION-ENGINE-R035.750.1); `None` for every other kind and
    /// when the shared ladder (the learned policy) answered.
    pub(super) route: Option<RouteExecuted>,
    /// The typed answer and premises a `CapabilityDesignation`, `LoadSet` or `UsageGuidance`
    /// question sealed (MEDULLA-DECISION-ENGINE-R172.7, R176.3, R177.4); `None` for
    /// every other kind and for an abstention.
    pub(super) answered: Option<AnsweredFamily>,
}

/// Where a premise a family computed at conclude time came from; a policy
/// premise is bound to the digest of the policy in force when it is sealed.
pub(super) enum FamilyPremiseSource {
    /// The request stated it in the named field.
    Request(&'static str),
    /// A component publisher's declaration at an exact revision.
    Publisher {
        component_id: String,
        definition_digest: String,
    },
    /// The decision policy in force.
    Policy,
    /// A recorded fact read from the graph, by the node that holds it.
    Observation(String),
}

/// One premise a family computed while it concluded.
pub(super) struct FamilyPremise {
    pub(super) subject: &'static str,
    pub(super) fact: String,
    pub(super) class: medulla_decision_types::decision::PremiseClass,
    pub(super) source: FamilyPremiseSource,
}

impl FamilyPremise {
    /// The sealed premise, bound to `policy_digest` when it came from the policy.
    pub(super) fn seal(&self, policy_digest: &str) -> medulla_decision_types::decision::PremiseRef {
        use medulla_decision_types::decision::{PremiseProvenance, PremiseRef};
        PremiseRef {
            subject: self.subject.to_string(),
            fact: self.fact.clone(),
            class: self.class,
            provenance: match &self.source {
                FamilyPremiseSource::Request(field) => PremiseProvenance::Request {
                    field: (*field).to_string(),
                },
                FamilyPremiseSource::Publisher {
                    component_id,
                    definition_digest,
                } => PremiseProvenance::Publisher {
                    component_id: component_id.clone(),
                    definition_digest: definition_digest.clone(),
                },
                FamilyPremiseSource::Policy => PremiseProvenance::Policy {
                    policy_digest: policy_digest.to_string(),
                },
                FamilyPremiseSource::Observation(node_id) => PremiseProvenance::Observation {
                    evaluation_id: node_id.clone(),
                },
            },
        }
    }
}

/// The rule stage that answered a route question.
pub(super) struct RouteExecuted {
    /// The stage name recorded in the `route_stage=<stage>` premise.
    pub(super) stage: &'static str,
    /// The option the stage chose.
    pub(super) option_id: String,
    /// How the answer was reached: a rule is a constraint, evaluated history
    /// is statistical.
    pub(super) resolution: medulla_decision_types::decision::ResolutionKind,
    /// The matched earlier decisions that ran the chosen option, when the
    /// stage reads history (team reuse); empty otherwise.
    pub(super) matched_records: Vec<String>,
}

/// A family answer with the premises it rests on.
pub(super) struct AnsweredFamily {
    pub(super) answer: Option<medulla_decision_types::decision::statistical::outcome::FamilyAnswer>,
    pub(super) premises: Vec<medulla_decision_types::decision::PremiseRef>,
    /// The resolution kind the family seals its answer with, when the answer
    /// itself decides it; `None` keeps the family hook's.
    pub(super) resolution: Option<medulla_decision_types::decision::ResolutionKind>,
    /// The evidence class the family seals its answer with, when the answer
    /// itself decides it; `None` keeps the claim rule of the record stage.
    pub(super) evidence: Option<medulla_decision_types::decision::EvidenceClass>,
}

/// A thought-search answer with the premises it rests on.
pub(super) struct ThoughtExecuted {
    pub(super) answer: medulla_decision_types::decision::thought_search::ThoughtSearchAnswer,
    pub(super) premises: Vec<medulla_decision_types::decision::PremiseRef>,
}

/// A topology plan with the premises it rests on.
pub(super) struct TopologyExecuted {
    pub(super) plan: Box<medulla_agent_types::agent_topology::TopologyPlan>,
    pub(super) premises: Vec<medulla_decision_types::decision::PremiseRef>,
}

/// Read and validate the feature schema and head a decision pins.
pub(super) fn pinned_inputs(
    ctx: &ExecutionContext,
    schema_pin: &ComponentDependency,
    head_pin: Option<&ComponentDependency>,
) -> Result<Pinned, String> {
    let (schema, schema_digest) = pinned_body::<FeatureSchemaBody>(
        ctx.store,
        ctx.tenant_id,
        schema_pin,
        AgentComponentKind::FeatureSchema,
        StatisticalErrorCode::FeatureSchemaInvalid,
    )?;
    let schema = schema
        .checked()
        .map_err(|detail| refusal(StatisticalErrorCode::FeatureSchemaInvalid, detail))?;
    let head = head_pin
        .map(|pin| {
            let (body, digest) = pinned_body::<DecisionHeadBody>(
                ctx.store,
                ctx.tenant_id,
                pin,
                AgentComponentKind::DecisionHead,
                StatisticalErrorCode::HeadInvalid,
            )?;
            let body = body
                .checked()
                .map_err(|detail| refusal(StatisticalErrorCode::HeadInvalid, detail))?;
            Ok::<_, String>((body, digest))
        })
        .transpose()?;
    Ok(Pinned {
        schema,
        schema_digest,
        head,
    })
}

fn abstained(reason: AbstainReason) -> LadderResult {
    LadderResult {
        outcome: StatisticalOutcome::Abstained {
            reasons: BoundedVec::new(vec![reason]).expect("one reason is inside the bound"),
        },
        calibration: None,
        explained: None,
        audit: None,
        logging: Vec::new(),
    }
}

fn head_reading(
    pinned: &Pinned,
    matrix: &FeatureMatrix,
) -> Result<Option<Result<Evaluated, AbstainReason>>, String> {
    let Some((head, _)) = &pinned.head else {
        return Ok(None);
    };
    check_compatible(head, &pinned.schema_digest, matrix).map_err(|r| r.render())?;
    Ok(Some(
        match read_head(head, matrix).map_err(|r| r.render())? {
            HeadReading::InDistribution(evaluated) => Ok(evaluated),
            HeadReading::OutOfDistribution { .. } => Err(AbstainReason::InsufficientConfidence),
        },
    ))
}

/// The state digest the exploration seed is keyed on: everything the decision
/// read, so a caller cannot change the draw without changing the inputs.
pub(super) fn state_digest(
    question: &StatisticalQuestion,
    params: &[TypedParam],
    matrix: &FeatureMatrix,
    policy: &ResolvedPolicy,
    now_ms: u64,
) -> String {
    medulla_query_types::solve_scalar::digest_text(
        "eg/decide-state/v1",
        &(
            question,
            Some((&matrix.candidate_ids, &matrix.values)),
            &policy.digest,
            params,
            now_ms,
        ),
    )
}

/// What the decision function reads once the matrix exists. `execute` builds
/// it from live candidates; the log's verify-replay builds it from a record.
pub(super) struct MatrixInputs<'a> {
    pub(super) question: &'a StatisticalQuestion,
    pub(super) params: &'a [TypedParam],
    pub(super) now_ms: u64,
    pub(super) server_secret: &'a [u8],
}

/// The decision function over a complete matrix: head reading, drift check,
/// keyed seed and the act/abstain ladder.
pub(super) fn run_on_matrix(
    inputs: &MatrixInputs,
    pinned: &Pinned,
    policy: &ResolvedPolicy,
    matrix: FeatureMatrix,
) -> Result<Executed, String> {
    let exploration = permit(&policy.policy, inputs.question).map_err(|r| r.render())?;
    let digest = state_digest(
        inputs.question,
        inputs.params,
        &matrix,
        policy,
        inputs.now_ms,
    );
    let seed = decision_seed(
        &exploration_key(inputs.server_secret),
        &digest,
        &inputs.question.question_id,
    );
    let reading = match head_reading(pinned, &matrix)? {
        Some(Err(reason)) => {
            return Ok(Executed {
                matrix: Some(matrix),
                ladder: abstained(reason),
                reading: None,
                seed,
                harness_contrast: None,
                topology: None,
                context_selection: None,
                thought: None,
                family_premises: Vec::new(),
                route: None,
                answered: None,
            });
        }
        Some(Ok(evaluated)) => Some(evaluated),
        None => None,
    };
    let ladder = decide(&LadderInputs {
        candidate_ids: &matrix.candidate_ids,
        head: pinned.head.as_ref().map(|(head, _)| head),
        reading: reading.as_ref(),
        policy: &policy.policy,
        statistical: &policy.statistical,
        permit: exploration,
        seed: &seed,
    })
    .map_err(|r| r.render())?;
    Ok(Executed {
        matrix: Some(matrix),
        ladder,
        reading,
        seed,
        harness_contrast: None,
        topology: None,
        context_selection: None,
        thought: None,
        family_premises: Vec::new(),
        route: None,
        answered: None,
    })
}

/// Run the ladder over the candidates.
pub(super) fn execute(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    pinned: &Pinned,
    policy: &ResolvedPolicy,
    views: &[CandidateView],
) -> Result<Executed, String> {
    permit(&policy.policy, &request.question).map_err(|r| r.render())?;
    let features = FeatureInputs {
        params: request.params.as_slice(),
        now_ms: ctx.now_ms,
    };
    let matrix = match feature_matrix(&pinned.schema, views, &features).map_err(|r| r.render())? {
        MatrixOutcome::Complete(matrix) => matrix,
        MatrixOutcome::UnknownFact {
            component_id,
            field,
        } => {
            return Ok(Executed {
                matrix: None,
                ladder: abstained(AbstainReason::UnknownFact {
                    component_id,
                    field,
                }),
                reading: None,
                seed: [0u8; 32],
                harness_contrast: None,
                topology: None,
                context_selection: None,
                thought: None,
                family_premises: Vec::new(),
                route: None,
                answered: None,
            });
        }
    };
    let inputs = MatrixInputs {
        question: &request.question,
        params: request.params.as_slice(),
        now_ms: ctx.now_ms,
        server_secret: ctx.server_secret,
    };
    run_on_matrix(&inputs, pinned, policy, matrix)
}

/// The recorded explanation of the explained option, for a linear head
/// (an `OptionAttention` head records none).
pub(super) fn recorded_explanation(
    pinned: &Pinned,
    executed: &Executed,
) -> Result<Option<medulla_decision_types::decision::statistical::LinearExplanation>, String> {
    let (Some((head, _)), Some(reading), Some(index), Some(matrix)) = (
        &pinned.head,
        &executed.reading,
        executed.ladder.explained,
        &executed.matrix,
    ) else {
        return Ok(None);
    };
    explanation(
        head,
        &matrix.candidate_ids[index],
        &reading.standardised[index],
    )
    .map_err(|r| r.render())
}
