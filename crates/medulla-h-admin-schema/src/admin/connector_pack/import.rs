//! Closed, bounded ConnectorPack import validation and commit planning.

#[cfg(all(test, feature = "rdf"))]
use std::collections::BTreeSet;
use std::sync::Arc;

use medulla_catalog_types::{
    connector_pack::contract_version::PendingPackRelease,
    connector_pack::ConnectorPackImportRequest, connector_pack::PackImportResult,
};
#[cfg(test)]
use medulla_types::contract::Digest256;
#[cfg(test)]
use sha2::{Digest, Sha256};

use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::request_scope::interruptible;
use medulla_server_api::request_scope::RequestCancel;
use medulla_server_blob::blob::engine_bodies::EngineBody;
use medulla_server_blob::blob::BlobCursors;

#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod annotations;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod catalog_attributes;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod concept_path;
#[cfg(feature = "rdf")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod contract_rdf;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod crosswalk;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod desired;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod external_schema;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod facts;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod front_matter;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod json;
#[cfg(feature = "rdf")]
pub mod nutrition;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod planning;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod release;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod seed_entries;
pub mod skill_files;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod skill_graph;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod validation;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod vocabulary;

#[cfg(test)]
use json::{json_size, parse_bounded_json, MAX_JSON_DEPTH};
use planning::{prepare, Prepared};
use release::{ClassUsage, Verdict};
#[cfg(test)]
use validation::section_bytes;
#[cfg(test)]
use validation::{declared_shape_iris, valid_skill_frontmatter, validate_ontology_imports};

/// Validate, copy bodies and commit one import (EH-536).
///
/// The whole pipeline runs on ONE blocking thread that owns the tenant's pack
/// lock, so the lock is held exactly as long as the work runs -- never released
/// by a dropped future while a commit is still in progress -- and it runs
/// interruptibly: a cancelled request stops at its next reasoning step or stage
/// boundary, and dropping `tenant_pack_guard` then frees the tenant for the next
/// pack operation.
pub async fn validate_and_commit(
    store: Arc<AgentLibraryStore>,
    blob: Arc<BlobCursors>,
    _verified: &VerifiedRequestContext,
    request: ConnectorPackImportRequest,
    archive: Vec<u8>,
    tenant_pack_guard: tokio::sync::OwnedMutexGuard<()>,
    usage: Arc<dyn ClassUsage>,
) -> Result<PackImportResult, String> {
    let cancel = medulla_server_api::request_scope::current();
    let chunks = blob.store.clone();
    tokio::task::spawn_blocking(move || {
        let _held = tenant_pack_guard;
        run_import(&store, &*chunks, request, &archive, &cancel, &*usage, None)
    })
    .await
    .map_err(|error| format!("connector pack import task failed: {error}"))?
}

fn run_import(
    store: &AgentLibraryStore,
    chunks: &dyn medulla_server_blob::blob::ChunkStore,
    request: ConnectorPackImportRequest,
    archive: &[u8],
    cancel: &RequestCancel,
    usage: &dyn ClassUsage,
    approval: Option<&PendingPackRelease>,
) -> Result<PackImportResult, String> {
    let prior = store.connector_pack_members_with(
        &request.context.tenant_id,
        &request.index.connector,
        medulla_persistence_records::connector_pack::undecodable::UndecodableRows::Replace,
    )?;
    let prepared = interruptible(cancel, || {
        prepare(store, chunks, &request, archive, prior.clone())
    })??;
    let prepared = match prepared {
        Prepared::Rejected(result) => return Ok(result),
        Prepared::Ready(ready) => *ready,
    };
    // The release contract check runs before any body is copied or row written
    // (MEDULLA-TYPED-PACKS-R107.3): a breaking or in-use release stops here.
    let release = match interruptible(cancel, || {
        release::judge(
            store,
            chunks,
            &request,
            release::JudgeInputs {
                archive,
                ready: &prepared,
                prior_members: &prior,
            },
            usage,
            approval,
        )
    })?? {
        Verdict::Refused(result) => return Ok(result),
        Verdict::Apply(applied) => applied,
    };
    let bodies = prepared
        .body_inputs
        .iter()
        .map(|(sha256, body)| EngineBody {
            sha256: *sha256,
            body: body.clone(),
        })
        .collect::<Vec<_>>();
    still_wanted(cancel)?;
    let stored = chunks.put_engine_bodies(
        &request.context.tenant_id,
        &bodies,
        request.context.created_at_ms,
    )?;
    let plan = prepared.finish(request, stored, release)?;
    // The last point an abandoned import can stop: past it the commit is atomic.
    still_wanted(cancel)?;
    store.commit_connector_pack(plan)
}

/// Install the pack staged behind an applied change request: the staged import runs through the
/// one import path with the release approved, so the whole pack (members, ontology, shapes)
/// becomes the connector's head (MEDULLA-TYPED-PACKS-R107.6). `context` is the activation's
/// bound mutation context.
pub fn commit_staged(
    store: &AgentLibraryStore,
    chunks: &dyn medulla_server_blob::blob::ChunkStore,
    context: medulla_agent_types::agent_library::AgentLibraryMutationContext,
    held: &PendingPackRelease,
    cancel: &RequestCancel,
    usage: &dyn ClassUsage,
) -> Result<PackImportResult, String> {
    let staged = held.staged.as_ref().ok_or_else(|| {
        "PACK_RELEASE_NOT_HELD: the held release staged no pack contents".to_string()
    })?;
    let (mut request, archive) = release::read_staged(chunks, &context.tenant_id, staged)?;
    request.context = context;
    run_import(store, chunks, request, &archive, cancel, usage, Some(held))
}

fn still_wanted(cancel: &RequestCancel) -> Result<(), String> {
    if cancel.is_cancelled() {
        return Err(medulla_server_api::request_scope::CANCELLED.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-TYPED-PACKS-R001.2
    #[test]
    fn section_ranges_are_checked_without_wrapping() {
        let bytes = b"abcdef";
        let valid = medulla_catalog_types::connector_pack::PackSection {
            offset: 1,
            length: 3,
            sha256: Digest256::from_bytes(Sha256::digest(b"bcd").into()),
        };
        assert_eq!(section_bytes(bytes, &valid).unwrap(), b"bcd");
        let overflow = medulla_catalog_types::connector_pack::PackSection {
            offset: u64::MAX,
            length: 2,
            sha256: valid.sha256,
        };
        assert!(section_bytes(bytes, &overflow).is_err());
    }

    #[test]
    fn skill_frontmatter_refuses_yaml_expansion_features() {
        assert!(valid_skill_frontmatter(
            b"---\nname: demo\ndescription: bounded skill\n---\n# Demo\n",
            "demo"
        ));
        assert!(!valid_skill_frontmatter(
            b"---\nname: demo\ndescription: &shared unsafe\n---\n",
            "demo"
        ));
        assert!(!valid_skill_frontmatter(
            b"---\nname: another\ndescription: bounded skill\n---\n",
            "demo"
        ));
    }

    #[test]
    fn json_budget_rejects_excess_depth() {
        let mut value = serde_json::Value::Null;
        for _ in 0..=MAX_JSON_DEPTH {
            value = serde_json::Value::Array(vec![value]);
        }
        assert!(json_size(&value, 0).is_err());
        assert!(parse_bounded_json(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse_bounded_json(br#"{"a":1,"nested":{"a":2}}"#).is_ok());
    }

    #[cfg(feature = "rdf")]
    #[test]
    fn ontology_imports_are_closed_over_pack_entries() {
        let allowed = BTreeSet::from(["ontology://demo/base.ttl"]);
        let accepted = "@prefix owl: <http://www.w3.org/2002/07/owl#> . <urn:demo> owl:imports <ontology://demo/base.ttl> .";
        let refused = "@prefix owl: <http://www.w3.org/2002/07/owl#> . <urn:demo> owl:imports <https://remote.example/ontology.ttl> .";
        assert!(
            validate_ontology_imports(accepted, "ontology://demo/current.ttl", &allowed).is_ok()
        );
        assert!(
            validate_ontology_imports(refused, "ontology://demo/current.ttl", &allowed).is_err()
        );
    }

    #[cfg(feature = "rdf")]
    #[test]
    fn duplicate_named_shapes_can_be_detected_across_files() {
        let ttl = "@prefix sh: <http://www.w3.org/ns/shacl#> . <urn:shape> a sh:NodeShape .";
        assert_eq!(
            declared_shape_iris(ttl),
            BTreeSet::from(["urn:shape".to_string()])
        );
    }
}
