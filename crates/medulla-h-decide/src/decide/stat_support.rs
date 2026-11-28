//! What the statistical handlers share: reading a pinned catalog component's
//! typed body, resolving the decision policy, and the engine's default
//! statistical levels.
//!
//! A pin is checked in full before a byte of its body is trusted: the
//! component must exist in the caller's tenant, be of the pinned kind, and one
//! of its retained revisions must carry the pinned definition digest. The body
//! is then re-hashed against the revision's content digest.

use medulla_decision_stats::decision::features::FeatureMatrix;
use serde::de::DeserializeOwned;
use std::sync::Arc;

use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_agent_types::{
    agent_component::AgentComponentEntry, agent_component::AgentComponentKind,
    agent_component::ComponentDependency,
};
use medulla_decision_program_types::decision::statistical::body::decode_body;
use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
use {
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_statistical_types::decision::StatisticalPolicy,
};

use medulla_persistence_records::agent_library::AgentLibraryStore;

/// Open `graph` for one caller and return its analysis snapshot filtered by
/// that caller's row-level isolation, before any statistical read.
pub(super) async fn filtered_view(
    state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    graph: &str,
) -> Result<medulla_graph_core::graph::GraphView, String> {
    filtered_view_versioned(state, agent_id, graph)
        .await
        .map(|(view, _)| view)
}

/// [`filtered_view`] with the graph's write version read just before the
/// snapshot, so the version never claims a write the rows do not reflect.
pub(super) async fn filtered_view_versioned(
    state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    graph: &str,
) -> Result<(medulla_graph_core::graph::GraphView, u64), String> {
    let (core, isolation) = readable_graph(
        state,
        agent_id,
        graph,
        StatisticalErrorCode::CandidatePlanRefused,
    )
    .await?;
    #[cfg(not(feature = "security"))]
    let _ = isolation;
    let version = core.version();
    let view = core.analysis_snapshot();
    #[cfg(feature = "security")]
    let view = {
        let mut view = view;
        isolation.filter_view(agent_id, &mut view);
        view
    };
    Ok((view, version))
}

/// Each graph a record read rows from, as it stood at the version each row was
/// read at (MEDULLA-DECISION-ENGINE-R121.2). A node's image comes from the
/// graph's retained history at that version, not from the live map; a node that
/// did not exist then, or that the caller cannot see in the image it had then,
/// is absent from the returned view. A version the history no longer reaches
/// refuses `GRAPH_VERSION_RETIRED`; one the graph never reached refuses
/// `DECISION_REPLAY_MISMATCH`.
pub(super) async fn historical_views(
    state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    rows: &[medulla_decision_program_types::decision::statistical::graph_rows::GraphRowProvenance],
) -> Result<super::stat_replay::GraphSnapshots, String> {
    use medulla_graph_core::graph::NodeHistoryError;
    let mut wanted: std::collections::BTreeMap<(String, u64), Vec<&str>> =
        std::collections::BTreeMap::new();
    for row in rows {
        wanted
            .entry((row.graph.clone(), row.graph_version))
            .or_default()
            .push(row.node_id.as_str());
    }
    let mut views = super::stat_replay::GraphSnapshots::new();
    for ((graph, version), ids) in wanted {
        let (core, isolation) = readable_graph(
            state,
            agent_id,
            &graph,
            StatisticalErrorCode::CandidatePlanRefused,
        )
        .await?;
        #[cfg(not(feature = "security"))]
        let _ = isolation;
        let mut view = core.analysis_snapshot();
        for id in ids {
            let image = core
                .node_properties_at(id, version)
                .map_err(|error| match error {
                    NodeHistoryError::Retired { .. } => {
                        refusal(StatisticalErrorCode::GraphVersionRetired, error.to_string())
                    }
                    NodeHistoryError::Future { .. } => format!(
                        "{}: {error}",
                        medulla_decision_statistical_types::decision::DecisionErrorCode::DecisionReplayMismatch
                            .as_str()
                    ),
                })?;
            match image {
                Some(blob) => {
                    view.node_properties.insert(id.to_string(), blob);
                }
                None => {
                    view.node_properties.remove(id);
                }
            }
            // The warm visibility index describes the live image; drop the entry so
            // visibility is derived from the image at the recorded version.
            #[cfg(feature = "security")]
            {
                view.visibility_index.remove(id);
                if view.node_properties.contains_key(id)
                    && !isolation.can_see_node(agent_id, &view, id)
                {
                    view.node_properties.remove(id);
                }
            }
        }
        views.insert((graph, version), view);
    }
    Ok(views)
}

/// Open a graph for one caller before exposing its data to a statistical read.
pub(super) async fn readable_graph(
    state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    graph: &str,
    missing_code: StatisticalErrorCode,
) -> Result<
    (
        Arc<medulla_graph_core::graph::GraphCore>,
        medulla_isolation::isolation::IsolationLayer,
    ),
    String,
> {
    graph_at_level(
        state,
        agent_id,
        graph,
        missing_code,
        medulla_isolation::isolation::AccessLevel::Read,
    )
    .await
}

/// `graph`, with `level` access checked for `agent_id`.
pub(super) async fn graph_at_level(
    state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    graph: &str,
    missing_code: StatisticalErrorCode,
    level: medulla_isolation::isolation::AccessLevel,
) -> Result<
    (
        Arc<medulla_graph_core::graph::GraphCore>,
        medulla_isolation::isolation::IsolationLayer,
    ),
    String,
> {
    // After a restart a durable graph is cold until a request opens it.
    medulla_server_api::api::reentry::ensure_graph_resident(state, graph).await;
    let guard = state.read().await;
    let entry = guard
        .registry
        .get(graph)
        .ok_or_else(|| refusal(missing_code, format!("unknown graph {graph}")))?;
    medulla_server_auth::access::check_graph_access(
        &guard.isolation,
        Some(agent_id),
        graph,
        entry.graph_type,
        entry.owner.as_deref(),
        entry.importing.as_deref(),
        level,
    )?;
    Ok((Arc::clone(&entry.core), guard.isolation.clone()))
}

/// Copy a pinned inline matrix into the numeric decision kernel's row format.
pub(super) fn materialize_matrix(
    candidate_ids: &[String],
    feature_names: &[String],
    values: &[i64],
) -> FeatureMatrix {
    FeatureMatrix {
        candidate_ids: candidate_ids.to_vec(),
        feature_names: feature_names.to_vec(),
        values: values.to_vec(),
    }
}

/// The refusal text of a statistical-surface failure.
pub(super) fn refusal(code: StatisticalErrorCode, detail: impl std::fmt::Display) -> String {
    code.refusal(detail)
}

pub(super) fn invalid_parameter(detail: impl std::fmt::Display) -> String {
    refusal(StatisticalErrorCode::ParameterInvalid, detail)
}

/// The revision a pin names, checked for tenant, kind and digest.
pub(super) fn pinned_entry(
    store: &AgentLibraryStore,
    tenant_id: &str,
    pin: &ComponentDependency,
    kind: AgentComponentKind,
) -> Result<AgentComponentEntry, String> {
    let mismatch = |detail: &str| {
        refusal(
            StatisticalErrorCode::ComponentPinMismatch,
            format!("{} {detail}", pin.component_id),
        )
    };
    if pin.kind != kind {
        return Err(mismatch("is pinned as the wrong kind"));
    }
    store
        .component_revisions(tenant_id, &pin.component_id)?
        .into_iter()
        .rev()
        .find(|entry| entry.definition_digest == pin.definition_digest)
        .filter(|entry| entry.kind == kind)
        .ok_or_else(|| mismatch("has no revision at the pinned definition digest"))
}

/// Decode the typed body of a pinned component.
pub(super) fn pinned_body<T: DeserializeOwned>(
    store: &AgentLibraryStore,
    tenant_id: &str,
    pin: &ComponentDependency,
    kind: AgentComponentKind,
    code: StatisticalErrorCode,
) -> Result<(T, String), String> {
    let entry = pinned_entry(store, tenant_id, pin, kind)?;
    let body = decode_body(&entry.content_digest, &entry.attributes)
        .map_err(|detail| refusal(code, detail))?;
    Ok((body, entry.content_digest))
}

/// The engine's default statistical levels, defined by the decision-policy
/// seam; the statistical handlers read the levels through this name.
pub(super) fn default_statistical_policy() -> StatisticalPolicy {
    medulla_decision_policy::policy::default_statistical_policy()
}

/// A resolved policy: the body, its digest and its statistical half.
pub(super) struct ResolvedPolicy {
    pub(super) policy: DecisionPolicy,
    pub(super) digest: String,
    pub(super) statistical: StatisticalPolicy,
}

/// One read handle over the agent library, for the decision-policy seam.
struct PolicyLibraryHandle<'a>(&'a AgentLibraryStore);

impl<'a> medulla_decision_policy::policy::PolicyLibrary for PolicyLibraryHandle<'a> {
    /// The retained revisions of `component_id` in `tenant_id`, newest first.
    fn policy_revisions(
        &self,
        tenant_id: &str,
        component_id: &str,
    ) -> Result<Vec<AgentComponentEntry>, String> {
        self.0
            .component_revisions(tenant_id, component_id)
            .map_err(|error| error.to_string())
    }

    fn champion_policy_target(
        &self,
        tenant_id: &str,
        policy_id: &str,
    ) -> Result<Option<String>, String> {
        self.0.champion_policy_target(tenant_id, policy_id)
    }
}

/// Resolve `reference` for `tenant_id` through the decision-policy seam.
pub(super) fn resolve_policy(
    store: &AgentLibraryStore,
    tenant_id: &str,
    reference: &DecisionPolicyRef,
) -> Result<ResolvedPolicy, String> {
    let resolved = medulla_decision_policy::policy::resolve_policy(
        &PolicyLibraryHandle(store),
        tenant_id,
        reference,
    )?;
    Ok(ResolvedPolicy {
        policy: resolved.policy,
        digest: resolved.digest,
        statistical: resolved.statistical,
    })
}

/// The `(source, target)` pairs of the view's edges named `relationship`, sorted.
pub(super) fn edges_named(
    view: &medulla_graph_core::graph::GraphView,
    relationship: &str,
) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = view
        .edge_properties
        .iter()
        .filter(|(_, blobs)| {
            blobs.iter().any(|blob| {
                medulla_types::msgpack::decode_property_value(blob.as_slice())
                    .ok()
                    .and_then(|edge| {
                        edge.get("relationship")
                            .and_then(serde_json::Value::as_str)
                            .map(|name| name == relationship)
                    })
                    .unwrap_or(false)
            })
        })
        .map(|((source, target), _)| (source.clone(), target.clone()))
        .collect();
    pairs.sort();
    pairs
}
