//! Served `GraphSchema.DiffOntology` and the impact an import records
//! (MEDULLA-TYPED-PACKS-R804.3).
//!
//! The diff reads the active revision of one connector-owned ontology and
//! compares it with a candidate document or the active revision itself; it
//! writes nothing. An import calls [`assess_import`] before it commits, keeps
//! the counts on the new revision record and, when any count is above zero,
//! opens one `ontology.breaking_change` change request under the importer's
//! tenant. The import still commits.

use medulla_catalog_types::graph_schema::{
    ontology_source_key, DiffTarget, GraphSchemaCommitted, GraphSchemaOp,
    GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
};
use medulla_core::stewardship::{open_change_request, TargetScopes};
use medulla_graph_core::graph::{GraphCore, SchemaSourceOrigin};
use medulla_types::contract::{Digest256, ServerErrorCode, TenantId};
use medulla_types::ontology_diff::{ChangeImpact, ImpactItems, OntologyDiffReport};
use medulla_types::stewardship::{StewardRole, StewardshipAssignment, StewardshipScope};
use serde_json::{json, Value};

use super::edits;
use super::ontology_diff::{diff_axioms, impact, snapshot_from_core, SchemaSide};

/// The kind a breaking-change request carries.
pub(super) const BREAKING_CHANGE_KIND: &str = "ontology.breaking_change";
const CHANGE_REQUEST_LABEL: &str = "ChangeRequest";

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// The active revision of `key` as a diff side, if `revision` names it.
fn active_side(core: &GraphCore, key: &str, revision: u64) -> Result<SchemaSide, String> {
    let sources = core.schema_sources();
    let source = sources.dynamic.get(key);
    match source.map(|source| (&source.origin, source)) {
        Some((
            SchemaSourceOrigin::Ontology {
                revision: active, ..
            },
            source,
        )) if *active == revision => Ok(SchemaSide {
            revision: revision.to_string(),
            ontology_ttl: source
                .ontology_ttl
                .as_deref()
                .unwrap_or_default()
                .to_string(),
            shapes_ttl: source.shapes_ttl.as_deref().map(str::to_string),
        }),
        Some((
            SchemaSourceOrigin::Ontology {
                revision: active, ..
            },
            _,
        )) => Err(refuse(
            ServerErrorCode::OntologyRevisionConflict,
            format!("revision {revision} is not stored; the active revision is {active}"),
        )),
        _ => Err(refuse(
            ServerErrorCode::OntologyRevisionConflict,
            format!("'{key}' has no active revision"),
        )),
    }
}

fn committed(
    core: &GraphCore,
    graph_name: &str,
    report: OntologyDiffReport,
) -> GraphSchemaCommitted {
    GraphSchemaCommitted {
        schema_version: GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
        graph: graph_name.to_string(),
        composed_digest: core.schema_sources().composed_digest().to_hex(),
        graph_version: core.version(),
        changed: false,
        edit_head: edits::head(core),
        ontology_diff: Some(report),
    }
}

/// `DiffOntology`: the axiom diff and the impact of moving the graph to `to`.
/// Reads only; the staged core is left as it was.
pub(super) fn diff_ontology(
    core: &GraphCore,
    graph_name: &str,
    tenant: &str,
    op: &GraphSchemaOp,
) -> Result<GraphSchemaCommitted, String> {
    let GraphSchemaOp::DiffOntology {
        connector,
        ontology_iri,
        from_revision,
        to,
    } = op
    else {
        return Err("diff_ontology needs a DiffOntology operation".to_string());
    };
    if TenantId::new(tenant).is_err() {
        return Err(refuse(
            ServerErrorCode::TenantDenied,
            "ontology diff outside the caller's tenant",
        ));
    }
    op.validate()
        .map_err(|error| format!("ONTOLOGY_INVALID: {error}"))?;
    let key = ontology_source_key(connector, ontology_iri);
    let from = active_side(core, &key, *from_revision)?;
    let target = match to {
        DiffTarget::Revision(revision) => active_side(core, &key, *revision)?,
        DiffTarget::Candidate { turtle } => SchemaSide {
            revision: "candidate".to_string(),
            ontology_ttl: turtle.clone(),
            shapes_ttl: from.shapes_ttl.clone(),
        },
    };
    let diff = diff_axioms(&from, &target)?;
    let mut snapshot = snapshot_from_core(core, &key)?;
    snapshot.compose = Some(((*core.schema_sources()).clone(), key));
    let (counts, items) = impact(&from, &target, &snapshot)?;
    Ok(committed(
        core,
        graph_name,
        OntologyDiffReport {
            diff,
            impact: counts,
            items,
        },
    ))
}

/// The impact of importing `ontology_ttl` as the next revision of `key`,
/// computed before the import commits. The first revision is measured against
/// an empty ontology.
pub(super) fn assess_import(
    core: &GraphCore,
    key: &str,
    revision: u64,
    ontology_ttl: &str,
    shapes_ttl: Option<&str>,
) -> Result<Option<(ChangeImpact, ImpactItems)>, String> {
    let from = active_side(core, key, revision.saturating_sub(1)).unwrap_or(SchemaSide {
        revision: "0".to_string(),
        ontology_ttl: String::new(),
        shapes_ttl: None,
    });
    let to = SchemaSide {
        revision: revision.to_string(),
        ontology_ttl: ontology_ttl.to_string(),
        shapes_ttl: shapes_ttl.map(str::to_string),
    };
    let snapshot = snapshot_from_core(core, key)?;
    impact(&from, &to, &snapshot).map(Some)
}

/// Whether any count of `impact` is above zero.
pub(super) fn is_breaking(impact: &ChangeImpact) -> bool {
    impact.instances_invalid + impact.shapes_broken + impact.mappings_broken + impact.queries_broken
        > 0
}

/// Open the `ontology.breaking_change` request for one committed revision.
/// The bound importer owns the ontology and is its reviewer. The node is
/// keyed by tenant, so another tenant never reads it.
pub(super) fn open_breaking_change(
    core: &GraphCore,
    tenant: &str,
    importer: &str,
    source_id: &str,
    revision: u64,
    digest: &Digest256,
    counts: &ChangeImpact,
) -> Result<(), String> {
    let owner = StewardshipAssignment {
        scope: StewardshipScope::Asset(source_id.to_string()),
        role: StewardRole::Owner,
        principal: importer.to_string(),
    };
    let scopes = TargetScopes {
        asset: Some(source_id.to_string()),
        concept_scheme: None,
        domain: None,
    };
    let proposed = format!("sha256:{}", digest.to_hex());
    let request = open_change_request(source_id, &proposed, &[owner], &scopes)
        .map_err(|error| format!("CHANGE_REQUEST_INVALID: {error}"))?;
    let mut value = serde_json::to_value(&request).map_err(|error| error.to_string())?;
    value["kind"] = json!(BREAKING_CHANGE_KIND);
    value["tenant"] = json!(tenant);
    value["revision"] = json!(revision);
    value["impact"] = serde_json::to_value(counts).map_err(|error| error.to_string())?;
    edits::put(
        core,
        format!("change-request:{tenant}:{source_id}:{revision}"),
        CHANGE_REQUEST_LABEL,
        value,
    )
}

/// The change requests `tenant` holds, oldest key first.
pub(super) fn change_requests_of(core: &GraphCore, tenant: &str) -> Vec<Value> {
    let mut found: Vec<(String, Value)> = core
        .get_nodes_by_label(CHANGE_REQUEST_LABEL, 0)
        .into_iter()
        .filter_map(|(id, bytes)| Some((id, rmp_serde::from_slice::<Value>(&bytes).ok()?)))
        .filter(|(_, value)| value["tenant"].as_str() == Some(tenant))
        .collect();
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found.into_iter().map(|(_, value)| value).collect()
}
