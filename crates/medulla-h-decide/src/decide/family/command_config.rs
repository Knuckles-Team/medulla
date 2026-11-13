//! The `command_config` question family (MEDULLA-DECISION-ENGINE-R148.3).
//!
//! A command's settings live in the graph as `CommandConfigSchema` and
//! `CommandMode` nodes (the node's `type` property is its label). The caller
//! names the graph in the `graph` param; the view is opened through the graph
//! ACL and the row-level filter before any node is read, and a node that names
//! another tenant in `tenant_id` is never read. The family picks the mode with
//! `select_mode` (the requested `mode_id`, else the `mode.default_mode_id`
//! ordinal), builds its config against the schema with `build_config` and
//! answers an advisory with `mode:<id>` and `config:<digest>`. The full
//! `ModeDecision` is kept in a premise. `Decide` is evaluate-only and writes
//! nothing.

use medulla_decision_types::decision::command_config::{
    build_config, select_mode, CommandConfigSchema, CommandMode, ModeDecision, ModeRefusal,
    COMMAND_CONFIG_SCHEMA_VERSION, KNOBS,
};
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::Digest256;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::filtered_view;

use super::support::{abstained, advisory, int_param, invalid, single_option, text_param};
use super::DecisionFamily;

/// The one declared option: the mode decision.
pub const MODE_OPTION: &str = "mode";
const PARAM_GRAPH: &str = "graph";
const MODE_LABEL: &str = "CommandMode";
const SCHEMA_LABEL: &str = "CommandConfigSchema";

/// The family of the `command_config` question kind.
pub struct CommandConfigFamily;

/// Open the graph a command-config question names before any read, through
/// the caller's row-level filter.
pub(in crate::decide) async fn command_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<GraphView, String> {
    let graph = text_param(request, PARAM_GRAPH)?
        .filter(|graph| !graph.trim().is_empty())
        .ok_or_else(|| invalid("a command_config question requires a graph param"))?;
    filtered_view(state, agent_id, graph).await
}

/// The nodes of `label` the tenant may read, decoded as `T`: a node naming
/// another tenant is skipped, and so is a node that does not hold the record's
/// shape. Ordered by node id.
fn nodes_of<T: serde::de::DeserializeOwned>(
    view: &GraphView,
    label: &str,
    tenant: &str,
) -> Vec<(String, T)> {
    let mut found: Vec<(String, T)> = view
        .node_properties
        .keys()
        .filter_map(|id| {
            let mut row = view.node_row_object(id)?;
            if row.get("type").and_then(serde_json::Value::as_str) != Some(label) {
                return None;
            }
            row.remove("type");
            let owner = row.remove("tenant_id");
            if owner
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .is_some_and(|owner| owner != tenant)
            {
                return None;
            }
            let record = serde_json::from_value(serde_json::Value::Object(row)).ok()?;
            Some((id.clone(), record))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn observation(subject: &'static str, fact: String, node_id: &str) -> FamilyPremise {
    FamilyPremise {
        subject,
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(node_id.to_string()),
    }
}

fn conclude_mode(
    request: &DecideRequest,
    tenant: &str,
    policy: &DecisionPolicy,
    view: &GraphView,
) -> Result<Executed, String> {
    let requested = text_param(request, "mode_id")?;
    let expected_revision = int_param(request, "expected_mode_revision")?;
    let expected_digest = text_param(request, "expected_config_digest")?;
    let default_ordinal = match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(value) => u32::try_from(value).unwrap_or(0),
        _ => 0,
    };

    let modes = nodes_of::<CommandMode>(view, MODE_LABEL, tenant);
    let table: Vec<CommandMode> = modes.iter().map(|(_, mode)| mode.clone()).collect();
    let refuse = |refusal: ModeRefusal| Ok(abstained(QuestionKind::CommandConfig, &refusal));
    let mode = match select_mode(&table, requested, default_ordinal) {
        Ok(mode) => mode,
        Err(refusal) => return refuse(refusal),
    };
    let mode_node = modes
        .iter()
        .find(|(_, held)| held.mode_id == mode.mode_id)
        .map(|(id, _)| id.clone())
        .unwrap_or_default();
    if let Some(expected) = expected_revision {
        if u64::try_from(expected).ok() != Some(mode.mode_revision) {
            return refuse(ModeRefusal::ModeUnknown {
                detail: format!(
                    "mode {} is at revision {}, not the expected {expected}",
                    mode.mode_id, mode.mode_revision
                ),
            });
        }
    }

    // The newest revision of the mode's schema.
    let schemas = nodes_of::<CommandConfigSchema>(view, SCHEMA_LABEL, tenant);
    let Some((schema_node, schema)) = schemas
        .iter()
        .filter(|(_, schema)| schema.schema_id == mode.schema_id)
        .max_by_key(|(_, schema)| schema.revision)
    else {
        return refuse(ModeRefusal::ConfigInvalid {
            detail: format!(
                "mode {} names schema {}, which the graph does not hold",
                mode.mode_id, mode.schema_id
            ),
        });
    };
    let config = match build_config(mode, schema) {
        Ok(config) => config,
        Err(refusal) => return refuse(refusal),
    };
    let digest = config.config_digest.to_hex();
    if let Some(expected) = expected_digest {
        if expected != digest {
            return refuse(ModeRefusal::ConfigInvalid {
                detail: format!("config digest {digest} is not the expected {expected}"),
            });
        }
    }
    let decision = ModeDecision {
        schema_version: COMMAND_CONFIG_SCHEMA_VERSION,
        mode_id: mode.mode_id.clone(),
        mode_revision: mode.mode_revision,
        config,
        schema_digest: Digest256::sha256(&serde_json::to_vec(schema).map_err(|e| e.to_string())?),
    };
    let decision_json = serde_json::to_string(&decision).map_err(|e| e.to_string())?;

    let mut executed = advisory(vec![
        format!("mode:{}", decision.mode_id),
        format!("config:{digest}"),
    ])?;
    executed.family_premises = vec![
        observation(
            "command_mode",
            format!(
                "command_config:mode={}@{}",
                decision.mode_id, decision.mode_revision
            ),
            &mode_node,
        ),
        observation(
            "command_schema",
            format!(
                "command_config:schema={}@{}",
                schema.schema_id, schema.revision
            ),
            schema_node,
        ),
        observation(
            "command_decision",
            format!("command_config:decision={decision_json}"),
            &mode_node,
        ),
    ];
    Ok(executed)
}

impl DecisionFamily for CommandConfigFamily {
    fn validate_candidates(
        &self,
        options: &[medulla_decision_types::decision::statistical::declared::DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, MODE_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        text_param(request, "mode_id")?;
        int_param(request, "expected_mode_revision")?;
        text_param(request, "expected_config_digest")?;
        Ok(())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        let Some(view) = graph_view else {
            return Some(Err(invalid(
                "a command_config question reads the graph its graph param names",
            )));
        };
        Some(conclude_mode(request, ctx.tenant_id, policy, view))
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

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}
