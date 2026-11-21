//! `COMPANY CONTEXT [CATEGORY name]` (MEDULLA-FEDERATED-QUERY-R078.7): the governed
//! company-context view, a UQL source over the caller's pinned, row-projected snapshot.
//!
//! One row is a source-bound reference to an organization, role, data asset or permission
//! node, or to the latest committed point of a data asset's series. A row carries its
//! provenance as text columns and never a copied value. A node whose `clearance` labels are
//! not all in the injected scope's labels is dropped before any time-series read. The view
//! adds no ontology term: the four classes are the core catalog's own.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::open_metadata::EXTERNAL_VERSION_PROPERTY;
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_query_types::wire::CompanyCategory;
use medulla_types::contract::ServerErrorCode;

use super::PlanCtx;
use crate::rowset::{RowSet, TextChannels};

/// The `source_id` of a node no connector synced.
const NATIVE_SOURCE: &str = "native";
/// The node property naming the connector source a synced node came from.
const SOURCE_PROPERTY: &str = "source";
/// The node property listing the clearance labels a reader must hold.
const CLEARANCE_PROPERTY: &str = "clearance";
/// The data-asset property naming its time series.
const SERIES_PROPERTY: &str = "series_id";

/// The ontology class a category reads.
fn class_of(category: CompanyCategory) -> &'static str {
    match category {
        CompanyCategory::Organization => "Organization",
        CompanyCategory::Role => "Role",
        CompanyCategory::DataAsset | CompanyCategory::TimeSeries => "DataAsset",
        CompanyCategory::Permission => "SecurityClearance",
    }
}

/// Whether some ontology document of `sources` declares `class` as an `owl:Class` or
/// `rdfs:Class`: a subject whose local name is `class`, then `a` or `rdf:type`, then the
/// class term.
fn class_declared(sources: &GraphSchemaSources, class: &str) -> bool {
    sources.ontologies().any(|(_, document)| {
        document.lines().any(|line| {
            let mut tokens = line.split_whitespace();
            let (Some(subject), Some(predicate), Some(object)) =
                (tokens.next(), tokens.next(), tokens.next())
            else {
                return false;
            };
            let local = subject
                .trim_end_matches('>')
                .rsplit([':', '#', '/'])
                .next()
                .unwrap_or("");
            local == class
                && matches!(predicate, "a" | "rdf:type")
                && matches!(
                    object.trim_end_matches([';', '.', ',']),
                    "owl:Class" | "rdfs:Class"
                )
        })
    })
}

/// The categories a node of `props` belongs to, first match only, in the fixed order
/// organization, role, data asset, permission.
fn node_category(props: &serde_json::Map<String, serde_json::Value>) -> Option<CompanyCategory> {
    let named = |class: &str| {
        ["type", "node_type", "label"]
            .iter()
            .any(|key| props.get(*key).and_then(serde_json::Value::as_str) == Some(class))
    };
    [
        CompanyCategory::Organization,
        CompanyCategory::Role,
        CompanyCategory::DataAsset,
        CompanyCategory::Permission,
    ]
    .into_iter()
    .find(|category| named(class_of(*category)))
}

/// The clearance labels a node requires: a string, an array of strings, or none.
pub(super) fn node_clearance(props: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    match props.get(CLEARANCE_PROPERTY) {
        Some(serde_json::Value::String(label)) => vec![label.clone()],
        Some(serde_json::Value::Array(labels)) => labels
            .iter()
            .filter_map(|label| label.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// A scalar property as text: a string as is, a number or boolean as written.
fn scalar_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Run one `COMPANY CONTEXT` stage (a source: its input is ignored, like `MATCH`).
pub(super) fn company_context_scan(
    ctx: &PlanCtx,
    category: Option<CompanyCategory>,
) -> Result<RowSet, String> {
    let scope = ctx
        .plan_scope
        .as_ref()
        .filter(|scope| scope.verified)
        .ok_or_else(|| {
            format!(
                "{}: the plan scope tenant is not verified",
                ServerErrorCode::PlanScopeUnverified
            )
        })?;
    let requested: Vec<CompanyCategory> = match category {
        Some(category) => vec![category],
        None => CompanyCategory::ALL.to_vec(),
    };
    for requested in &requested {
        let class = class_of(*requested);
        if !class_declared(&ctx.view.schema_sources, class) {
            return Err(format!(
                "{}: the class `{class}` of category `{}` is not declared in the snapshot schema",
                ServerErrorCode::CompanyMappingMissing,
                requested.name()
            ));
        }
    }
    let wants = |wanted: CompanyCategory| requested.contains(&wanted);
    let allowed: BTreeSet<&str> = scope.clearance_labels.iter().map(String::as_str).collect();
    let snapshot_revision = ctx.snapshot_revision.unwrap_or(0).to_string();
    let ids: BTreeSet<&String> = ctx.view.node_map.keys().collect();
    let mut order: Vec<String> = Vec::new();
    let mut texts: TextChannels = BTreeMap::new();
    for id in ids {
        let Some(props) = ctx.view.node_row_object(id) else {
            continue;
        };
        let Some(node_cat) = node_category(&props) else {
            continue;
        };
        let clearance = node_clearance(&props);
        if !clearance.iter().all(|label| allowed.contains(label.as_str())) {
            continue;
        }
        let source_id = props
            .get(SOURCE_PROPERTY)
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| NATIVE_SOURCE.to_string(), str::to_string);
        let source_revision = props
            .get(EXTERNAL_VERSION_PROPERTY)
            .and_then(scalar_text)
            .unwrap_or_else(|| snapshot_revision.clone());
        let provenance = |category: CompanyCategory| -> BTreeMap<String, String> {
            BTreeMap::from([
                ("category".to_string(), category.name().to_string()),
                ("entity_id".to_string(), id.clone()),
                ("source_id".to_string(), source_id.clone()),
                ("source_revision".to_string(), source_revision.clone()),
                ("clearance".to_string(), clearance.join(",")),
            ])
        };
        if wants(node_cat) {
            order.push(id.clone());
            texts.insert(id.clone(), provenance(node_cat));
        }
        if node_cat == CompanyCategory::DataAsset && wants(CompanyCategory::TimeSeries) {
            if let Some(series_id) = props.get(SERIES_PROPERTY).and_then(serde_json::Value::as_str)
            {
                if let Some((timestamp_ms, row_id)) = latest_point_row(ctx, series_id)? {
                    let mut row = provenance(CompanyCategory::TimeSeries);
                    row.insert("series_id".to_string(), series_id.to_string());
                    row.insert("timestamp_ms".to_string(), timestamp_ms.to_string());
                    order.push(row_id.clone());
                    texts.insert(row_id, row);
                }
            }
        }
    }
    Ok(RowSet::from_rows(order.into_iter().map(|id| (id, None))).with_texts(texts))
}

/// The instant a time-bounded read measures "at or before the snapshot" against. A read
/// pinned to a snapshot revision uses that revision's commit instant, and refuses
/// `SNAPSHOT_TIME_UNKNOWN` when the commit log does not reach it (never the pinned clock or
/// the wall clock). A read with no pinned revision uses the pinned `now_ms`, then the clock.
#[cfg(feature = "timeseries")]
fn snapshot_instant_ms(ctx: &PlanCtx) -> Result<i64, String> {
    if let Some(committed_at_ms) = ctx.snapshot_commit_ms {
        return Ok(committed_at_ms);
    }
    if let Some(revision) = ctx.snapshot_revision {
        return Err(format!(
            "{}: the commit instant of snapshot revision {revision} is not retained",
            ServerErrorCode::SnapshotTimeUnknown
        ));
    }
    Ok(ctx.now_ms.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }))
}

/// The latest committed point of `series_id` at or before the snapshot's commit instant
/// ([`snapshot_instant_ms`]), as `(timestamp_ms, row id)`. No store, scope or point yields
/// none; no value is read out.
#[cfg(feature = "timeseries")]
fn latest_point_row(ctx: &PlanCtx, series_id: &str) -> Result<Option<(i64, String)>, String> {
    const NS_PER_MS: i64 = 1_000_000;
    let at_ms = snapshot_instant_ms(ctx)?;
    let committed = super::tsscan::CommittedSeries {
        store: ctx.tsdb,
        tenant: ctx.tsdb_tenant,
        graph: ctx.tsdb_graph,
    };
    let Some((ts_ns, _)) =
        committed.latest_at_or_before(series_id, at_ms.saturating_mul(NS_PER_MS))
    else {
        return Ok(None);
    };
    let timestamp_ms = ts_ns / NS_PER_MS;
    Ok(Some((timestamp_ms, format!("ts:{series_id}:{timestamp_ms}"))))
}

/// Without the `timeseries` feature there is no store to read: no time-series rows.
#[cfg(not(feature = "timeseries"))]
fn latest_point_row(_ctx: &PlanCtx, _series_id: &str) -> Result<Option<(i64, String)>, String> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphCore;
    use medulla_query_types::wire::PlanScope;
    use medulla_semantic_index::semantic::SemanticStore;

    fn blob(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).expect("encodes")
    }

    fn scope(labels: &[&str]) -> PlanScope {
        PlanScope {
            tenant: "tenant-a".into(),
            clearance_labels: labels.iter().map(|l| l.to_string()).collect(),
            verified: true,
            leaves: Vec::new(),
        }
    }

    fn graph() -> GraphCore {
        let core = GraphCore::new();
        // `type` and `label` name different categories: the fixed order gives organization.
        core.add_node(
            "ambiguous".into(),
            blob(serde_json::json!({ "type": "Organization", "label": "Role" })),
        );
        core.add_node("role-1".into(), blob(serde_json::json!({ "node_type": "Role" })));
        core.add_node(
            "asset-1".into(),
            blob(serde_json::json!({ "label": "DataAsset", "source": "warehouse", "clearance": "internal" })),
        );
        core.add_node(
            "asset-2".into(),
            blob(serde_json::json!({ "type": "DataAsset", (EXTERNAL_VERSION_PROPERTY): 7, "clearance": ["internal", "finance"] })),
        );
        core.add_node(
            "perm-1".into(),
            blob(serde_json::json!({ "type": "SecurityClearance" })),
        );
        core.add_node("other".into(), blob(serde_json::json!({ "type": "Doc" })));
        core
    }

    fn run(
        core: &GraphCore,
        labels: &[&str],
        category: Option<CompanyCategory>,
    ) -> Result<RowSet, String> {
        let view = core.analysis_snapshot();
        let semantic = SemanticStore::new();
        let ctx = PlanCtx::new(&view, &semantic)
            .with_plan_scope(scope(labels))
            .with_snapshot_revision(42);
        company_context_scan(&ctx, category)
    }

    // spec: MEDULLA-FEDERATED-QUERY-R078.7
    #[test]
    fn fixed_category_order_and_clearance_subset() {
        let core = graph();
        let rows = run(&core, &["internal"], None).expect("scan");
        // asset-2 needs `finance` too, so it is dropped; `other` is in no category.
        assert_eq!(
            rows.ids(),
            vec!["ambiguous", "asset-1", "perm-1", "role-1"]
        );
        assert_eq!(rows.text("ambiguous", "category"), Some("organization"));
        assert_eq!(rows.text("role-1", "category"), Some("role"));
        assert_eq!(rows.text("asset-1", "source_id"), Some("warehouse"));
        assert_eq!(rows.text("asset-1", "source_revision"), Some("42"));
        assert_eq!(rows.text("asset-1", "clearance"), Some("internal"));
        assert_eq!(rows.text("role-1", "source_id"), Some("native"));
        assert_eq!(rows.text("role-1", "clearance"), Some(""));

        let both = run(&core, &["internal", "finance"], Some(CompanyCategory::DataAsset))
            .expect("scan");
        assert_eq!(both.ids(), vec!["asset-1", "asset-2"]);
        assert_eq!(both.text("asset-2", "source_revision"), Some("7"));
        assert_eq!(both.text("asset-2", "clearance"), Some("internal,finance"));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R078.7
    #[test]
    fn an_unverified_or_absent_scope_refuses_before_reading() {
        let core = graph();
        let view = core.analysis_snapshot();
        let semantic = SemanticStore::new();
        let absent = PlanCtx::new(&view, &semantic);
        assert!(company_context_scan(&absent, None)
            .unwrap_err()
            .starts_with("PLAN_SCOPE_UNVERIFIED"));
        let mut unverified = scope(&["internal"]);
        unverified.verified = false;
        let ctx = PlanCtx::new(&view, &semantic).with_plan_scope(unverified);
        assert!(company_context_scan(&ctx, None)
            .unwrap_err()
            .starts_with("PLAN_SCOPE_UNVERIFIED"));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R078.7
    #[test]
    fn a_category_whose_class_the_schema_lacks_refuses() {
        let core = graph();
        let mut view = core.analysis_snapshot();
        view.schema_sources = std::sync::Arc::new(GraphSchemaSources {
            core: BTreeMap::new(),
            dynamic: BTreeMap::new(),
        });
        let semantic = SemanticStore::new();
        let ctx = PlanCtx::new(&view, &semantic).with_plan_scope(scope(&["internal"]));
        let error = company_context_scan(&ctx, Some(CompanyCategory::Role)).unwrap_err();
        assert!(error.starts_with("COMPANY_MAPPING_MISSING"), "{error}");
    }

    // spec: MEDULLA-FEDERATED-QUERY-R078.7
    #[cfg(feature = "timeseries")]
    #[test]
    fn a_revision_without_a_commit_instant_refuses_the_time_series_read() {
        let core = GraphCore::new();
        core.add_node(
            "asset-ts".into(),
            blob(serde_json::json!({ "type": "DataAsset", "series_id": "s1" })),
        );
        let view = core.analysis_snapshot();
        let semantic = SemanticStore::new();
        let pinned = PlanCtx::new(&view, &semantic)
            .with_plan_scope(scope(&[]))
            .with_snapshot_revision(3)
            .with_now_ms(1_000);
        let error = company_context_scan(&pinned, Some(CompanyCategory::TimeSeries)).unwrap_err();
        assert!(error.starts_with("SNAPSHOT_TIME_UNKNOWN"), "{error}");
        let known = pinned.clone().with_snapshot_commit_ms(Some(500));
        assert!(company_context_scan(&known, Some(CompanyCategory::TimeSeries)).is_ok());
    }

    #[test]
    fn class_declaration_reads_prefixed_and_iri_subjects() {
        let mut core_sources = GraphSchemaSources::default();
        core_sources.core.clear();
        assert!(!class_declared(&core_sources, "Role"));
        assert!(class_declared(&GraphSchemaSources::default(), "Role"));
        assert!(class_declared(&GraphSchemaSources::default(), "SecurityClearance"));
        assert!(class_declared(&GraphSchemaSources::default(), "DataAsset"));
        assert!(class_declared(&GraphSchemaSources::default(), "Organization"));
    }
}
