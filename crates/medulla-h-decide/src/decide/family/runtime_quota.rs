//! The `runtime_quota` question family (MEDULLA-DECISION-ENGINE-R133.3,
//! R133.4, R133.5).
//!
//! Answers the quota one run gets. The defaults are the `quota.*` knobs, or the
//! overrides the run's task class holds in the caller's tenant-filtered graph
//! (clamped to the ceiling). A request value above its ceiling refuses before
//! the task class is looked up; otherwise a request only tightens. `Decide` is
//! evaluate-only and writes nothing.

use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::run_governance::{
    decide_runtime_quota, knob_quotas, QuotaField, QuotaRefusal, QuotaRequest, RuntimeQuota,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, TypedValue};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::filtered_view;

use super::support::{advisory, invalid, param, single_option, text_param, unsigned_param};
use super::DecisionFamily;

/// The one option a runtime quota question declares.
pub const QUOTA_OPTION: &str = "quota";
const PARAM_TASK_CLASS: &str = "task_class";
const PARAM_GRAPH: &str = "graph";

/// The family of the `runtime_quota` question kind.
pub struct RuntimeQuotaFamily;

/// The request parameter naming `field`'s requested value.
fn request_param(field: QuotaField) -> &'static str {
    match field {
        QuotaField::WallTimeMs => "requested_wall_time_ms",
        QuotaField::MemoryBytes => "requested_memory_bytes",
        QuotaField::ProcessCount => "requested_process_count",
        QuotaField::GraceMs => "requested_grace_ms",
    }
}

/// The task-class node property holding `field`'s default override.
fn override_property(field: QuotaField) -> &'static str {
    match field {
        QuotaField::WallTimeMs => "defaultWallTimeMs",
        QuotaField::MemoryBytes => "defaultMemoryBytes",
        QuotaField::ProcessCount => "defaultProcessCount",
        QuotaField::GraceMs => "defaultGraceMs",
    }
}

/// The values the request asks for.
fn requested(request: &DecideRequest) -> Result<QuotaRequest, String> {
    Ok(QuotaRequest {
        wall_time_ms: unsigned_param(request, request_param(QuotaField::WallTimeMs))?,
        memory_bytes: unsigned_param(request, request_param(QuotaField::MemoryBytes))?,
        process_count: unsigned_param(request, request_param(QuotaField::ProcessCount))?,
        grace_ms: unsigned_param(request, request_param(QuotaField::GraceMs))?,
    })
}

/// The task class the request names, if any.
fn task_class(request: &DecideRequest) -> Result<Option<&str>, String> {
    match param(request, PARAM_TASK_CLASS)? {
        None => Ok(None),
        Some(TypedValue::Iri(iri)) => Ok(Some(iri.as_str())),
        Some(_) => Err(invalid("parameter task_class is not an IRI")),
    }
}

/// A quota refusal as the wire error: `PARAMETER_INVALID` naming the registered
/// code and the field. It carries no quota.
fn refused(refusal: QuotaRefusal) -> String {
    invalid(format!("{}: {}", refusal.code().as_str(), refusal.detail()))
}

/// Refuse a request value above its ceiling. The ceilings are `Fixed` knobs, so
/// the engine default policy resolves them for every caller.
fn check_ceilings(request: &DecideRequest) -> Result<(), String> {
    let (defaults, ceilings) = knob_quotas(&DecisionPolicy::engine_default());
    decide_runtime_quota(&defaults, &ceilings, &requested(request)?)
        .map(|_| ())
        .map_err(refused)
}

/// Open the graph a quota question names for its task class, through the
/// caller's row-level filter. A question without a task class reads nothing;
/// a request above a ceiling is refused before any graph is opened.
pub(in crate::decide) async fn quota_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<GraphView>, String> {
    check_ceilings(request)?;
    if task_class(request)?.is_none() {
        return Ok(None);
    }
    let graph = text_param(request, PARAM_GRAPH)?
        .filter(|graph| !graph.trim().is_empty())
        .ok_or_else(|| invalid("a runtime_quota question with a task_class requires a graph"))?;
    filtered_view(state, agent_id, graph).await.map(Some)
}

/// Where a field's default came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DefaultSource {
    Knob,
    TaskClass,
}

impl DefaultSource {
    fn name(self) -> &'static str {
        match self {
            Self::Knob => "knob",
            Self::TaskClass => "task_class",
        }
    }
}

/// The task class's overrides, clamped to the ceiling, over the knob defaults.
/// The fourth value is whether the task class node was found; when it is not
/// (absent, not in the view, or another tenant's) the knob defaults answer.
fn defaults_with_overrides(
    tenant: &str,
    policy: &DecisionPolicy,
    view: Option<&GraphView>,
    class: Option<&str>,
) -> (RuntimeQuota, RuntimeQuota, [DefaultSource; 4], bool) {
    let (mut defaults, ceilings) = knob_quotas(policy);
    let mut sources = [DefaultSource::Knob; 4];
    let props = view
        .zip(class)
        .and_then(|(view, class)| view.node_row_object(class))
        // The view is already the caller's row-level filtered snapshot; a node
        // that names another tenant is never read on top of that.
        .filter(|props| {
            props
                .get("tenant_id")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|owner| owner == tenant)
        });
    let found = props.is_some();
    if let Some(props) = props {
        for (slot, field) in QuotaField::ALL.into_iter().enumerate() {
            if let Some(value) = props
                .get(override_property(field))
                .and_then(serde_json::Value::as_u64)
            {
                let clamped = value.min(ceilings.get(field));
                match field {
                    QuotaField::WallTimeMs => defaults.wall_time_ms = clamped,
                    QuotaField::MemoryBytes => defaults.memory_bytes = clamped,
                    QuotaField::ProcessCount => defaults.process_count = clamped,
                    QuotaField::GraceMs => defaults.grace_ms = clamped,
                }
                sources[slot] = DefaultSource::TaskClass;
            }
        }
    }
    (defaults, ceilings, sources, found)
}

fn decided(
    tenant: &str,
    request: &DecideRequest,
    policy: &DecisionPolicy,
    view: Option<&GraphView>,
) -> Result<(RuntimeQuota, Vec<FamilyPremise>), String> {
    let asked = requested(request)?;
    let class = task_class(request)?;
    let (defaults, ceilings, sources, found) = defaults_with_overrides(tenant, policy, view, class);
    let quota = decide_runtime_quota(&defaults, &ceilings, &asked).map_err(refused)?;
    let premise = |fact: String, class: PremiseClass, source: FamilyPremiseSource| FamilyPremise {
        subject: "runtime_quota",
        fact,
        class,
        source,
    };
    let mut premises = Vec::new();
    for (slot, field) in QuotaField::ALL.into_iter().enumerate() {
        let name = field.name();
        premises.push(premise(
            format!("runtime_quota.{name}={}", quota.get(field)),
            PremiseClass::Definition,
            FamilyPremiseSource::Policy,
        ));
        if let Some(value) = asked.get(field) {
            premises.push(premise(
                format!("runtime_quota.requested_{name}={value}"),
                PremiseClass::Claim,
                FamilyPremiseSource::Request(request_param(field)),
            ));
        }
        let source = sources[slot];
        premises.push(match (source, class) {
            (DefaultSource::TaskClass, Some(class)) => premise(
                format!("runtime_quota.default_source:{name}={}", source.name()),
                PremiseClass::Observation,
                FamilyPremiseSource::Observation(class.to_string()),
            ),
            _ => premise(
                format!("runtime_quota.default_source:{name}={}", source.name()),
                PremiseClass::Definition,
                FamilyPremiseSource::Policy,
            ),
        });
    }
    if !found {
        premises.push(premise(
            "runtime_quota.missing:task_class".to_string(),
            PremiseClass::Claim,
            FamilyPremiseSource::Request(PARAM_TASK_CLASS),
        ));
    }
    Ok((quota, premises))
}

impl DecisionFamily for RuntimeQuotaFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, QUOTA_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        requested(request)?;
        task_class(request)?;
        check_ceilings(request)
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(
            decided(ctx.tenant_id, request, policy, graph_view).and_then(|(quota, premises)| {
                let mut executed = advisory(vec![
                    format!("wall_time_ms:{}", quota.wall_time_ms),
                    format!("memory_bytes:{}", quota.memory_bytes),
                    format!("process_count:{}", quota.process_count),
                    format!("grace_ms:{}", quota.grace_ms),
                ])?;
                executed.family_premises = premises;
                Ok(executed)
            }),
        )
    }

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
}
