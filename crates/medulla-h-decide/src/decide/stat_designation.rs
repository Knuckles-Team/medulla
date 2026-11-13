//! The `CapabilityDesignation` question (MEDULLA-DECISION-ENGINE-R172.7): the
//! request's one typed parameter, and the server-side read that loads the
//! resident index handle and the schema closure the family concludes over.

use std::sync::Arc;

use medulla_decision_memory::memory::capability_index::{CapabilityIndex, ShardLimits};
use medulla_decision_types::decision::capability_designation::{
    DesignationAsk, DesignationKnobs, SchemaProjection, DESIGNATION_PARAM,
};
use medulla_decision_types::decision::statistical::retrieval_adapter::AdapterScope;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionSafety, TypedValue,
};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::{ErasedHandle, ServerState};

use super::stat_support::invalid_parameter;
use super::SharedState;

/// What one designation question reads beside the Agent Library.
#[derive(Clone)]
pub(super) struct DesignationRuntime {
    /// The resident index every designation question and the feed share.
    pub(super) index: Arc<CapabilityIndex>,
    /// The subclass closure at `schema_digest`.
    pub(super) projection: SchemaProjection,
    /// The composed schema digest the closure was derived at; empty when the
    /// question needed no schema and none was readable.
    pub(super) schema_digest: String,
    /// Why the closure could not be read; a question with required
    /// capabilities then refuses `DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE`.
    pub(super) schema_error: Option<String>,
    /// The agents and graph revision a `load_set` question reads beside the
    /// index (MEDULLA-DECISION-ENGINE-R176.3); `None` for a designation.
    pub(super) load_set: Option<super::stat_load_set::LoadSetRead>,
    /// The pack facts and tool outcome counts a `usage_guidance` question
    /// reads (MEDULLA-DECISION-ENGINE-R177.3); `None` otherwise.
    pub(super) usage_guidance: Option<super::stat_usage_guidance::UsageGuidanceRead>,
    /// The adapter active for the question's capability scope (or the graph's
    /// scope), applied to the task embedding before the index read
    /// (MEDULLA-DECISION-ENGINE-R172.13). `None` ranks by the base vector.
    pub(super) adapter: Option<AppliedAdapter>,
}

/// An active adapter and the revision of the pointer move that activated it.
#[derive(Clone)]
pub(super) struct AppliedAdapter {
    pub(super) served: Arc<medulla_server_ops::api::admin::served_adapter::ServedAdapter>,
    pub(super) revision: u64,
}

impl AppliedAdapter {
    /// The premise fact naming the adapter: `adapter:<digest>@<revision>`.
    pub(super) fn fact(&self) -> String {
        format!("adapter:{}@{}", self.served.digest(), self.revision)
    }
}

/// The adapter a question runs with: the scope of its one required
/// capability when that scope has an active adapter, else the graph scope,
/// else none. A graph the caller cannot read has none.
async fn adapter_of(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    ask: &DesignationAsk,
) -> Option<AppliedAdapter> {
    let core = super::topology_read::graph_core(state, &ask.graph, verified)
        .await
        .ok()?;
    let tenant = ask.question.tenant_id.as_str();
    let mut scopes = Vec::new();
    let mut required = ask.question.required_capabilities.iter();
    if let (Some(capability), None) = (required.next(), required.next()) {
        scopes.push(AdapterScope::Capability {
            capability_iri: capability.to_string(),
        });
    }
    scopes.push(AdapterScope::Graph);
    for scope in scopes {
        let key = super::stat_adapter::scoped_adapter_pointer(&ask.graph, &scope);
        if let Some((served, revision)) =
            super::served_adapter::served_at(state, tenant, &key, &core).await
        {
            return Some(AppliedAdapter { served, revision });
        }
    }
    None
}

/// `CODE: detail`, the one refusal text form of a registered code.
pub(super) fn coded(
    code: medulla_types::refusal_code::RefusalCode,
    detail: impl std::fmt::Display,
) -> String {
    format!("{}: {detail}", code.as_str())
}

/// The typed question of a designation request, or why the request is not one.
pub(super) fn question_of(request: &DecideRequest) -> Result<&DesignationAsk, String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
        || request.params.len() != 1
    {
        return Err(invalid_parameter(
            "capability_designation needs ordinary safety, no head or belief slices, and one parameter",
        ));
    }
    let param = &request.params.as_slice()[0];
    let TypedValue::CapabilityDesignation(ask) = &param.value else {
        return Err(invalid_parameter(
            "capability_designation requires a typed designation question",
        ));
    };
    if param.name != DESIGNATION_PARAM {
        return Err(invalid_parameter(format!(
            "the designation parameter must be named {DESIGNATION_PARAM}"
        )));
    }
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(
            "capability_designation declares exactly one candidate: the question id",
        ));
    };
    let [option] = options.as_slice() else {
        return Err(invalid_parameter(
            "capability_designation declares exactly one candidate: the question id",
        ));
    };
    if option.option_id != request.question.question_id
        || !option.classification.is_empty()
        || !option.numbers.is_empty()
        || !option.texts.is_empty()
    {
        return Err(invalid_parameter(
            "the declared candidate must be the question id without facts",
        ));
    }
    if ask.question.tenant_id.as_str() != request.tenant_id {
        return Err(
            "ACCESS_DENIED: the designation question tenant must be the request tenant".to_string(),
        );
    }
    Ok(ask)
}

/// The resident capability index every question and the feed share. It is
/// created once, under the engine policy's shard limits, and stores nothing
/// durable: the Agent Library is its source.
pub fn shared_capability_index(state: &ServerState) -> Arc<CapabilityIndex> {
    let mut slot = state.capability_index.lock();
    if let Some(index) = slot
        .as_ref()
        .and_then(|handle| Arc::clone(handle).downcast::<CapabilityIndex>().ok())
    {
        return index;
    }
    let knobs = DesignationKnobs::from_policy(&DecisionPolicy::engine_default());
    let index = Arc::new(CapabilityIndex::new(ShardLimits {
        max_records: knobs.max_records_per_shard,
        compact_ratio_q32: knobs.tombstone_compact_ratio_q32,
    }));
    *slot = Some(Arc::clone(&index) as ErasedHandle);
    index
}

fn unkey(node: &str) -> String {
    node.trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// The subclass closure of the graph's composed schema and its digest.
pub(super) async fn schema_closure(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    graph: &str,
) -> Result<(SchemaProjection, String), String> {
    let core = super::topology_read::graph_core(state, graph, verified).await?;
    // In-process OWL completion over the composed schema: CPU work.
    tokio::task::spawn_blocking(move || {
        let sources = core.schema_sources();
        medulla_server_api::schema_compose::validate_and_compose(&sources)?;
        let (triples, _, _) = super::topology_schema::ontology(&sources)?;
        let classification = medulla_reasoner::owl::classify_hybrid_weighted(&triples);
        let digest = format!("sha256:{}", sources.composed_digest().to_hex());
        let edges: Vec<(String, String)> = classification
            .subsumers
            .iter()
            .flat_map(|(sub, supers)| {
                supers
                    .iter()
                    .filter(move |sup| *sup != sub)
                    .map(move |sup| (unkey(sub), unkey(sup)))
            })
            .filter(|(sub, sup)| {
                sub.contains(':') && sup.contains(':') && !sup.ends_with("owl#Thing")
            })
            .collect();
        let projection = SchemaProjection::new(&digest, edges)
            .with_classes(classification.subsumers.keys().map(|class| unkey(class)));
        Ok::<_, String>((projection, digest))
    })
    .await
    .map_err(|error| format!("designation schema closure task failed: {error}"))?
}

/// Load the index handle and the schema closure of the question. A schema
/// that cannot be read is recorded in the runtime: the family refuses a
/// question with required capabilities then, and never falls back to literal
/// matching.
pub(super) async fn with_runtime(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    request: &DecideRequest,
) -> Result<DesignationRuntime, String> {
    let ask = question_of(request)?;
    let index = shared_capability_index(&*state.read().await);
    let adapter = adapter_of(state, verified, ask).await;
    Ok(match schema_closure(state, verified, &ask.graph).await {
        Ok((projection, schema_digest)) => DesignationRuntime {
            index,
            projection,
            schema_digest,
            schema_error: None,
            load_set: None,
            usage_guidance: None,
            adapter,
        },
        Err(detail) => DesignationRuntime {
            index,
            projection: SchemaProjection::default(),
            schema_digest: String::new(),
            schema_error: Some(format!(
                "the schema closure of graph {} is unavailable: {detail}",
                ask.graph
            )),
            load_set: None,
            usage_guidance: None,
            adapter,
        },
    })
}
