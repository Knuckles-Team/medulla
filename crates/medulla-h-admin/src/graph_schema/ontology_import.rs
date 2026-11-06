//! `GraphSchema.ImportOntology` / `WithdrawOntology` (MEDULLA-TYPED-PACKS-R103.2
//! to R103.5): one connector-owned runtime ontology per
//! `ontology:<connector>/<ontology_iri>` key, revised in place.
//!
//! The persisted record is the [`SchemaSourceOrigin::Ontology`] source in the
//! same keyed schema-source set `AttachPack` writes: it carries the active
//! revision, the bound importer and a metadata-only revision history. Earlier
//! bodies are never kept, so only the active revision composes and there is no
//! second store. Both functions run inside the mutation gateway on the staged
//! [`GraphCore`]; a refusal returns before `install_schema_sources`, so no
//! refusal changes the composed schema.

use std::sync::Arc;

use medulla_catalog_types::graph_schema::{
    ontology_source_key, GraphSchemaCommitted, GraphSchemaOp, OntologyRevisionEntry,
    GRAPH_SCHEMA_RESULT_SCHEMA_VERSION, MAX_ONTOLOGY_REVISION_HISTORY,
};
use medulla_graph_core::graph::{
    GraphCore, GraphSchemaSource, SchemaSourceOrigin, MAX_SCHEMA_DOCUMENT_TRIPLES,
};
use medulla_types::contract::{Digest256, ServerErrorCode};

/// The verified authority one import or withdrawal runs under. `importer` is
/// the authenticated request principal; the request body carries none.
#[derive(Clone, Debug)]
pub(crate) struct OntologyAuthority {
    pub(crate) importer: String,
    pub(crate) tenant: String,
    /// Acceptance time, unix milliseconds, fixed before the gateway runs.
    pub(crate) at_ms: u64,
}

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

fn check_authority(authority: &OntologyAuthority) -> Result<(), String> {
    if authority.importer.is_empty() {
        return Err(refuse(
            ServerErrorCode::ImporterMismatch,
            "runtime ontology import needs an authenticated principal",
        ));
    }
    if medulla_types::contract::TenantId::new(&authority.tenant).is_err() {
        return Err(refuse(
            ServerErrorCode::TenantDenied,
            "runtime ontology write outside the caller's tenant",
        ));
    }
    Ok(())
}

/// Map a composition refusal onto the ontology-import vocabulary.
fn composition_refusal(error: String) -> String {
    let Some((code, detail)) = error.split_once(": ") else {
        return error;
    };
    match code {
        "SCHEMA_SOURCE_CONFLICT" => refuse(ServerErrorCode::OntologyCoreConflict, detail),
        "SCHEMA_SOURCE_REGRESSION" | "SHAPES_INVALID" => {
            refuse(ServerErrorCode::ShaclViolation, detail)
        }
        _ => error,
    }
}

fn parse_bounded(label: &str, document: &str) -> Result<(), String> {
    medulla_rdf::mapping::parse_turtle_bounded(document, MAX_SCHEMA_DOCUMENT_TRIPLES)
        .map(|_| ())
        .map_err(|error| format!("ONTOLOGY_INVALID: {label}: {error}"))
}

/// The prior runtime-ontology record under `source_id`, if any.
fn prior_record(
    core: &GraphCore,
    source_id: &str,
) -> Option<(u64, String, Vec<OntologyRevisionEntry>)> {
    match core
        .schema_sources()
        .dynamic
        .get(source_id)
        .map(|source| &source.origin)
    {
        Some(SchemaSourceOrigin::Ontology {
            revision,
            importer,
            history,
            ..
        }) => Some((*revision, importer.clone(), history.clone())),
        _ => None,
    }
}

fn committed(
    core: &GraphCore,
    graph_name: &str,
    composed: Digest256,
    changed: bool,
) -> GraphSchemaCommitted {
    GraphSchemaCommitted {
        schema_version: GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
        graph: graph_name.to_string(),
        composed_digest: composed.to_hex(),
        graph_version: core.version().saturating_add(u64::from(changed)),
        changed,
    }
}

/// Import or revise one runtime ontology (R103.2, refusals R103.4).
///
/// The same digest as the active revision answers that revision unchanged; a
/// new digest commits the next revision. The first accepted revision binds the
/// importer; any other principal is refused `IMPORTER_MISMATCH`.
pub(crate) fn import_ontology(
    core: &GraphCore,
    graph_name: &str,
    op: &GraphSchemaOp,
    authority: &OntologyAuthority,
) -> Result<GraphSchemaCommitted, String> {
    let GraphSchemaOp::ImportOntology {
        connector,
        ontology_iri,
        ontology_ttl,
        shapes_ttl,
        content_digest,
        expected_prior_revision,
        ..
    } = op
    else {
        return Err("import_ontology needs an ImportOntology operation".to_string());
    };
    check_authority(authority)?;
    op.validate()
        .map_err(|error| format!("ONTOLOGY_INVALID: {error}"))?;
    let digest = Digest256::sha256(ontology_ttl.as_bytes());
    if digest != *content_digest {
        return Err(refuse(
            ServerErrorCode::OntologyDigestMismatch,
            format!(
                "body digest {} does not match {}",
                digest.to_hex(),
                content_digest.to_hex()
            ),
        ));
    }
    parse_bounded("ontology_ttl", ontology_ttl)?;
    if let Some(shapes) = shapes_ttl.as_deref() {
        parse_bounded("shapes_ttl", shapes)?;
    }
    let source_id = ontology_source_key(connector, ontology_iri);
    let prior = prior_record(core, &source_id);
    if let Some((_, importer, _)) = &prior {
        if *importer != authority.importer {
            return Err(refuse(
                ServerErrorCode::ImporterMismatch,
                format!("'{source_id}' is bound to another importer"),
            ));
        }
    }
    let prior_revision = prior.as_ref().map_or(0, |(revision, _, _)| *revision);
    let sources = core.schema_sources();
    if let Some(current) = sources.dynamic.get(&source_id) {
        let same_shapes = current.shapes_sha256
            == shapes_ttl
                .as_deref()
                .map(|shapes| Digest256::sha256(shapes.as_bytes()));
        if current.ontology_sha256 == Some(digest) && same_shapes {
            return Ok(committed(
                core,
                graph_name,
                sources.composed_digest(),
                false,
            ));
        }
    }
    if let Some(expected) = expected_prior_revision {
        if *expected != prior_revision {
            return Err(refuse(
                ServerErrorCode::OntologyRevisionConflict,
                format!("expected revision {expected}, active revision {prior_revision}"),
            ));
        }
    }
    let revision = prior_revision + 1;
    let mut history = prior.map(|(_, _, history)| history).unwrap_or_default();
    history.push(OntologyRevisionEntry {
        digest,
        revision,
        importer: authority.importer.clone(),
        at: authority.at_ms,
    });
    if history.len() > MAX_ONTOLOGY_REVISION_HISTORY {
        let excess = history.len() - MAX_ONTOLOGY_REVISION_HISTORY;
        history.drain(..excess);
    }
    let source = GraphSchemaSource::new(
        SchemaSourceOrigin::Ontology {
            connector: connector.as_str().to_string(),
            ontology_iri: ontology_iri.clone(),
            revision,
            importer: authority.importer.clone(),
            history,
        },
        shapes_ttl.as_deref().map(Arc::from),
        Some(Arc::from(ontology_ttl.as_str())),
        authority.at_ms,
    )?;
    let mut staged = (*sources).clone();
    staged
        .attach_dynamic(source_id, source)
        .map_err(composition_refusal)?;
    medulla_server_api::schema_compose::validate_entering_schema(&staged)
        .map_err(composition_refusal)?;
    let composed = staged.composed_digest();
    core.install_schema_sources(Arc::new(staged));
    Ok(committed(core, graph_name, composed, true))
}

/// Withdraw one runtime ontology as a whole (R103.2): the source and its
/// record detach together. Only the bound importer may withdraw.
pub(crate) fn withdraw_ontology(
    core: &GraphCore,
    graph_name: &str,
    op: &GraphSchemaOp,
    authority: &OntologyAuthority,
) -> Result<GraphSchemaCommitted, String> {
    let GraphSchemaOp::WithdrawOntology {
        connector,
        ontology_iri,
        expected_revision,
    } = op
    else {
        return Err("withdraw_ontology needs a WithdrawOntology operation".to_string());
    };
    check_authority(authority)?;
    op.validate()
        .map_err(|error| format!("ONTOLOGY_INVALID: {error}"))?;
    let source_id = ontology_source_key(connector, ontology_iri);
    let Some((revision, importer, _)) = prior_record(core, &source_id) else {
        return Err(refuse(
            ServerErrorCode::OntologyRevisionConflict,
            format!("'{source_id}' has no active revision"),
        ));
    };
    if importer != authority.importer {
        return Err(refuse(
            ServerErrorCode::ImporterMismatch,
            format!("'{source_id}' is bound to another importer"),
        ));
    }
    if revision != *expected_revision {
        return Err(refuse(
            ServerErrorCode::OntologyRevisionConflict,
            format!("expected revision {expected_revision}, active revision {revision}"),
        ));
    }
    let mut staged = (*core.schema_sources()).clone();
    staged.detach_dynamic(&source_id);
    medulla_server_api::schema_compose::validate_and_compose(&staged)
        .map_err(composition_refusal)?;
    let composed = staged.composed_digest();
    core.install_schema_sources(Arc::new(staged));
    Ok(committed(core, graph_name, composed, true))
}

#[cfg(test)]
#[path = "ontology_import_tests.rs"]
mod tests;
