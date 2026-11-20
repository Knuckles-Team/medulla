//! The `harness_config` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Reads the model profile and task-class facts the harness rules decide
//! from (MEDULLA-DECISION-ENGINE-R128.8), decides the three-part
//! configuration with the pure harness rules (R128.9), explains every part
//! with premises and why-not entries (R128.11) and seals decided parts as an
//! advisory outcome and the evidence the replay re-derives (R128.12).

use medulla_agent_types::agent_component::{
    AgentComponentFacts, AgentComponentKind, AgentComponentSearchRequest, MeasuredFact,
    ObservationRef, ShellProficiency, StoppingBehaviour,
};
use std::sync::Arc;

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::harness_config::{
    decide_action_space, decide_context_policy, decide_plan_scaffold, ActionSpace, ContextPolicy,
    HarnessConfigAnswer, HarnessConfigEvaluationEvidence, HarnessConfigRequestBody,
    HarnessDefaults, HarnessEvidenceLimits, HarnessPart, HarnessReplayLimits,
    HarnessSolverCertificate, PlanScaffold, PlanScaffoldReason, HARNESS_REPLAY_MISMATCH,
};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::tool_subset::ToolSubsetRefusal;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, ScoredOption, StatisticalDecisionRecord, StatisticalErrorCode,
    StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, CandidateSourceRecord, DecisionPolicy, PremiseClass, PremiseProvenance,
    PremiseRef, QuantScaleTag, QuantisedValue,
};
use medulla_graph_core::graph::GraphView;
use medulla_identity_types::assistant_profile::AssistantProfile;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;
use tokio::sync::RwLock;

use super::DecisionFamily;
use crate::decide::candidates::DECLARED_OPTIONS_DOMAIN;
use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyExecuted, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::{filtered_view, invalid_parameter, refusal};
use crate::decide::stat_tool_subset::{
    solved_covering_subset, token_cost, tool_subset_body, SubsetFailure,
};

/// The profile a request names is absent, retired or outside the tenant.
pub const HARNESS_PROFILE_UNKNOWN: &str = "HARNESS_PROFILE_UNKNOWN";

/// The assistant profile's allowlist removed every declared tool candidate
/// (MEDULLA-DECISION-ENGINE-R128.16). Registered in the `participant`
/// refusal table; here it names the missing fact of the abstained action space
/// and the why-not entry of each removed tool.
pub const PROFILE_TOOL_NOT_ALLOWED: &str = "PROFILE_TOOL_NOT_ALLOWED";

const SWARM: &str = "https://knuckles-team.github.io/ontology/swarm#";
const PAGE: u32 = 256;
const MAX_PAGES: usize = 64;

/// The family of the `harness_config` question kind.
pub struct HarnessConfigFamily;

/// The one score every decided part carries: the Q32 value one, uncalibrated.
const DECIDED_SCORE: i64 = 1i64 << 32;

impl DecisionFamily for HarnessConfigFamily {
    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        if !matches!(request.candidates, CandidateSource::Declared { .. }) {
            return Err(invalid_parameter(
                "a harness_config question declares its own tool candidates",
            ));
        }
        request_body(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_config(ctx, request, policy, graph_view))
    }
}

/// The RLS-filtered snapshot of the optional `graph` param, where the task
/// class and profile nodes live. Without the param the graph facts are unknown.
pub(crate) async fn evidence_view(
    state: &Arc<RwLock<ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<GraphView>, String> {
    match param(request, "graph") {
        None => Ok(None),
        Some(TypedValue::Text(graph)) if !graph.is_empty() => {
            filtered_view(state, agent_id, graph).await.map(Some)
        }
        Some(_) => Err(invalid_parameter("graph must be non-empty text")),
    }
}

/// The facts one `harness_config` question decides from. An absent fact is
/// `None`, never a default value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HarnessFacts {
    pub profile_digest: String,
    pub context_window_tokens: u32,
    pub shell_proficiency: Option<MeasuredFact<ShellProficiency>>,
    pub stopping_behaviour: Option<MeasuredFact<StoppingBehaviour>>,
    pub shell_centric: Option<bool>,
    pub task_token_upper_bound: Option<u64>,
    pub plan_scaffold_no_gain: Option<MeasuredFact<bool>>,
}

fn param<'a>(request: &'a DecideRequest, name: &str) -> Option<&'a TypedValue> {
    request
        .params
        .iter()
        .find(|param| param.name == name)
        .map(|param| &param.value)
}

fn required_iri(request: &DecideRequest, name: &str) -> Result<String, String> {
    match param(request, name) {
        Some(TypedValue::Iri(iri)) if !iri.is_empty() => Ok(iri.clone()),
        Some(_) => Err(invalid_parameter(format!("{name} must be a non-empty iri"))),
        None => Err(invalid_parameter(format!(
            "a harness_config question requires a {name} (iri) param"
        ))),
    }
}

fn optional_tokens(request: &DecideRequest, name: &str) -> Result<Option<u32>, String> {
    match param(request, name) {
        None => Ok(None),
        Some(TypedValue::Int(tokens)) => u32::try_from(*tokens)
            .map(Some)
            .map_err(|_| invalid_parameter(format!("{name} must fit a non-negative u32"))),
        Some(_) => Err(invalid_parameter(format!("{name} must be an int"))),
    }
}

/// The typed body of a `harness_config` question, read from its params
/// (MEDULLA-DECISION-ENGINE-R128.2).
pub(crate) fn request_body(request: &DecideRequest) -> Result<HarnessConfigRequestBody, String> {
    let budget = optional_tokens(request, "usable_context_budget_tokens")?.ok_or_else(|| {
        invalid_parameter(
            "a harness_config question requires a usable_context_budget_tokens (int) param",
        )
    })?;
    let priority = match param(request, "priority") {
        Some(TypedValue::Text(text)) => {
            serde_json::from_value(serde_json::Value::String(text.clone()))
                .map_err(|_| invalid_parameter("priority must be interactive or orchestration"))?
        }
        _ => {
            return Err(invalid_parameter(
                "a harness_config question requires a priority (text) param",
            ))
        }
    };
    let assistant_profile_ref = match param(request, "assistant_profile_ref") {
        None => None,
        Some(TypedValue::Iri(iri)) if !iri.is_empty() => Some(iri.clone()),
        Some(_) => {
            return Err(invalid_parameter(
                "assistant_profile_ref must be a non-empty iri",
            ))
        }
    };
    // Without an assistant profile the request names its model profile; with
    // one, an absent value is filled from the profile's hints by
    // `apply_profile` before any fact is read.
    let model_profile_ref = match (param(request, "model_profile_ref"), &assistant_profile_ref) {
        (None, Some(_)) => String::new(),
        _ => required_iri(request, "model_profile_ref")?,
    };
    Ok(HarnessConfigRequestBody {
        model_profile_ref,
        task_class: required_iri(request, "task_class")?,
        usable_context_budget_tokens: budget,
        priority,
        expected_trajectory_tokens: optional_tokens(request, "expected_trajectory_tokens")?,
        required_capabilities: match param(request, "required_capabilities") {
            None => BoundedVec::default(),
            Some(TypedValue::IriList(list)) => BoundedVec::new(list.iter().cloned().collect())
                .map_err(|detail| invalid_parameter(detail))?,
            Some(_) => {
                return Err(invalid_parameter(
                    "required_capabilities must be an iri_list",
                ))
            }
        },
        assistant_profile_ref,
    })
}

/// Read the assistant profile a `harness_config` request names, under the
/// rule the served profile store uses (MEDULLA-DECISION-ENGINE-R656.2): the
/// profile lives in the graph the `graph` param names, and an absent profile,
/// another tenant's and one the caller may not read all refuse with
/// `PROFILE_NOT_VISIBLE`. A request without `assistant_profile_ref` reads
/// nothing.
pub(crate) async fn resolve_profile(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    request: &DecideRequest,
) -> Result<Option<AssistantProfile>, String> {
    let Some(TypedValue::Iri(profile_id)) = param(request, "assistant_profile_ref") else {
        return Ok(None);
    };
    let graph = match param(request, "graph") {
        Some(TypedValue::Text(graph)) if !graph.is_empty() => graph,
        _ => {
            return Err(invalid_parameter(
                "an assistant_profile_ref needs the graph param naming the graph that holds the profile",
            ))
        }
    };
    #[cfg(feature = "security")]
    {
        let (core, isolation) = crate::decide::stat_support::readable_graph(
            state,
            verified.agent_id(),
            graph,
            StatisticalErrorCode::CandidatePlanRefused,
        )
        .await?;
        let tenant_scope = medulla_server_api::carrier::tenant_scope_of(verified.tenant());
        medulla_isolation::profile_access::visible_profile(
            &core,
            &isolation,
            &tenant_scope,
            verified.agent_id(),
            profile_id,
        )
        .map(Some)
        .map_err(invalid_parameter)
    }
    #[cfg(not(feature = "security"))]
    {
        let _ = (state, verified, graph, profile_id);
        Err(invalid_parameter(
            "an assistant_profile_ref needs a build with the security feature",
        ))
    }
}

/// What the assistant profile did to one request.
struct Applied {
    /// The declared tool candidates that passed the allowlist.
    options: Vec<DeclaredOption>,
    /// The declared tool candidates the allowlist removed.
    removed: Vec<String>,
    /// The allowlist removed every candidate that was declared.
    emptied: bool,
}

/// Apply the profile as constraints (MEDULLA-DECISION-ENGINE-R128.16). A
/// request value always wins over a hint: an absent `model_profile_ref` takes
/// the first preferred profile the library holds; the usable budget is the
/// minimum of the request value and `max_context_tokens`; the tool candidates
/// intersect with `tool_allowlist` ahead of the exact solve. The profile only
/// removes candidates; it adds no capability the library lacks.
fn apply_profile(
    store: &AgentLibraryStore,
    tenant_id: &str,
    body: &mut HarnessConfigRequestBody,
    declared: &[DeclaredOption],
    profile: &AssistantProfile,
) -> Result<Applied, String> {
    if body.model_profile_ref.is_empty() {
        let preferred = profile
            .model_hints
            .preferred_model_profile_refs
            .iter()
            .find(|candidate| read_profile(store, tenant_id, candidate).is_ok());
        body.model_profile_ref = preferred.cloned().ok_or_else(|| {
            invalid_parameter(format!(
                "{HARNESS_PROFILE_UNKNOWN}: no preferred model profile of assistant profile {} is in the library",
                profile.profile_id
            ))
        })?;
    }
    if let Some(max) = profile.model_hints.max_context_tokens {
        body.usable_context_budget_tokens = body.usable_context_budget_tokens.min(max);
    }
    let (options, removed): (Vec<DeclaredOption>, Vec<DeclaredOption>) =
        declared.iter().cloned().partition(|option| {
            profile
                .tool_allowlist
                .iter()
                .any(|allowed| *allowed == option.option_id)
        });
    Ok(Applied {
        emptied: options.is_empty() && !declared.is_empty(),
        removed: removed.into_iter().map(|option| option.option_id).collect(),
        options,
    })
}

/// The tenant's visible `ModelProfile` entry that `model_profile_ref` names.
fn read_profile(
    store: &AgentLibraryStore,
    tenant_id: &str,
    profile_ref: &str,
) -> Result<medulla_agent_types::agent_component::AgentComponentEntry, String> {
    let mut cursor = None;
    for _ in 0..MAX_PAGES {
        let page = store
            .search_components(&AgentComponentSearchRequest {
                tenant_id: tenant_id.to_string(),
                task: None,
                capabilities: Vec::new(),
                kinds: vec![AgentComponentKind::ModelProfile],
                read_only: false,
                limit: Some(PAGE),
                cursor: cursor.take(),
            })
            .map_err(invalid_parameter)?;
        if let Some(entry) = page
            .entries
            .into_iter()
            .find(|entry| entry.component_id == profile_ref)
        {
            return Ok(entry);
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Err(invalid_parameter(format!(
        "{HARNESS_PROFILE_UNKNOWN}: no model profile {profile_ref} is visible to the tenant"
    )))
}

fn property<'a>(
    row: &'a serde_json::Map<String, serde_json::Value>,
    local: &str,
) -> Option<&'a serde_json::Value> {
    row.get(&format!("{SWARM}{local}"))
        .or_else(|| row.get(&format!("swarm:{local}")))
        .or_else(|| row.get(local))
}

fn as_bool(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(flag) => Some(*flag),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn as_text(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

/// The facts the model profile of one question carries, read through the
/// Agent Library at its current definition.
struct ProfileFacts {
    profile_digest: String,
    context_window_tokens: u32,
    shell_proficiency: Option<MeasuredFact<ShellProficiency>>,
    stopping_behaviour: Option<MeasuredFact<StoppingBehaviour>>,
}

fn profile_facts(
    store: &AgentLibraryStore,
    tenant_id: &str,
    body: &HarnessConfigRequestBody,
) -> Result<ProfileFacts, String> {
    let entry = read_profile(store, tenant_id, &body.model_profile_ref)?;
    let AgentComponentFacts::ModelProfile {
        context_window_tokens,
        shell_proficiency,
        stopping_behaviour,
        ..
    } = entry.facts
    else {
        return Err(invalid_parameter(format!(
            "{HARNESS_PROFILE_UNKNOWN}: {} is not a model profile",
            body.model_profile_ref
        )));
    };
    Ok(ProfileFacts {
        profile_digest: entry.definition_digest,
        context_window_tokens,
        shell_proficiency,
        stopping_behaviour,
    })
}

/// Read the facts of one question: the profile through the Agent Library, the
/// task-class and profile-node facts through the tenant-filtered graph view.
/// An absent graph fact stays `None` and is never read as `false`.
pub(crate) fn read_facts(
    store: &AgentLibraryStore,
    tenant_id: &str,
    body: &HarnessConfigRequestBody,
    graph_view: Option<&GraphView>,
) -> Result<HarnessFacts, String> {
    let profile = profile_facts(store, tenant_id, body)?;
    let row_of = |iri: &str| graph_view.and_then(|view| view.node_row_object(iri));
    let shell_centric =
        medulla_agent_types::agent_ontology::shell_centric_for_task(&body.task_class, |iri| {
            row_of(iri).and_then(|row| property(&row, "shellCentric").and_then(as_bool))
        });
    let task_token_upper_bound = row_of(&body.task_class)
        .and_then(|row| property(&row, "taskTokenUpperBound").and_then(|v| v.as_u64()));
    let plan_scaffold_no_gain = row_of(&body.model_profile_ref).and_then(|row| {
        let value = property(&row, "planScaffoldNoGainAblation").and_then(as_bool)?;
        let evaluation_id = property(&row, "planScaffoldNoGainAblationRun").and_then(as_text)?;
        let digest = property(&row, "planScaffoldNoGainAblationRunDigest").and_then(as_text)?;
        Some(MeasuredFact {
            value,
            evaluation_run_ref: ObservationRef {
                evaluation_id,
                digest,
            },
        })
    });
    Ok(HarnessFacts {
        profile_digest: profile.profile_digest,
        context_window_tokens: profile.context_window_tokens,
        shell_proficiency: profile.shell_proficiency,
        stopping_behaviour: profile.stopping_behaviour,
        shell_centric,
        task_token_upper_bound,
        plan_scaffold_no_gain,
    })
}

fn selected<T>(part: &HarnessPart<T>) -> &T {
    match part {
        HarnessPart::Decided { value } => value,
        HarnessPart::Abstained {
            conservative_default,
            ..
        } => conservative_default,
    }
}

fn missing_of<T>(part: &HarnessPart<T>) -> Vec<String> {
    match part {
        HarnessPart::Decided { .. } => Vec::new(),
        HarnessPart::Abstained { missing_fact, .. } => missing_fact.as_slice().to_vec(),
    }
}

fn is_decided<T>(part: &HarnessPart<T>) -> bool {
    matches!(part, HarnessPart::Decided { .. })
}

fn wire<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

fn fraction(
    part: &HarnessPart<medulla_decision_types::decision::numeric::UnitRationalWire>,
) -> String {
    let value = selected(part);
    format!("{}/{}", value.numerator(), value.denominator())
}

/// How the tool-subset part came out before it is wrapped as a part.
enum Subset {
    Chosen(Vec<String>),
    /// Chosen by the exact solve, with the certificate it was verified under.
    Solved(Vec<String>, HarnessSolverCertificate),
    Missing(String),
}

fn bounded_ids(ids: Vec<String>) -> Result<BoundedVec<String, 256>, String> {
    BoundedVec::new(ids)
        .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))
}

/// The action-space part's tool subset (MEDULLA-DECISION-ENGINE-R128.9): the
/// exact covering solve of the `tool_subset` family, skipped when no
/// capability is required and empty when the action space is shell only.
fn solve_subset(
    options: &[DeclaredOption],
    body: &HarnessConfigRequestBody,
    action: &HarnessPart<ActionSpace>,
) -> Result<Subset, String> {
    let required: Vec<String> = body.required_capabilities.as_slice().to_vec();
    let shell_only = matches!(
        action,
        HarnessPart::Decided {
            value: ActionSpace::ShellOnly
        }
    );
    if shell_only || required.is_empty() {
        return Ok(Subset::Chosen(Vec::new()));
    }
    if options.iter().any(|option| token_cost(option).is_none()) {
        return Ok(Subset::Missing("schemaTokens".to_string()));
    }
    let request = tool_subset_body(options, required.clone(), body.usable_context_budget_tokens)?;
    match solved_covering_subset(&request) {
        Ok(solved) => Ok(Subset::Solved(
            solved.chosen,
            HarnessSolverCertificate {
                model_digest: solved.certificate.model_digest,
                branch_count: solved.certificate.branch_count,
                solver_certificate_digest: solved.solver_certificate_digest,
            },
        )),
        Err(SubsetFailure::Refused(ToolSubsetRefusal::CapabilityUncoverable)) => {
            let uncovered = required
                .iter()
                .find(|capability| {
                    !options
                        .iter()
                        .any(|option| option.classification.iter().any(|c| c == *capability))
                })
                .cloned()
                .unwrap_or_default();
            Ok(Subset::Missing(format!("coverageOf:{uncovered}")))
        }
        Err(SubsetFailure::Refused(ToolSubsetRefusal::BudgetExceeded)) => {
            Ok(Subset::Missing("contextBudget".to_string()))
        }
        Err(SubsetFailure::Solver(detail)) => Err(detail),
    }
}

/// An allowlist that removed every candidate abstains the action space: the
/// missing fact is `PROFILE_TOOL_NOT_ALLOWED` and the conservative default is
/// the tool set, which then holds no tool.
fn abstain_for_profile(
    action: HarnessPart<ActionSpace>,
) -> Result<HarnessPart<ActionSpace>, String> {
    let mut missing = missing_of(&action);
    if !missing.iter().any(|fact| fact == PROFILE_TOOL_NOT_ALLOWED) {
        missing.push(PROFILE_TOOL_NOT_ALLOWED.to_string());
    }
    Ok(HarnessPart::Abstained {
        missing_fact: BoundedVec::new(missing).map_err(|detail| invalid_parameter(detail))?,
        conservative_default: ActionSpace::ToolSet,
    })
}

fn tool_subset_part(
    subset: Subset,
    action: &HarnessPart<ActionSpace>,
) -> Result<
    (
        HarnessPart<BoundedVec<String, 256>>,
        Option<HarnessSolverCertificate>,
    ),
    String,
> {
    let mut missing = missing_of(action);
    let (subset, certificate) = match subset {
        Subset::Solved(ids, certificate) => (Subset::Chosen(ids), Some(certificate)),
        other => (other, None),
    };
    let part = match subset {
        Subset::Chosen(ids) if missing.is_empty() => HarnessPart::Decided {
            value: bounded_ids(ids)?,
        },
        Subset::Chosen(ids) => HarnessPart::Abstained {
            missing_fact: BoundedVec::new(missing).map_err(|detail| invalid_parameter(detail))?,
            conservative_default: bounded_ids(ids)?,
        },
        Subset::Missing(fact) => {
            missing.push(fact);
            HarnessPart::Abstained {
                missing_fact: BoundedVec::new(missing)
                    .map_err(|detail| invalid_parameter(detail))?,
                conservative_default: BoundedVec::default(),
            }
        }
        Subset::Solved(..) => unreachable!("a solved subset was folded into Chosen above"),
    };
    Ok((part, certificate))
}

fn premise(
    subject: &str,
    fact: String,
    class: PremiseClass,
    provenance: PremiseProvenance,
) -> FamilyPremise {
    FamilyPremise {
        subject: subject.to_string().into(),
        fact,
        class,
        source: FamilyPremiseSource::Bound(provenance),
    }
}

/// Everything one conclusion rests on, for the premise stage.
struct Basis<'a> {
    body: &'a HarnessConfigRequestBody,
    facts: &'a HarnessFacts,
    pressure_q32: Option<u64>,
    policy_digest: &'a str,
}

fn request_premise(name: &str, value: String) -> FamilyPremise {
    premise(
        "harness_config",
        format!("harness_config.{name}={value}"),
        PremiseClass::Claim,
        PremiseProvenance::Request {
            field: name.to_string(),
        },
    )
}

/// One premise per rule input, class and provenance by where the input came
/// from (MEDULLA-DECISION-ENGINE-R128.8, R128.11).
fn input_premises(basis: &Basis) -> Vec<FamilyPremise> {
    let (body, facts) = (basis.body, basis.facts);
    let mut out = vec![
        request_premise("model_profile_ref", body.model_profile_ref.clone()),
        request_premise("task_class", body.task_class.clone()),
        request_premise(
            "usable_context_budget_tokens",
            body.usable_context_budget_tokens.to_string(),
        ),
    ];
    if let Some(tokens) = body.expected_trajectory_tokens {
        out.push(request_premise(
            "expected_trajectory_tokens",
            tokens.to_string(),
        ));
    }
    out.push(premise(
        &body.model_profile_ref,
        format!(
            "harness_config.context_window_tokens={}",
            facts.context_window_tokens
        ),
        PremiseClass::Claim,
        PremiseProvenance::Publisher {
            component_id: body.model_profile_ref.clone(),
            definition_digest: facts.profile_digest.clone(),
        },
    ));
    let from_task = |name: &str, value: String| {
        premise(
            &body.task_class,
            format!("harness_config.{name}={value}"),
            PremiseClass::Claim,
            PremiseProvenance::Request {
                field: "task_class".to_string(),
            },
        )
    };
    if let Some(bound) = facts.task_token_upper_bound {
        out.push(from_task("task_token_upper_bound", bound.to_string()));
    }
    if let Some(centric) = facts.shell_centric {
        out.push(from_task("shell_centric", centric.to_string()));
    }
    if let Some(pressure) = basis.pressure_q32 {
        out.push(premise(
            "harness_config",
            format!("harness_config.pressure_q32={pressure}"),
            PremiseClass::Definition,
            PremiseProvenance::Policy {
                policy_digest: basis.policy_digest.to_string(),
            },
        ));
    }
    let measured = |name: &str, value: String, run: &str| {
        premise(
            &body.model_profile_ref,
            format!("harness_config.{name}={value}"),
            PremiseClass::Observation,
            PremiseProvenance::Observation {
                evaluation_id: run.to_string(),
            },
        )
    };
    if let Some(fact) = &facts.shell_proficiency {
        out.push(measured(
            "shell_proficiency",
            wire(&fact.value),
            &fact.evaluation_run_ref.evaluation_id,
        ));
    }
    if let Some(fact) = &facts.stopping_behaviour {
        out.push(measured(
            "stopping_behaviour",
            wire(&fact.value),
            &fact.evaluation_run_ref.evaluation_id,
        ));
    }
    if let Some(fact) = &facts.plan_scaffold_no_gain {
        out.push(measured(
            "plan_scaffold_ablation",
            fact.value.to_string(),
            &fact.evaluation_run_ref.evaluation_id,
        ));
    }
    out
}

fn why_not(policy_digest: &str, part: &str, value: &str, code: &str) -> FamilyPremise {
    premise(
        code,
        format!("harness_config.why_not:{part}:{value}"),
        PremiseClass::Definition,
        PremiseProvenance::Policy {
            policy_digest: policy_digest.to_string(),
        },
    )
}

const MISSING_FACT: &str = "MISSING_FACT";

/// One why-not premise per rejected value of each closed-vocabulary part, and
/// per unselected candidate tool, carrying the rule code
/// (MEDULLA-DECISION-ENGINE-R128.11).
fn why_not_premises(
    digest: &str,
    answer: &HarnessConfigAnswer,
    scaffold_reason: PlanScaffoldReason,
    window_covers_bound: Option<bool>,
    options: &[DeclaredOption],
) -> Vec<FamilyPremise> {
    let mut out = Vec::new();
    let mut reject = |part: &str, value: &str, code: &str| {
        out.push(why_not(digest, part, value, code));
    };
    let context = &answer.context_policy;
    let abstained = !is_decided(context);
    for rejected in [
        ContextPolicy::None,
        ContextPolicy::Elide,
        ContextPolicy::ElideThenSummarize,
    ] {
        if rejected == *selected(context) {
            continue;
        }
        let code = match (rejected, abstained, window_covers_bound) {
            (_, true, _) => MISSING_FACT,
            (ContextPolicy::None, false, None) => "TASK_BOUND_UNKNOWN",
            (ContextPolicy::None, false, Some(_)) => "TASK_BOUND_EXCEEDS_WINDOW",
            (ContextPolicy::Elide, false, _) => "ELIDE_ONLY_NOT_SELECTED",
            (ContextPolicy::ElideThenSummarize, false, _) => "TASK_BOUND_WITHIN_WINDOW",
        };
        reject("context_policy", &wire(&rejected), code);
    }
    let rejected_scaffold = match selected(&answer.plan_scaffold) {
        PlanScaffold::On => PlanScaffold::Off,
        PlanScaffold::Off => PlanScaffold::On,
    };
    let code = match scaffold_reason {
        PlanScaffoldReason::Accuracy => "STOPS_EARLY_NEEDS_SCAFFOLD",
        PlanScaffoldReason::Cost => "OVER_VERIFIES_NEEDS_SCAFFOLD",
        PlanScaffoldReason::NoGainAblation => "ABLATION_SHOWS_NO_GAIN",
        PlanScaffoldReason::RetainScaffold => "ABLATION_SHOWS_GAIN",
        PlanScaffoldReason::MissingEvidence => MISSING_FACT,
    };
    reject("plan_scaffold", &wire(&rejected_scaffold), code);
    let space = &answer.action_space;
    let (rejected_space, code) = match (selected(space), is_decided(space)) {
        (_, false) => (ActionSpace::ShellOnly, MISSING_FACT),
        (ActionSpace::ShellOnly, true) => {
            (ActionSpace::ToolSet, "SHELL_PROFICIENT_AND_SHELL_CENTRIC")
        }
        (ActionSpace::ToolSet, true) => (ActionSpace::ShellOnly, "SHELL_ONLY_CONDITIONS_NOT_MET"),
    };
    reject("action_space", &wire(&rejected_space), code);
    let chosen: Vec<&String> = selected(&answer.tool_subset).iter().collect();
    for option in options {
        if chosen.iter().any(|id| **id == option.option_id) {
            continue;
        }
        let code = match (is_decided(&answer.tool_subset), selected(space)) {
            (false, _) => MISSING_FACT,
            (true, ActionSpace::ShellOnly) => "ACTION_SPACE_SHELL_ONLY",
            (true, ActionSpace::ToolSet) => "NOT_IN_MINIMAL_COVER",
        };
        reject("tool_subset", &option.option_id, code);
    }
    out
}

fn conclude_config(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
    graph_view: Option<&GraphView>,
) -> Result<Executed, String> {
    let mut body = request_body(request)?;
    let declared: &[DeclaredOption] = match &request.candidates {
        CandidateSource::Declared { options } => options.as_slice(),
        _ => {
            return Err(invalid_parameter(
                "a harness_config question declares its own tool candidates",
            ))
        }
    };
    let reference = body.assistant_profile_ref.clone();
    let applied = match (reference, ctx.assistant_profile) {
        (None, _) => None,
        (Some(_), Some(profile)) => Some(apply_profile(
            ctx.store,
            ctx.tenant_id,
            &mut body,
            declared,
            profile,
        )?),
        (Some(reference), None) => {
            return Err(invalid_parameter(format!(
                "assistant profile {reference} was not read by the serving path"
            )))
        }
    };
    let facts = read_facts(ctx.store, ctx.tenant_id, &body, graph_view)?;
    derive_config(
        body,
        declared,
        applied.as_ref(),
        ctx.assistant_profile,
        facts,
        policy,
    )
}

/// The decision of one `harness_config` question from its typed body, the
/// declared tool options, the facts read and the pinned policy. Decide reads
/// the facts from the graph; the commit replay rebuilds them from the stored
/// premises (R128.13.2.1); both run this one derivation.
fn derive_config(
    body: HarnessConfigRequestBody,
    declared: &[DeclaredOption],
    applied: Option<&Applied>,
    assistant_profile: Option<&AssistantProfile>,
    facts: HarnessFacts,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let options: &[DeclaredOption] = applied.map_or(declared, |applied| applied.options.as_slice());
    let emptied = applied.is_some_and(|applied| applied.emptied);
    let thresholds = HarnessDefaults::from_policy(policy).map_err(invalid_parameter)?;
    let bound = facts
        .task_token_upper_bound
        .map(u32::try_from)
        .transpose()
        .map_err(|_| invalid_parameter("taskTokenUpperBound must fit a u32"))?;
    let context = decide_context_policy(
        Some(facts.context_window_tokens),
        body.expected_trajectory_tokens,
        bound,
        &thresholds,
    )
    .map_err(invalid_parameter)?;
    let threshold = |value| match &context.part {
        HarnessPart::Decided { .. } => HarnessPart::Decided { value },
        HarnessPart::Abstained { missing_fact, .. } => HarnessPart::Abstained {
            missing_fact: missing_fact.clone(),
            conservative_default: value,
        },
    };
    let scaffold = decide_plan_scaffold(
        facts.stopping_behaviour.as_ref(),
        facts.plan_scaffold_no_gain.as_ref(),
    )
    .map_err(invalid_parameter)?;
    let mut action = decide_action_space(facts.shell_proficiency.as_ref(), facts.shell_centric)
        .map_err(invalid_parameter)?;
    if emptied {
        action = abstain_for_profile(action)?;
    }
    let subset = if emptied {
        Subset::Chosen(Vec::new())
    } else {
        solve_subset(options, &body, &action)?
    };
    let (tool_subset, tool_subset_certificate) = tool_subset_part(subset, &action)?;
    let answer = HarnessConfigAnswer {
        soft_threshold: threshold(thresholds.soft_threshold),
        hard_threshold: threshold(thresholds.hard_threshold),
        recent_window_fraction: threshold(thresholds.recent_window_fraction),
        context_policy: context.part.clone(),
        plan_scaffold: scaffold.part.clone(),
        action_space: action,
        tool_subset,
    };
    answer.validate().map_err(invalid_parameter)?;
    let digest = medulla_decision_types::decision::digest::policy_digest(policy);
    let basis = Basis {
        body: &body,
        facts: &facts,
        pressure_q32: context.pressure_q32,
        policy_digest: &digest,
    };
    let mut premises = input_premises(&basis);
    if let Some(profile) = assistant_profile {
        premises.push(premise(
            &profile.profile_id,
            format!("profile:{}@{}", profile.profile_id, profile.revision),
            PremiseClass::Claim,
            PremiseProvenance::Request {
                field: "assistant_profile_ref".to_string(),
            },
        ));
    }
    let mut missing: Vec<String> = Vec::new();
    for part in [
        missing_of(&answer.context_policy),
        missing_of(&answer.soft_threshold),
        missing_of(&answer.hard_threshold),
        missing_of(&answer.recent_window_fraction),
        missing_of(&answer.plan_scaffold),
        missing_of(&answer.action_space),
        missing_of(&answer.tool_subset),
    ] {
        for fact in part {
            if !missing.contains(&fact) {
                missing.push(fact);
            }
        }
    }
    premises.extend(missing.iter().map(|fact| {
        premise(
            "harness_config",
            format!("harness_config.missing:{fact}"),
            PremiseClass::Claim,
            PremiseProvenance::Request {
                field: "params".to_string(),
            },
        )
    }));
    let window_covers_bound = bound.map(|bound| bound <= facts.context_window_tokens);
    premises.extend(why_not_premises(
        &digest,
        &answer,
        scaffold.reason,
        window_covers_bound,
        options,
    ));
    if let Some(applied) = applied {
        premises.extend(
            applied
                .removed
                .iter()
                .map(|id| why_not(&digest, "tool_subset", id, PROFILE_TOOL_NOT_ALLOWED)),
        );
    }
    let outcome = outcome_of(&body.model_profile_ref, &answer, &missing)?;
    let evidence = HarnessConfigEvaluationEvidence {
        model_profile: medulla_agent_types::agent_component::ComponentDependency {
            component_id: body.model_profile_ref.clone(),
            kind: medulla_agent_types::agent_component::AgentComponentKind::ModelProfile,
            definition_digest: facts.profile_digest.clone(),
        },
        request: body,
        answer,
        tool_subset_certificate,
        declared_options: BoundedVec::new(declared.to_vec())
            .map_err(|detail| invalid_parameter(detail))?,
        plan_scaffold_no_gain_run: facts
            .plan_scaffold_no_gain
            .as_ref()
            .map(|fact| fact.evaluation_run_ref.clone()),
    };
    Ok(Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        family: Some(FamilyExecuted {
            answer: Some(FamilyAnswer::HarnessConfig(Box::new(evidence))),
            premises: Vec::new(),
            resolution: None,
            evidence: None,
        }),
        family_premises: premises,
        route: None,
    })
}

/// One advisory option per decided part, `<part>:<value>`; when every part
/// abstains, an abstention naming each missing fact (R128.12).
fn outcome_of(
    profile: &str,
    answer: &HarnessConfigAnswer,
    missing: &[String],
) -> Result<StatisticalOutcome, String> {
    let ids = selected(&answer.tool_subset).as_slice().join(",");
    let parts: [(&str, bool, String); 7] = [
        (
            "context_policy",
            is_decided(&answer.context_policy),
            wire(selected(&answer.context_policy)),
        ),
        (
            "soft_threshold",
            is_decided(&answer.soft_threshold),
            fraction(&answer.soft_threshold),
        ),
        (
            "hard_threshold",
            is_decided(&answer.hard_threshold),
            fraction(&answer.hard_threshold),
        ),
        (
            "recent_window_fraction",
            is_decided(&answer.recent_window_fraction),
            fraction(&answer.recent_window_fraction),
        ),
        (
            "plan_scaffold",
            is_decided(&answer.plan_scaffold),
            wire(selected(&answer.plan_scaffold)),
        ),
        (
            "action_space",
            is_decided(&answer.action_space),
            wire(selected(&answer.action_space)),
        ),
        (
            "tool_subset",
            is_decided(&answer.tool_subset),
            format!("[{ids}]"),
        ),
    ];
    if parts.iter().all(|(_, decided, _)| !decided) {
        let mut reasons: Vec<AbstainReason> = missing
            .iter()
            .map(|fact| AbstainReason::UnknownFact {
                component_id: profile.to_string(),
                field: fact.clone(),
            })
            .collect();
        reasons.truncate(64);
        return Ok(StatisticalOutcome::Abstained {
            reasons: BoundedVec::new(reasons).map_err(|detail| invalid_parameter(detail))?,
        });
    }
    let scores: Vec<ScoredOption> = parts
        .into_iter()
        .filter(|(_, decided, _)| *decided)
        .map(|(part, _, value)| ScoredOption {
            option_id: format!("{part}:{value}"),
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: DECIDED_SCORE,
            },
            probability: None,
        })
        .collect();
    Ok(StatisticalOutcome::Advisory {
        scores: BoundedVec::new(scores)
            .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?,
        calibrated: false,
    })
}

fn mismatch(detail: impl std::fmt::Display) -> String {
    refusal(
        StatisticalErrorCode::ParameterInvalid,
        format!("{HARNESS_REPLAY_MISMATCH}: {detail}"),
    )
}

/// The one stored premise `harness_config.<name>=` of `subject`, parsed. An
/// absent premise is an absent fact; a repeated or unparsable one is a
/// mismatch.
fn stored_fact<T>(
    record: &StatisticalDecisionRecord,
    subject: &str,
    name: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<(T, PremiseProvenance)>, String> {
    let prefix = format!("harness_config.{name}=");
    let mut found = record
        .premises
        .iter()
        .filter(|premise| premise.subject == subject && premise.fact.starts_with(&prefix));
    let Some(premise) = found.next() else {
        return Ok(None);
    };
    if found.next().is_some() {
        return Err(mismatch(format!("the {name} premise is stored twice")));
    }
    let value = parse(&premise.fact[prefix.len()..])
        .ok_or_else(|| mismatch(format!("the {name} premise does not parse")))?;
    Ok(Some((value, premise.provenance.clone())))
}

/// The task and run facts the stored premises carry, the profile facts read
/// again through the Agent Library at the pinned definition.
fn replayed_facts(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
    evidence: &HarnessConfigEvaluationEvidence,
) -> Result<HarnessFacts, String> {
    let body = &evidence.request;
    let profile = profile_facts(ctx.store, ctx.tenant_id, body)?;
    if profile.profile_digest != evidence.model_profile.definition_digest
        || evidence.model_profile.component_id != body.model_profile_ref
    {
        return Err(mismatch(
            "the model profile no longer has the recorded definition",
        ));
    }
    let task = body.task_class.as_str();
    let model = body.model_profile_ref.as_str();
    let task_token_upper_bound =
        stored_fact(record, task, "task_token_upper_bound", |v| v.parse().ok())?.map(|(v, _)| v);
    let shell_centric =
        stored_fact(record, task, "shell_centric", |v| v.parse().ok())?.map(|(v, _)| v);
    let plan_scaffold_no_gain =
        stored_fact(record, model, "plan_scaffold_ablation", |v| v.parse().ok())?
            .map(|(value, provenance)| match provenance {
                PremiseProvenance::Observation { evaluation_id } => {
                    // The premise names the run's id; its digest rides the evidence.
                    let digest = evidence
                        .plan_scaffold_no_gain_run
                        .as_ref()
                        .filter(|run| run.evaluation_id == evaluation_id)
                        .map(|run| run.digest.clone())
                        .ok_or_else(|| {
                            mismatch("the plan_scaffold_ablation run is not in the evidence")
                        })?;
                    Ok(MeasuredFact {
                        value,
                        evaluation_run_ref: ObservationRef {
                            evaluation_id,
                            digest,
                        },
                    })
                }
                _ => Err(mismatch(
                    "the plan_scaffold_ablation premise is not an observation",
                )),
            })
            .transpose()?;
    Ok(HarnessFacts {
        profile_digest: profile.profile_digest,
        context_window_tokens: profile.context_window_tokens,
        shell_proficiency: profile.shell_proficiency,
        stopping_behaviour: profile.stopping_behaviour,
        shell_centric,
        task_token_upper_bound,
        plan_scaffold_no_gain,
    })
}

/// Re-derive a sealed `harness_config` record (MEDULLA-DECISION-ENGINE-R128.13.2.1):
/// the declared options must be the ones the record digests, the profile pin
/// and the policy must still resolve, and running the family derivation on the
/// stored task facts must give the stored evidence, outcome and premises.
pub(crate) fn replay(
    ctx: &ExecutionContext,
    record: &StatisticalDecisionRecord,
    policy: &DecisionPolicy,
) -> Result<(), String> {
    let evidence = record
        .inputs
        .harness_evaluation
        .as_ref()
        .ok_or_else(|| mismatch("the record carries no harness evidence"))?;
    let frame = serde_json::to_vec(evidence)
        .map_err(|error| mismatch(format!("the evidence does not encode: {error}")))?;
    HarnessEvidenceLimits::from_policy(policy)
        .check_frame(frame.len())
        .and_then(|()| HarnessEvidenceLimits::from_policy(policy).check_evidence(evidence))
        .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    HarnessReplayLimits::from_policy(policy)
        .check(record.premises.len(), evidence, frame.len())
        .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    let options = evidence.declared_options.as_slice();
    let digest = medulla_query_types::solve_scalar::digest_text(DECLARED_OPTIONS_DOMAIN, &options);
    match &record.candidate_source {
        CandidateSourceRecord::Declared { options_digest } if *options_digest == digest => {}
        _ => {
            return Err(mismatch(
                "the evidence options are not the record's declared candidates",
            ))
        }
    }
    let request = DecideRequest {
        tenant_id: record.tenant_id.clone(),
        question: record.question.clone(),
        candidates: CandidateSource::Declared {
            options: evidence.declared_options.clone(),
        },
        feature_schema: record.inputs.feature_schema.clone(),
        head: record.inputs.head.clone(),
        policy: record.inputs.policy.clone(),
        params: record.inputs.params.clone(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: record.inputs.evidence_streams.clone(),
    };
    let mut body = request_body(&request)?;
    let applied = match (body.assistant_profile_ref.is_some(), ctx.assistant_profile) {
        (false, _) => None,
        (true, Some(profile)) => Some(apply_profile(
            ctx.store,
            ctx.tenant_id,
            &mut body,
            options,
            profile,
        )?),
        (true, None) => {
            return Err(mismatch(
                "the record names an assistant profile the replay did not read",
            ))
        }
    };
    if body != evidence.request {
        return Err(mismatch(
            "the evidence request is not the record's stored params",
        ));
    }
    let facts = replayed_facts(ctx, record, evidence)?;
    let executed = derive_config(
        body,
        options,
        applied.as_ref(),
        ctx.assistant_profile,
        facts,
        policy,
    )?;
    let family = executed
        .family
        .as_ref()
        .ok_or_else(|| mismatch("the derivation produced no harness configuration"))?;
    let derived = family
        .answer
        .as_ref()
        .and_then(FamilyAnswer::harness_config)
        .ok_or_else(|| mismatch("the derivation produced no harness configuration"))?;
    if derived != evidence {
        return Err(mismatch(
            "re-deriving the configuration gives another answer than the stored evidence",
        ));
    }
    if executed.ladder.outcome != record.outcome {
        return Err(mismatch(
            "re-deriving the configuration gives another outcome",
        ));
    }
    let policy_digest = medulla_decision_types::decision::digest::policy_digest(policy);
    let derived_premises: Vec<PremiseRef> = executed
        .family_premises
        .iter()
        .map(|premise| premise.seal(&policy_digest))
        .collect();
    let is_ours = |premise: &PremiseRef| premise.fact.starts_with("harness_config.");
    let stored_all_derived = record
        .premises
        .iter()
        .filter(|premise| is_ours(premise))
        .all(|premise| derived_premises.contains(premise));
    let derived_all_stored = derived_premises
        .iter()
        .all(|premise| record.premises.iter().any(|stored| stored == premise));
    if !(stored_all_derived && derived_all_stored) {
        return Err(mismatch(
            "the stored premises are not the ones the facts derive",
        ));
    }
    Ok(())
}
