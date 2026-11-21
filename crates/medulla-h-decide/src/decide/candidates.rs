//! `CandidateSource`: where a statistical decision's options come from (EH-059).
//!
//! * **Library** candidates are the tenant's published HEAD components in the
//!   request's kind and classification scope, read through the tenant-bound
//!   Agent Library search. Visibility for library rows is tenant-wide
//!   (DECIDE-LAYER-DESIGN §4.3), so step 1a is the tenant check plus the scope.
//! * **Graph** candidates are the rows an ordinary plan returns over ONE graph
//!   snapshot the caller's RLS already filtered: the graph ACL is checked, the
//!   snapshot is filtered BEFORE the plan runs, and the plan may only select
//!   (`Scan`, `Filter`, `Traverse`, `AsOf`, `Limit`) -- a ranking stage would
//!   read global ANN/BM25 statistics or truncate before visibility, both leak
//!   vectors, so it is refused. Features then read exactly those rows.
//!
//! Either set is bounded BEFORE any feature is computed; a scope naming more
//! options than one decision may hold is refused, never truncated.

#[cfg(feature = "query")]
use std::collections::BTreeMap;

use medulla_agent_types::{
    agent_component::AgentComponentEntry, agent_component::AgentComponentKind,
    agent_component::AgentComponentSearchRequest,
};
use medulla_decision_stats::decision::candidate::CandidateView;
use medulla_decision_types::decision::statistical::graph_rows::{
    GraphNodeClass, GraphRowProvenance,
};
use medulla_decision_types::decision::statistical::log::RecordVisibility;
use medulla_decision_types::{
    decision::statistical::declared::check_declared,
    decision::statistical::declared::MAX_RANK_CATALOG_CANDIDATES,
    decision::statistical::declared::DeclaredOption,
};
use medulla_decision_types::{
    decision::statistical::CandidateSource, decision::statistical::StatisticalErrorCode,
    decision::statistical::TypedParam, decision::statistical::TypedValue,
};
use medulla_types::contract::BoundedVec;

use super::features::capability_text::capability_features;
use medulla_decision_types::{
    decision::CandidateSourceRecord, decision::LibraryCandidateScope,
    decision::MAX_ASSEMBLY_CANDIDATES,
};

use super::family::DecisionFamily;
use super::stat_classes::{current_rules, derive, DerivedClasses};
use super::stat_support::refusal;
use medulla_persistence_records::agent_library::AgentLibraryStore;

/// Domain of a declared option set's digest.
pub(super) const DECLARED_OPTIONS_DOMAIN: &str = "eg/decide-declared-options/v1";
/// Page size of the library read.
const PAGE: u32 = 256;
/// Most pages one read walks before it refuses as unbounded.
const MAX_PAGES: usize = 64;

/// The options of one decision, sorted by id.
pub(super) struct ReadCandidates {
    pub(super) views: Vec<CandidateView>,
    /// The library entries behind `views`; empty for graph candidates.
    pub(super) entries: Vec<AgentComponentEntry>,
    pub(super) record: CandidateSourceRecord,
    #[allow(dead_code)]
    // set at every candidate construction; the record's visibility is enforced upstream, kept for the candidate's provenance
    pub(super) visibility: RecordVisibility,
    /// Rule-derived classes of library candidates (EH-200); empty for graph rows.
    pub(super) derived: DerivedClasses,
    /// The provenance of each graph row read, in id order; empty for library
    /// and declared candidates (MEDULLA-DECISION-ENGINE-R121.2).
    pub(super) graph_rows: Vec<GraphRowProvenance>,
}

fn too_many() -> String {
    refusal(
        StatisticalErrorCode::CandidateSetTooLarge,
        format!("the scope names more than {MAX_ASSEMBLY_CANDIDATES} options; narrow it"),
    )
}

fn search_request(
    tenant_id: &str,
    scope: &LibraryCandidateScope,
    cursor: Option<String>,
) -> AgentComponentSearchRequest {
    AgentComponentSearchRequest {
        tenant_id: tenant_id.to_string(),
        task: None,
        // With kinds to page by, `classification_under` is applied locally
        // over declared AND rule-derived classes (EH-200); without kinds the
        // declared classification is the only selector the search has.
        capabilities: if scope.kinds.is_empty() {
            scope.classification_under.iter().cloned().collect()
        } else {
            Vec::new()
        },
        kinds: scope.kinds.iter().copied().collect(),
        read_only: false,
        limit: Some(PAGE),
        cursor,
    }
}

/// Whether `entry` falls under `root` by its declared or derived classes.
fn under_root(entry: &AgentComponentEntry, derived: &DerivedClasses, root: Option<&str>) -> bool {
    let Some(root) = root else { return true };
    let derived_classes = derived
        .by_component
        .get(&entry.component_id)
        .into_iter()
        .flatten()
        .map(|(class, _)| class);
    entry
        .classification
        .iter()
        .chain(derived_classes)
        .any(|class| medulla_agent_types::agent_ontology::satisfies(class, root))
}

fn read_library(
    store: &AgentLibraryStore,
    tenant_id: &str,
    scope: &LibraryCandidateScope,
) -> Result<(Vec<AgentComponentEntry>, DerivedClasses), String> {
    let mut entries = Vec::new();
    let mut derived = DerivedClasses {
        rules: current_rules(),
        ..DerivedClasses::default()
    };
    let mut cursor = None;
    for _ in 0..MAX_PAGES {
        let page = store
            .search_components(&search_request(tenant_id, scope, cursor))
            .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
        let classes = derive(&page.entries);
        for entry in page.entries {
            if under_root(&entry, &classes, scope.classification_under.as_deref()) {
                let own = classes
                    .by_component
                    .get(&entry.component_id)
                    .cloned()
                    .unwrap_or_default();
                derived.by_component.insert(entry.component_id.clone(), own);
                entries.push(entry);
            }
        }
        if entries.len() > MAX_ASSEMBLY_CANDIDATES {
            return Err(too_many());
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok((entries, derived)),
        }
    }
    Err(refusal(
        StatisticalErrorCode::CandidateSetTooLarge,
        "the candidate scan did not finish",
    ))
}

fn view_of(entry: &AgentComponentEntry, derived: &DerivedClasses) -> CandidateView {
    let mut view = CandidateView::from_component(entry);
    for (class, _) in derived
        .by_component
        .get(&entry.component_id)
        .into_iter()
        .flatten()
    {
        if !view.classification.contains(class) {
            view.classification.push(class.clone());
        }
    }
    view
}

/// Read library candidates for `tenant_id`.
pub(super) fn library_candidates(
    store: &AgentLibraryStore,
    tenant_id: &str,
    scope: &LibraryCandidateScope,
) -> Result<ReadCandidates, String> {
    let (mut entries, derived) = read_library(store, tenant_id, scope)?;
    entries.sort_by(|a, b| a.component_id.cmp(&b.component_id));
    Ok(ReadCandidates {
        views: entries
            .iter()
            .map(|entry| view_of(entry, &derived))
            .collect(),
        entries,
        derived,
        record: CandidateSourceRecord::AgentLibrary {
            kinds: scope.kinds.clone(),
            classification_under: scope.classification_under.clone(),
        },
        visibility: RecordVisibility::Tenant,
        graph_rows: Vec::new(),
    })
}

/// The candidates of a question whose family reads its own index instead of
/// the stage-2 candidate scan (`ToolRelevance`, MEDULLA-DECISION-ENGINE-R134.110.3).
///
/// The source must be the Agent Library scoped to the `Tool` kind alone; no
/// view is read here, so the 64-candidate stage-2 cap does not apply. The
/// record still names the source, so the sealed record binds it.
pub(super) fn indexed_tool_candidates(source: &CandidateSource) -> Result<ReadCandidates, String> {
    let CandidateSource::AgentLibrary { scope } = source else {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "a tool_relevance question reads the Agent Library with kind tool",
        ));
    };
    if scope.kinds.as_slice() != [AgentComponentKind::Tool] {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "a tool_relevance question reads the Agent Library with kind tool",
        ));
    }
    Ok(ReadCandidates {
        views: Vec::new(),
        entries: Vec::new(),
        graph_rows: Vec::new(),
        record: CandidateSourceRecord::AgentLibrary {
            kinds: scope.kinds.clone(),
            classification_under: scope.classification_under.clone(),
        },
        visibility: RecordVisibility::Tenant,
        derived: DerivedClasses {
            rules: current_rules(),
            ..DerivedClasses::default()
        },
    })
}

/// Whether one plan stage only SELECTS rows of the already-filtered snapshot.
#[cfg(feature = "query")]
fn selects_only(op: &medulla_query_types::wire::Op) -> bool {
    use medulla_query_types::wire::Op;
    matches!(
        op,
        Op::Scan { .. }
            | Op::Filter { .. }
            | Op::Traverse { .. }
            | Op::AsOf { .. }
            | Op::Limit { .. }
    )
}

/// Refuse a graph plan that could rank or truncate over invisible rows.
#[cfg(feature = "query")]
pub(super) fn check_selecting_plan(plan: &medulla_query_types::wire::Plan) -> Result<(), String> {
    if plan.ops.is_empty() || !plan.ops.iter().all(selects_only) {
        return Err(refusal(
            StatisticalErrorCode::CandidatePlanRefused,
            "a graph candidate plan may only Scan, Filter, Traverse, AsOf and Limit: ranking \
             stages read global index statistics before visibility",
        ));
    }
    Ok(())
}

/// One visible row's properties as a candidate: numbers on `Q32`, strings as
/// text fields. Other property shapes are not features.
#[cfg(feature = "query")]
fn row_view(id: String, properties: &serde_json::Value) -> CandidateView {
    let mut numbers = BTreeMap::new();
    let mut texts = BTreeMap::new();
    for (key, value) in properties.as_object().into_iter().flatten() {
        if let Some(number) = value.as_f64() {
            if let Ok(q) = medulla_decision_stats::decision::quant::q32(number) {
                numbers.insert(key.clone(), q.value);
            }
        } else if let Some(text) = value.as_str() {
            texts.insert(key.clone(), text.to_string());
        }
    }
    CandidateView::from_row(id, numbers, texts)
}

/// Whether the node's typed label is `label`: its `type`, `node_type` or
/// `label` field, or a member of its `labels` list. Property names other than
/// these never decide a row's class.
fn has_label(properties: &serde_json::Value, label: &str) -> bool {
    ["type", "node_type", "label"]
        .iter()
        .any(|key| properties.get(key).and_then(serde_json::Value::as_str) == Some(label))
        || properties
            .get("labels")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|labels| labels.iter().any(|value| value.as_str() == Some(label)))
}

/// Whether `text` is an absolute IRI: a scheme, a colon and a non-empty rest
/// with no whitespace or control character.
fn is_iri(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The class and source of one graph row (MEDULLA-DECISION-ENGINE-R121.2). An
/// `Evidence` node that carries a `source` is an observation and the source
/// must be an IRI; every other node is a claim.
pub(super) fn graph_row_provenance(
    graph: &str,
    graph_version: u64,
    node_id: &str,
    properties: &serde_json::Value,
) -> Result<GraphRowProvenance, String> {
    let source = properties.get("source");
    let (node_class, source_iri) = match source {
        Some(source) if has_label(properties, "Evidence") => {
            let iri = source.as_str().filter(|text| is_iri(text)).ok_or_else(|| {
                refusal(
                    StatisticalErrorCode::GraphProvenanceMalformed,
                    format!("evidence row {node_id} names a source that is not an IRI"),
                )
            })?;
            (GraphNodeClass::Observation, Some(iri.to_string()))
        }
        _ => (GraphNodeClass::Claim, None),
    };
    Ok(GraphRowProvenance {
        graph: graph.to_string(),
        node_id: node_id.to_string(),
        node_class,
        source_iri,
        graph_version,
    })
}

/// Graph candidates: the rows `plan` selects from `view`, which the caller's
/// RLS has already filtered.
#[cfg(feature = "query")]
pub(super) fn graph_candidates(
    view: &medulla_graph_core::graph::GraphView,
    graph: &str,
    graph_version: u64,
    plan: &medulla_query_types::wire::Plan,
    principal: String,
) -> Result<ReadCandidates, String> {
    check_selecting_plan(plan)?;
    let semantic = medulla_semantic_index::semantic::SemanticStore::new();
    let rows =
        medulla_exec::exec::execute(plan, &medulla_exec::exec::PlanCtx::new(view, &semantic))
            .map_err(|detail| refusal(StatisticalErrorCode::CandidatePlanRefused, detail))?;
    let mut ids = rows.ids();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() > MAX_ASSEMBLY_CANDIDATES {
        return Err(too_many());
    }
    let mut views = Vec::with_capacity(ids.len());
    let mut graph_rows = Vec::with_capacity(ids.len());
    for id in ids {
        let properties = view
            .node_properties
            .get(&id)
            .and_then(|blob| medulla_types::msgpack::decode_property_value(blob.as_slice()).ok())
            .unwrap_or(serde_json::Value::Null);
        graph_rows.push(graph_row_provenance(
            graph,
            graph_version,
            &id,
            &properties,
        )?);
        views.push(row_view(id, &properties));
    }
    Ok(ReadCandidates {
        views,
        entries: Vec::new(),
        record: CandidateSourceRecord::Graph {
            plan_digest: medulla_query_types::solve_scalar::digest_text(
                "eg/decide-candidate-plan/v1",
                &(graph, plan),
            ),
        },
        visibility: RecordVisibility::Principal { principal },
        derived: DerivedClasses::default(),
        graph_rows,
    })
}

/// The Q32 fact named `key` on `option`, or 0 when the option does not
/// carry it -- absence is a feature-reading concern elsewhere, not a shape
/// refusal here.
pub(super) fn declared_number(option: &DeclaredOption, key: &str) -> i64 {
    option
        .numbers
        .iter()
        .find(|n| n.key == key)
        .map_or(0, |n| n.q32)
}

/// Map a question-specific registry check's failure to its wire refusal.
///
/// Each such `check()` (`IngestionLaneRequest`, `EnrichmentScheduleRequest`)
/// reports its MOST SPECIFIC failure as free text prefixed with its own
/// closed-vocabulary code (e.g. `"UNSUPPORTED_INGESTION_LANE: "`), mirrored
/// by a matching `StatisticalErrorCode` variant; a less specific failure
/// (an empty or malformed candidate list) carries no such prefix. Passing
/// the whole string through `refusal(StatisticalErrorCode::ParameterInvalid,
/// ...)` regardless would nest a specific code two levels deep
/// (`"PARAMETER_INVALID: UNSUPPORTED_INGESTION_LANE: ..."`), and
/// `crate::protocol::Response::err` only ever promotes the FIRST
/// `"CODE: detail"` segment to the wire-visible `error` field -- the served
/// response would read `PARAMETER_INVALID`, silently dropping the specific
/// code a caller needs to branch on (caught by
/// `consumer_tests::an_unregistered_ingestion_lane_is_refused_through_decide`
/// and `consumer_tests::a_negative_enrichment_cost_is_refused_through_decide`,
/// which assert on the served response, not the type's own unit test).
pub(super) fn prefixed_refusal(detail: &str, prefix: &str, code: StatisticalErrorCode) -> String {
    match detail.strip_prefix(prefix) {
        Some(rest) => refusal(code, rest),
        None => refusal(StatisticalErrorCode::ParameterInvalid, detail),
    }
}

/// Declared candidates: the caller's own options, each fact a claim, visible
/// to the declaring principal only. Validated before any feature reads them.
///
/// The family's `validate_candidates` hook carries the question-specific
/// declared-option registry gates (MEDULLA-DECISION-ENGINE-R001.101): an
/// `IngestionLane` candidate must name a registered
/// [`medulla_decision_types::decision::statistical::ingestion_lane::IngestionLane`],
/// refused here rather than discovered later as an opaque abstention
/// (MEDULLA-DECISION-ENGINE-R030); an `EnrichmentSchedule` candidate may not
/// declare a negative `enrichment.cost_q32` -- it has no meaning on that
/// scale and would let a candidate manufacture unbounded net value
/// (MEDULLA-DECISION-ENGINE-R031); a `RetrievalPlan` candidate whose option
/// id claims the reserved
/// [`medulla_decision_types::decision::statistical::retrieval_plan::OPTION_ID_PREFIX`]
/// namespace must name a registered
/// [`medulla_decision_types::decision::statistical::retrieval_plan::RetrievalPlanKind`],
/// refused the same way (MEDULLA-DECISION-ENGINE-R029). An option id outside
/// that namespace is an ordinary declared option, unaffected by this
/// registry.
pub(super) fn declared_candidates(
    options: &[DeclaredOption],
    principal: &str,
    family: &dyn DecisionFamily,
) -> Result<ReadCandidates, String> {
    family.precheck_declared(options)?;
    check_declared(options)
        .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?;
    family.validate_candidates(options, principal)?;
    let views = options
        .iter()
        .map(|option| {
            let mut view = CandidateView::from_row(
                option.option_id.clone(),
                option
                    .numbers
                    .iter()
                    .map(|n| (n.key.clone(), n.q32))
                    .collect(),
                option
                    .texts
                    .iter()
                    .map(|t| (t.key.clone(), t.text.clone()))
                    .collect(),
            );
            view.classification = option.classification.iter().cloned().collect();
            view
        })
        .collect();
    Ok(ReadCandidates {
        views,
        entries: Vec::new(),
        record: CandidateSourceRecord::Declared {
            options_digest: medulla_query_types::solve_scalar::digest_text(
                DECLARED_OPTIONS_DOMAIN,
                &options,
            ),
        },
        visibility: RecordVisibility::Principal {
            principal: principal.to_string(),
        },
        derived: DerivedClasses::default(),
        graph_rows: Vec::new(),
    })
}

/// Read the options `source` names. Graph candidates need the caller's
/// filtered snapshot, which [`super::stat_decide`] takes under the state lock.
/// `family` is the question's family, whose `validate_candidates` hook
/// carries the declared candidates' question-specific registry gate.
pub(super) fn read_candidates(
    store: &AgentLibraryStore,
    tenant_id: &str,
    source: &CandidateSource,
    graph_view: Option<&medulla_graph_core::graph::GraphView>,
    graph_version: u64,
    principal: &str,
    family: &dyn DecisionFamily,
    params: &[TypedParam],
) -> Result<ReadCandidates, String> {
    match source {
        CandidateSource::AgentLibrary { scope } => library_candidates(store, tenant_id, scope),
        CandidateSource::Declared { options } => {
            declared_candidates(options.as_slice(), principal, family)
        }
        CandidateSource::RankCatalog { options } => {
            if !family.accepts_rank_catalog() {
                return Err(refusal(
                    StatisticalErrorCode::ParameterInvalid,
                    "this question does not accept a rank catalog",
                ));
            }
            if options.len() > MAX_RANK_CATALOG_CANDIDATES {
                return Err(refusal(
                    StatisticalErrorCode::CandidateSetTooLarge,
                    format!(
                        "a rank catalog names at most {MAX_RANK_CATALOG_CANDIDATES} candidates; {} were sent",
                        options.len()
                    ),
                ));
            }
            let query = params
                .iter()
                .find_map(|param| match (&*param.name, &param.value) {
                    ("query", TypedValue::Text(text)) => Some(text.as_str()),
                    _ => None,
                })
                .ok_or_else(|| {
                    refusal(
                        StatisticalErrorCode::ParameterInvalid,
                        "a rank catalog question carries a text param named query",
                    )
                })?;
            // Medulla computes the features from the candidate text; the
            // embedding model is not reachable from the candidate stage, so
            // semantic_similarity is absent.
            let features = capability_features(query, options, None);
            let featured = options
                .iter()
                .zip(features)
                .map(|(option, numbers)| {
                    let mut option = option.clone();
                    option.numbers = BoundedVec::new(numbers).map_err(|_| {
                        refusal(
                            StatisticalErrorCode::ParameterInvalid,
                            "a rank catalog candidate carries no numbers",
                        )
                    })?;
                    Ok(option)
                })
                .collect::<Result<Vec<_>, String>>()?;
            declared_candidates(&featured, principal, family)
        }
        CandidateSource::Graph { graph, plan } => {
            let view = graph_view.ok_or_else(|| {
                refusal(
                    StatisticalErrorCode::CandidatePlanRefused,
                    "graph candidates need the query feature",
                )
            })?;
            #[cfg(feature = "query")]
            {
                graph_candidates(view, graph, graph_version, plan, principal.to_string())
            }
            #[cfg(not(feature = "query"))]
            {
                let _ = (view, graph, graph_version, plan, principal);
                Err(refusal(
                    StatisticalErrorCode::CandidatePlanRefused,
                    "graph candidates need the query feature",
                ))
            }
        }
    }
}

/// The caller's RLS-filtered snapshot of the graph a graph source names, with
/// the graph's write version, or `None` for a library source. The graph ACL is
/// checked first; the snapshot is filtered before any plan stage sees it.
#[cfg(feature = "query")]
pub(super) async fn filtered_graph_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    source: &CandidateSource,
) -> Result<Option<(medulla_graph_core::graph::GraphView, u64)>, String> {
    let CandidateSource::Graph { graph, .. } = source else {
        return Ok(None);
    };
    Ok(Some(
        super::stat_support::filtered_view_versioned(state, agent_id, graph).await?,
    ))
}

/// Without the query feature a graph source is refused before any read.
#[cfg(not(feature = "query"))]
pub(super) async fn filtered_graph_view(
    _state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    _agent_id: &str,
    _source: &CandidateSource,
) -> Result<Option<(medulla_graph_core::graph::GraphView, u64)>, String> {
    Ok(None)
}
