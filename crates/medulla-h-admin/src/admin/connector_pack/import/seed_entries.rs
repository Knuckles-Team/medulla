//! The seed part entries of one pack import (MEDULLA-TYPED-PACKS-R104.3).
//!
//! Each seed part is one pack entry of kind `seed_part`; its URI (`seed://identity`,
//! `seed://catalog` or `seed://references`) names its [`SeedPartKey`]. The entries are
//! assembled into one [`SeedGraphImport`] and judged by `prepare_seed_import` against
//! the composition of core plus this pack's own ontology and shapes. A seed error is a
//! pack violation with its own code and the import commits nothing. The pack head's
//! seed members are the only durable seed state: the prior head's bodies are read back
//! through the blob store and canonicalized, so no seed table or graph read-back exists.

use medulla_catalog_types::connector_pack::{
    ConnectorPackImportRequest, PackEntry, PackEntryKind, PackViolation, PackViolationCode,
    SeedImportReceipt,
};
use medulla_persistence_records::connector_pack::ConnectorPackMemberRow;

use super::validation::violation;

#[cfg(all(feature = "rdf", feature = "shacl"))]
pub(super) fn validate_seed_entries(
    request: &ConnectorPackImportRequest,
    archive: &[u8],
    all: &[&PackEntry],
    prior: &[ConnectorPackMemberRow],
    blob: &dyn medulla_server_blob::blob::ChunkStore,
    violations: &mut Vec<PackViolation>,
) -> Option<SeedImportReceipt> {
    if !all.iter().any(|entry| entry.kind == PackEntryKind::SeedPart) {
        return None;
    }
    match plan_seed(request, archive, all, prior, blob) {
        Ok(receipt) => Some(receipt),
        Err(error) => {
            violations.push(seed_violation(&error));
            None
        }
    }
}

#[cfg(all(feature = "rdf", feature = "shacl"))]
/// A seed refusal's code is the first token of its error string; a code with no
/// pack counterpart (a Turtle parse failure) is `SEED_INVALID`.
fn seed_violation(error: &str) -> PackViolation {
    let (token, detail) = error.split_once(": ").unwrap_or(("SEED_INVALID", error));
    let code = match token {
        "SEED_TOO_LARGE" => PackViolationCode::SeedTooLarge,
        "SEED_BULK_DATA" => PackViolationCode::SeedBulkData,
        "SEED_SHACL_VIOLATION" => PackViolationCode::SeedShaclViolation,
        "SEED_DIGEST_MISMATCH" => PackViolationCode::SeedDigestMismatch,
        _ => PackViolationCode::SeedInvalid,
    };
    violation(code, None, detail)
}

#[cfg(all(feature = "rdf", feature = "shacl"))]
fn plan_seed(
    request: &ConnectorPackImportRequest,
    archive: &[u8],
    all: &[&PackEntry],
    prior: &[ConnectorPackMemberRow],
    blob: &dyn medulla_server_blob::blob::ChunkStore,
) -> Result<SeedImportReceipt, String> {
    use crate::admin::connector_pack::seed::{
        canonical_digest, current_seed_content, prepare_seed_import, seed_content_digest,
        SeedImportOutcome,
    };
    use medulla_catalog_types::connector_pack::{
        connector_instance_iri, seed_revision_iri, SeedGraphImport, SeedPart, SeedPartKey,
        SeedPartOutcome,
    };
    use std::collections::BTreeMap;

    use super::validation::section_bytes;

    let mut parts = BTreeMap::new();
    for entry in all.iter().filter(|e| e.kind == PackEntryKind::SeedPart) {
        let key = SeedPartKey::from_uri(&entry.uri).ok_or_else(|| {
            format!("SEED_INVALID: '{}' names no seed part (identity, catalog, references)", entry.uri)
        })?;
        if parts.contains_key(&key) {
            return Err(format!(
                "SEED_INVALID: more than one seed part entry for {}",
                key.token()
            ));
        }
        let bytes = section_bytes(archive, &entry.body)?;
        let turtle = std::str::from_utf8(bytes)
            .map_err(|_| "SEED_INVALID: a seed part is not UTF-8".to_string())?;
        parts.insert(
            key,
            SeedPart {
                key,
                turtle: turtle.to_string(),
                digest: canonical_digest(turtle)?,
            },
        );
    }
    let digests = parts
        .iter()
        .map(|(key, part)| (*key, part.digest))
        .collect::<BTreeMap<_, _>>();
    let seed = SeedGraphImport {
        connector: request.index.connector.clone(),
        instance_iri: connector_instance_iri(&request.index.connector),
        pack_digest: request.index.pack_digest,
        content_digest: seed_content_digest(&digests),
        parts,
    };

    // The prior head's seed members are the current seed content. A head that held a
    // different set of parts is replaced whole, so a dropped part never lingers.
    let mut prior_turtle = BTreeMap::new();
    for member in prior.iter().filter(|member| {
        member.kind == PackEntryKind::SeedPart
            && member.lifecycle
                == medulla_agent_types::agent_library::AgentLibraryLifecycle::Published
    }) {
        let Some(key) = SeedPartKey::from_uri(&member.uri) else {
            continue;
        };
        let body = medulla_server_blob::blob::engine_bodies::read_engine_body(
            blob,
            &request.context.tenant_id,
            &member.engine_manifest_digest,
            member.body_sha256,
            member.body_length,
        )?;
        let turtle = String::from_utf8(body)
            .map_err(|_| "SEED_INVALID: a stored seed part is not UTF-8".to_string())?;
        prior_turtle.insert(key, turtle);
    }
    let current = if !prior_turtle.is_empty()
        && prior_turtle.keys().eq(seed.parts.keys())
    {
        Some(current_seed_content(&prior_turtle)?)
    } else {
        None
    };

    let sources = super::vocabulary::pack_sources(request, archive, all)
        .map_err(|error| format!("SEED_INVALID: the pack's vocabulary does not compose: {error}"))?;
    let composed = medulla_server_api::schema_compose::validate_and_compose(&sources)
        .map_err(|error| format!("SEED_INVALID: the pack's vocabulary does not compose: {error}"))?;
    let plan = prepare_seed_import(&seed, current.as_ref(), &composed.shapes, &sources)?;
    let outcomes = seed
        .parts
        .keys()
        .map(|key| {
            let outcome = match &plan.outcome {
                SeedImportOutcome::Inserted => SeedPartOutcome::Inserted,
                SeedImportOutcome::Unchanged => SeedPartOutcome::Unchanged,
                SeedImportOutcome::Replaced { changed_parts } if changed_parts.contains(key) => {
                    SeedPartOutcome::Replaced
                }
                SeedImportOutcome::Replaced { .. } => SeedPartOutcome::Unchanged,
            };
            (*key, outcome)
        })
        .collect();
    Ok(SeedImportReceipt {
        connector: seed.connector.clone(),
        content_digest: plan.content.content_digest,
        outcomes,
        graph_revision: seed_revision_iri(seed.connector.as_str(), &plan.content.content_digest),
    })
}

#[cfg(not(all(feature = "rdf", feature = "shacl")))]
pub(super) fn validate_seed_entries(
    _request: &ConnectorPackImportRequest,
    _archive: &[u8],
    all: &[&PackEntry],
    _prior: &[ConnectorPackMemberRow],
    _blob: &dyn medulla_server_blob::blob::ChunkStore,
    violations: &mut Vec<PackViolation>,
) -> Option<SeedImportReceipt> {
    if all.iter().any(|entry| entry.kind == PackEntryKind::SeedPart) {
        violations.push(violation(
            PackViolationCode::SeedInvalid,
            None,
            "a seed graph needs the rdf and shacl features",
        ));
    }
    None
}
