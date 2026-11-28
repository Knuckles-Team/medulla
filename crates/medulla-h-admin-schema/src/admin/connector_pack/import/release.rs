//! The release contract check of one import (MEDULLA-TYPED-PACKS-R107.3, R107.4).
//!
//! After planning accepts a pack and before anything commits, the release the
//! pack would activate is diffed against the release the connector's head
//! activated. An additive release goes on to the commit and records itself as
//! the new active summary. A breaking release is held on the head row behind a
//! change request and the import answers `CONTRACT_BREAK_PENDING`; a release
//! that withdraws a class stored data still names answers `CLASS_IN_USE`. In
//! both refusals the head, and so the active version, is untouched.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use medulla_catalog_types::connector_pack::contract_version::{
    compat_verdict, CompatVerdict, ContractBreak, ContractBreakKind, NestedSchema,
    PackContractVersion, PackRelease, PendingPackRelease, SchemaCompatRecord, SchemaDigest,
    StagedPack, StagedPart,
};
use medulla_catalog_types::connector_pack::{
    ChangeRequestRef, ConnectorPackImportRequest, PackEntryKind, PackImportResult,
    PackReleaseEvidence, PackViolation, PackViolationCode,
};
use medulla_core::stewardship::TargetScopes;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::connector_pack::ConnectorPackMemberRow;
use medulla_types::contract::{BoundedVec, Digest256, ResourceId};
use tokio::sync::RwLock;
use {
    medulla_catalog_source_types::stewardship::StewardRole,
    medulla_catalog_source_types::stewardship::StewardshipAssignment,
    medulla_catalog_source_types::stewardship::StewardshipScope,
};

use super::planning::ReadyPlan;
use super::validation::section_bytes;
use crate::admin::connector_pack::contract_diff::{
    diff_release, ContractHistory, ReleaseContract, ReleaseDisposition,
};
use medulla_server_api::state::ServerState;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
/// Most violations one `Rejected` result carries.
const MAX_REPORTED: usize = 256;

/// Which of `classes` stored data still names, for the importing tenant's
/// graphs that compose a connector's current head.
pub trait ClassUsage: Send + Sync {
    fn classes_in_use(
        &self,
        tenant: &str,
        connector: &ResourceId,
        record_id: &str,
        classes: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, String>;
}

/// The served implementation (DECISIONS PACK-CLASS-IN-USE-IS-SCOPED-AND-INDEXED).
///
/// A graph is a candidate only when its recorded `pack:<connector>` schema
/// source was composed from `record_id` for the importing tenant: a resident
/// graph answers that from its installed sources, a catalog-only graph from its
/// durable catalog row. No graph is opened. Another tenant's graph is never a
/// candidate, so its data never holds back this tenant's release.
///
/// A fully resident candidate answers from its `type` label index (the
/// `rdf:type` fold of `medulla_rdf::mapping`), and for a resource typed more
/// than once from the class node's incoming typing edges. A candidate that is
/// not resident, or whose material is hibernated or incomplete, answers from
/// the durable node table by the class's node id: a class node stored there
/// counts as named.
///
/// `classes_in_use` runs on the import's blocking thread (`validate_and_commit`
/// runs `run_import` under `spawn_blocking`), so it takes the state lock with
/// `blocking_read`, holds it only to copy the candidate list, and never enters
/// the async runtime.
pub struct GraphClassUsage {
    pub state: Arc<RwLock<ServerState>>,
}

/// One registered graph the check may read.
struct UsageCandidate {
    name: String,
    /// The resident core, `None` for a catalog-only graph.
    core: Option<Arc<medulla_graph_core::graph::GraphCore>>,
    /// Whether the resident core's material is complete.
    complete: bool,
}

impl GraphClassUsage {
    fn candidates(
        &self,
    ) -> (
        Vec<UsageCandidate>,
        Option<Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>>,
    ) {
        let state = self.state.blocking_read();
        let candidates = state
            .registry
            .list()
            .into_iter()
            .map(|(name, _)| {
                let core = state.registry.get(&name).map(|entry| entry.core.clone());
                let complete = core.is_some()
                    && state
                        .registry
                        .materialization_handle(&name)
                        .is_none_or(|manifest| manifest.read().is_ok_and(|m| m.valid()));
                UsageCandidate {
                    name,
                    core,
                    complete,
                }
            })
            .collect();
        (candidates, state.persistence.clone())
    }
}

/// Whether `sources` holds `pack:<connector>` composed from `record_id` for
/// `tenant`. A source with no recorded tenant counts for every tenant.
fn composes_tenant_pack(
    sources: &medulla_graph_core::graph::GraphSchemaSources,
    source_id: &str,
    tenant: &str,
    record_id: &str,
) -> bool {
    sources.dynamic.get(source_id).is_some_and(|source| {
        matches!(
            &source.origin,
            medulla_graph_core::graph::SchemaSourceOrigin::Pack {
                record_id: composed,
                tenant: recorded,
                ..
            } if composed == record_id
                && recorded.as_deref().is_none_or(|recorded| recorded == tenant)
        )
    })
}

/// The canonical node id of an IRI resource (`medulla_rdf::mapping`).
fn class_node_id(class: &str) -> String {
    format!("<{class}>")
}

/// The classes a resident core's data types an instance with: the `type`
/// label index first, then the class node's incoming typing edges for a
/// resource whose folded `type` is another of its classes.
fn resident_typed(
    core: &medulla_graph_core::graph::GraphCore,
    pending: &[&String],
) -> Result<Vec<String>, String> {
    let mut typed = Vec::new();
    for class in pending {
        if !core.get_nodes_by_label(class, 1).is_empty() {
            typed.push((*class).clone());
            continue;
        }
        let node = class_node_id(class);
        if !core.has_node(&node) {
            continue;
        }
        let typing = core.get_predecessors(&node)?.iter().any(|subject| {
            core.get_edge_properties(subject, &node).iter().any(|blob| {
                medulla_types::msgpack::decode_edge_relationship(blob).as_deref() == Some(RDF_TYPE)
            })
        });
        if typing {
            typed.push((*class).clone());
        }
    }
    Ok(typed)
}

/// The classes whose node the durable node table of `graph_fname` holds.
fn durable_named(
    backend: &dyn medulla_server_api::api::persistence::PersistenceBackend,
    graph_fname: &str,
    pending: &[&String],
) -> Result<Vec<String>, String> {
    let ids: Vec<String> = pending.iter().map(|class| class_node_id(class)).collect();
    let present = backend.durable_node_presence(graph_fname, &ids)?;
    if present.len() != ids.len() {
        return Err(format!(
            "durable node presence for {graph_fname} answered {} of {} class nodes",
            present.len(),
            ids.len()
        ));
    }
    Ok(pending
        .iter()
        .zip(present)
        .filter(|(_, present)| *present)
        .map(|(class, _)| (*class).clone())
        .collect())
}

impl ClassUsage for GraphClassUsage {
    fn classes_in_use(
        &self,
        tenant: &str,
        connector: &ResourceId,
        record_id: &str,
        classes: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, String> {
        let source_id = format!("pack:{}", connector.as_str());
        let (candidates, persistence) = self.candidates();
        let mut found = BTreeSet::new();
        for candidate in candidates {
            let pending: Vec<&String> = classes.iter().filter(|c| !found.contains(*c)).collect();
            if pending.is_empty() {
                break;
            }
            let graph_fname = medulla_kernel::persist::sanitize(&candidate.name);
            let sources = match (&candidate.core, &persistence) {
                (Some(core), _) => core.schema_sources(),
                (None, Some(backend)) => {
                    match backend.graph_catalog_schema_sources(&graph_fname)? {
                        Some(sources) => sources,
                        None => continue,
                    }
                }
                // Without a durable catalog every registered graph is resident.
                (None, None) => continue,
            };
            if !composes_tenant_pack(&sources, &source_id, tenant, record_id) {
                continue;
            }
            let typed = match (&candidate.core, &persistence) {
                (Some(core), _) if candidate.complete && core.node_count() > 0 => {
                    resident_typed(core, &pending)?
                }
                (_, Some(backend)) => durable_named(backend.as_ref(), &graph_fname, &pending)?,
                (Some(core), None) => resident_typed(core, &pending)?,
                (None, None) => Vec::new(),
            };
            found.extend(typed);
        }
        Ok(found)
    }
}

/// The release an import activates and the diff its answer reports
/// (MEDULLA-TYPED-PACKS-R107.5).
pub struct AppliedRelease {
    pub release: PackRelease,
    pub evidence: PackReleaseEvidence,
}

/// What the check decided for one import.
pub enum Verdict {
    /// Commit, recording this release as the head's active summary.
    Apply(AppliedRelease),
    /// Answer with this result; nothing else is written for the head's move.
    Refused(PackImportResult),
}

fn framed(parts: &[&[u8]]) -> Result<String, String> {
    Digest256::framed(b"eg/connector-pack-release/v1", parts)
        .map(|digest| format!("sha256:{}", digest.to_hex()))
}

fn joined<'a>(documents: impl Iterator<Item = &'a str>) -> String {
    documents.collect::<Vec<_>>().join("\n")
}

/// The tool field paths and declared types of the release being imported.
fn next_fields(
    request: &ConnectorPackImportRequest,
    archive: &[u8],
) -> Result<BTreeMap<String, String>, String> {
    let mut fields = BTreeMap::new();
    for entry in request.index.entries.iter() {
        if entry.kind != PackEntryKind::Tool {
            continue;
        }
        let Some(section) = &entry.input_schema else {
            continue;
        };
        let schema: serde_json::Value = serde_json::from_slice(section_bytes(archive, section)?)
            .map_err(|error| format!("MALFORMED_BODY: tool input_schema is not JSON: {error}"))?;
        let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
            continue;
        };
        for (name, declared) in properties {
            let declared_type = declared
                .get("type")
                .map(serde_json::Value::to_string)
                .unwrap_or_default();
            fields.insert(format!("{}#/{name}", entry.uri), declared_type);
        }
    }
    Ok(fields)
}

fn next_mappings(ready: &ReadyPlan, kept: &BTreeSet<&str>) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for member in ready
        .members()
        .iter()
        .filter(|m| kept.contains(m.uri.as_str()))
    {
        ids.extend(
            member
                .schema_mappings
                .iter()
                .flat_map(|maps| maps.keys())
                .map(|id| format!("schema:{id}")),
        );
        ids.extend(
            member
                .relationship_mappings
                .iter()
                .flat_map(|maps| maps.keys())
                .map(|id| format!("relationship:{id}")),
        );
    }
    ids
}

/// The ontology and shapes documents of the head's published members.
fn prior_documents(
    chunks: &dyn medulla_server_blob::blob::ChunkStore,
    tenant: &str,
    members: &[ConnectorPackMemberRow],
) -> Result<(String, Option<String>), String> {
    let read = |kind: PackEntryKind| -> Result<Option<String>, String> {
        let mut picked: Vec<&ConnectorPackMemberRow> = members
            .iter()
            .filter(|m| {
                m.kind == kind
                    && m.lifecycle
                        == medulla_agent_types::agent_library::AgentLibraryLifecycle::Published
            })
            .collect();
        picked.sort_by(|a, b| a.uri.cmp(&b.uri));
        if picked.is_empty() {
            return Ok(None);
        }
        let mut documents = Vec::with_capacity(picked.len());
        for member in picked {
            let bytes = medulla_server_blob::blob::engine_bodies::read_engine_body(
                chunks,
                tenant,
                &member.engine_manifest_digest,
                member.body_sha256,
                member.body_length,
            )?;
            documents
                .push(String::from_utf8(bytes).map_err(|_| {
                    "MALFORMED_BODY: stored pack document is not UTF-8".to_string()
                })?);
        }
        Ok(Some(joined(documents.iter().map(String::as_str))))
    };
    let ontology = read(PackEntryKind::Ontology)?.unwrap_or_default();
    let shapes = read(PackEntryKind::Shapes)?;
    Ok((ontology, shapes))
}

fn next_contract(
    request: &ConnectorPackImportRequest,
    archive: &[u8],
    ready: &ReadyPlan,
) -> Result<ReleaseContract, String> {
    let documents = |kind: PackEntryKind| -> Result<Option<String>, String> {
        let mut picked: Vec<_> = request
            .index
            .entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .collect();
        picked.sort_by(|a, b| a.uri.cmp(&b.uri));
        if picked.is_empty() {
            return Ok(None);
        }
        let mut texts = Vec::with_capacity(picked.len());
        for entry in picked {
            texts.push(
                std::str::from_utf8(section_bytes(archive, &entry.body)?)
                    .map_err(|_| "MALFORMED_BODY: pack document is not UTF-8".to_string())?,
            );
        }
        Ok(Some(joined(texts.into_iter())))
    };
    let ontology_ttl = documents(PackEntryKind::Ontology)?.unwrap_or_default();
    let shapes_ttl = documents(PackEntryKind::Shapes)?;
    let kept: BTreeSet<&str> = request
        .index
        .entries
        .iter()
        .map(|entry| entry.uri.as_str())
        .collect();
    let fields = next_fields(request, archive)?;
    let mappings = next_mappings(ready, &kept);
    let fields_json = serde_json::to_vec(&fields).map_err(|error| error.to_string())?;
    let mappings_json = serde_json::to_vec(&mappings).map_err(|error| error.to_string())?;
    let shapes_text = shapes_ttl.as_deref().unwrap_or_default();
    let version = PackContractVersion {
        connector: request.index.connector.as_str().to_string(),
        version: ready.record_id().to_string(),
        contract_digest: framed(&[
            ontology_ttl.as_bytes(),
            shapes_text.as_bytes(),
            &fields_json,
            &mappings_json,
        ])?,
        ontology_digest: framed(&[ontology_ttl.as_bytes()])?,
        shapes_digest: framed(&[shapes_text.as_bytes()])?,
        supersedes: None,
    };
    Ok(ReleaseContract {
        version,
        ontology_ttl,
        shapes_ttl,
        fields,
        mappings,
    })
}

fn release_of(contract: &ReleaseContract, schemas: BTreeMap<String, SchemaDigest>) -> PackRelease {
    PackRelease {
        version: contract.version.clone(),
        fields: contract.fields.clone(),
        mappings: contract.mappings.clone(),
        schemas,
    }
}

/// The nested schema every entry declares, keyed by entry URI plus `#in` or
/// `#out` (MEDULLA-TYPED-PACKS-R001.100).
fn next_schemas(
    request: &ConnectorPackImportRequest,
    archive: &[u8],
) -> Result<BTreeMap<String, SchemaDigest>, String> {
    let mut schemas = BTreeMap::new();
    for entry in request.index.entries.iter() {
        for (suffix, section) in [("#in", &entry.input_schema), ("#out", &entry.output_schema)] {
            let Some(section) = section else {
                continue;
            };
            let document: serde_json::Value =
                serde_json::from_slice(section_bytes(archive, section)?)
                    .map_err(|error| format!("MALFORMED_BODY: schema is not JSON: {error}"))?;
            let schema = NestedSchema::from_document(&document)
                .map_err(|error| format!("MALFORMED_BODY: schema evolution members: {error}"))?;
            schemas.insert(
                format!("{}{suffix}", entry.uri),
                SchemaDigest {
                    digest: section.sha256,
                    schema,
                    compat: None,
                },
            );
        }
    }
    Ok(schemas)
}

/// Judge each next schema against the one the active release recorded for the
/// same entry. A schema that names a base the active release no longer holds
/// is stale; a breaking one is incompatible; any other records its verdict.
fn judge_schemas(
    active: &PackRelease,
    next: &mut BTreeMap<String, SchemaDigest>,
) -> Vec<PackViolation> {
    let mut found = Vec::new();
    for (key, schema) in next.iter_mut() {
        let uri = key.rsplit_once('#').map_or(key.as_str(), |(uri, _)| uri);
        let base = active.schemas.get(key);
        if let Some(declared) = schema.schema.base_digest {
            if base.map(|held| held.digest) != Some(declared) {
                found.push(violation(
                    PackViolationCode::PackPlanStale,
                    Some(uri.to_string()),
                    "the schema names a base digest the active release does not hold".to_string(),
                ));
                continue;
            }
        }
        let Some(base) = base else {
            continue;
        };
        if base.digest == schema.digest {
            schema.compat = base.compat.clone();
            continue;
        }
        let (verdict, changed) = compat_verdict(base, &schema.schema);
        if verdict == CompatVerdict::Breaking {
            let shown: Vec<&str> = changed.iter().take(8).map(String::as_str).collect();
            found.push(violation(
                PackViolationCode::PackSchemaIncompatible,
                Some(uri.to_string()),
                format!("incompatible schema change: {}", shown.join(", ")),
            ));
        } else {
            schema.compat = Some(SchemaCompatRecord {
                verdict,
                base_digest: base.digest,
                changed_paths: changed,
            });
        }
    }
    found
}

/// Stage the held pack: the import request and the archive as one byte stream in the engine
/// body store, in bodies of at most the component body bound, so activation installs the whole
/// pack (MEDULLA-TYPED-PACKS-R107.6). The request's own archive blob is caller-owned and is not
/// relied on.
fn stage_pack(
    chunks: &dyn medulla_server_blob::blob::ChunkStore,
    request: &ConnectorPackImportRequest,
    archive: &[u8],
) -> Result<StagedPack, String> {
    use medulla_server_blob::blob::engine_bodies::EngineBody;
    use sha2::{Digest, Sha256};

    let json = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    let mut stream = Vec::with_capacity(8 + json.len() + archive.len());
    stream.extend_from_slice(&(json.len() as u64).to_be_bytes());
    stream.extend_from_slice(&json);
    stream.extend_from_slice(archive);
    let mut unique: BTreeMap<Digest256, Vec<u8>> = BTreeMap::new();
    let mut order = Vec::new();
    for part in stream.chunks(medulla_agent_types::agent_component::MAX_COMPONENT_BODY_BYTES) {
        let sha256 = Digest256::from_bytes(Sha256::digest(part).into());
        unique.entry(sha256).or_insert_with(|| part.to_vec());
        order.push(sha256);
    }
    let bodies: Vec<EngineBody> = unique
        .into_iter()
        .map(|(sha256, body)| EngineBody { sha256, body })
        .collect();
    let stored = chunks.put_engine_bodies(
        &request.context.tenant_id,
        &bodies,
        request.context.created_at_ms,
    )?;
    let parts = order
        .into_iter()
        .map(|sha256| {
            stored
                .iter()
                .find(|body| body.sha256 == sha256)
                .map(|body| StagedPart {
                    sha256,
                    manifest_digest: body.manifest_digest.clone(),
                    length: body.length,
                })
                .ok_or_else(|| "BODY_MISSING: a staged pack body was not stored".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StagedPack { parts })
}

/// Read a staged pack back: the import request it was staged with and the archive bytes.
pub fn read_staged(
    chunks: &dyn medulla_server_blob::blob::ChunkStore,
    tenant: &str,
    staged: &StagedPack,
) -> Result<(ConnectorPackImportRequest, Vec<u8>), String> {
    let mut stream = Vec::new();
    for part in &staged.parts {
        stream.extend(medulla_server_blob::blob::engine_bodies::read_engine_body(
            chunks,
            tenant,
            &part.manifest_digest,
            part.sha256,
            part.length,
        )?);
    }
    let corrupt = || "CORRUPT_CONNECTOR_PACK: the staged pack stream is malformed".to_string();
    let head: [u8; 8] = stream
        .get(..8)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(corrupt)?;
    let length = usize::try_from(u64::from_be_bytes(head)).map_err(|_| corrupt())?;
    let json = stream
        .get(8..8usize.saturating_add(length))
        .ok_or_else(corrupt)?;
    let request = serde_json::from_slice(json).map_err(|_| corrupt())?;
    let archive = stream[8 + length..].to_vec();
    Ok((request, archive))
}

fn violation(code: PackViolationCode, uri: Option<String>, detail: String) -> PackViolation {
    PackViolation {
        code,
        uri,
        detail: detail.chars().take(1024).collect(),
    }
}

fn refusal(
    request: &ConnectorPackImportRequest,
    violations: Vec<PackViolation>,
    budget_exhausted: bool,
    held: Option<PackReleaseEvidence>,
) -> Result<Verdict, String> {
    Ok(Verdict::Refused(PackImportResult::Rejected {
        pack_digest: Some(request.index.pack_digest),
        violations: BoundedVec::new(violations)?,
        budget_exhausted,
        held,
    }))
}

fn kind_name(kind: ContractBreakKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The archive, the prepared plan and the head's members one release judgement reads.
pub struct JudgeInputs<'a> {
    pub archive: &'a [u8],
    pub ready: &'a ReadyPlan,
    pub prior_members: &'a [ConnectorPackMemberRow],
}

/// Diff the release `ready` would activate against the head's active release.
pub fn judge(
    store: &AgentLibraryStore,
    chunks: &dyn medulla_server_blob::blob::ChunkStore,
    request: &ConnectorPackImportRequest,
    inputs: JudgeInputs<'_>,
    usage: &dyn ClassUsage,
    approval: Option<&PendingPackRelease>,
) -> Result<Verdict, String> {
    let JudgeInputs {
        archive,
        ready,
        prior_members,
    } = inputs;
    let tenant = request.context.tenant_id.as_str();
    let connector = &request.index.connector;
    let mut next = next_contract(request, archive, ready)?;
    let mut schemas = next_schemas(request, archive)?;
    let Some(active) = store.connector_pack_release(tenant, connector)? else {
        let evidence = PackReleaseEvidence {
            previous_contract_digest: None,
            contract_digest: next.version.contract_digest.clone(),
            break_count: 0,
            change_request: None,
        };
        return Ok(Verdict::Apply(AppliedRelease {
            release: release_of(&next, schemas),
            evidence,
        }));
    };
    let schema_violations = judge_schemas(&active, &mut schemas);
    if !schema_violations.is_empty() {
        let over = schema_violations.len() > MAX_REPORTED;
        let shown = schema_violations.into_iter().take(MAX_REPORTED).collect();
        return refusal(request, shown, over, None);
    }
    let (ontology_ttl, shapes_ttl) = prior_documents(chunks, tenant, prior_members)?;
    let kept_tools: BTreeSet<&str> = request
        .index
        .entries
        .iter()
        .filter(|entry| entry.kind == PackEntryKind::Tool)
        .map(|entry| entry.uri.as_str())
        .collect();
    // A tool removed as a whole is a pack withdrawal, not a field break: only
    // the fields of tools the release keeps are compared.
    let prior_fields = active
        .fields
        .iter()
        .filter(|(path, _)| {
            path.split_once("#/")
                .is_some_and(|(tool, _)| kept_tools.contains(tool))
        })
        .map(|(path, declared)| (path.clone(), declared.clone()))
        .collect();
    let prior = ReleaseContract {
        version: active.version.clone(),
        ontology_ttl,
        shapes_ttl,
        fields: prior_fields,
        mappings: active.mappings.clone(),
    };
    let found = diff_release(&prior, &next).map_err(|error| error.to_string())?;
    let removed: BTreeSet<String> = found
        .iter()
        .filter(|found| found.kind == ContractBreakKind::RemovedClass)
        .map(|found| found.item.clone())
        .collect();
    if !removed.is_empty() {
        let in_use = usage.classes_in_use(tenant, connector, &active.version.version, &removed)?;
        if !in_use.is_empty() {
            let violations = in_use
                .iter()
                .take(MAX_REPORTED)
                .map(|class| {
                    violation(
                        PackViolationCode::ClassInUse,
                        Some(class.clone()),
                        "stored data still names this class; the release is refused".to_string(),
                    )
                })
                .collect();
            return refusal(request, violations, in_use.len() > MAX_REPORTED, None);
        }
    }
    if let Some(held) = approval {
        // The held release's change request was applied: the one activation path is
        // `ContractHistory::approve`, which moves it through the release machine.
        let mut history = ContractHistory::with_held(
            prior,
            next.clone(),
            held.breaks.clone(),
            held.change_request.clone(),
        )
        .map_err(|error| error.to_string())?;
        let version = next.version.version.clone();
        history
            .approve(&version)
            .map_err(|error| error.to_string())?;
        next.version = history
            .by_version(&version)
            .map(|stored| stored.contract.version.clone())
            .ok_or_else(|| "PACK_RELEASE_NOT_HELD: the held release is unknown".to_string())?;
        let evidence = PackReleaseEvidence {
            previous_contract_digest: Some(active.version.contract_digest.clone()),
            contract_digest: next.version.contract_digest.clone(),
            break_count: held.total_breaks,
            change_request: Some(ChangeRequestRef {
                target: held.change_request.target.clone(),
                proposed_digest: held.change_request.proposed_digest.clone(),
            }),
        };
        return Ok(Verdict::Apply(AppliedRelease {
            release: release_of(&next, schemas),
            evidence,
        }));
    }
    let target = format!("pack:{}", connector.as_str());
    let owner = StewardshipAssignment {
        scope: StewardshipScope::Asset(target.clone()),
        role: StewardRole::Owner,
        principal: request.context.caller_principal.clone(),
    };
    let scopes = TargetScopes {
        asset: Some(target),
        concept_scheme: None,
        domain: None,
    };
    let mut history = ContractHistory::with_active(prior).map_err(|error| error.to_string())?;
    let disposition = history
        .submit(next.clone(), &[owner], &scopes)
        .map_err(|error| error.to_string())?;
    next.version.supersedes = Some(active.version.version.clone());
    match disposition {
        ReleaseDisposition::Applied { .. } => {
            let evidence = PackReleaseEvidence {
                previous_contract_digest: Some(active.version.contract_digest.clone()),
                contract_digest: next.version.contract_digest.clone(),
                break_count: 0,
                change_request: None,
            };
            Ok(Verdict::Apply(AppliedRelease {
                release: release_of(&next, schemas),
                evidence,
            }))
        }
        ReleaseDisposition::Pending {
            breaks,
            total_breaks,
            change_request,
        } => {
            let head = request.expected_head.as_ref().ok_or_else(|| {
                "PACK_HEAD_CONFLICT: a held release needs the head it was diffed against"
                    .to_string()
            })?;
            let staged = stage_pack(chunks, request, archive)?;
            let pending = PendingPackRelease {
                staged: Some(staged),
                release: release_of(&next, schemas),
                pack_digest: request.index.pack_digest,
                breaks: breaks.clone(),
                total_breaks: u64::try_from(total_breaks).unwrap_or(u64::MAX),
                change_request: change_request.clone(),
            };
            store.hold_connector_pack_release(request.context.clone(), connector, head, pending)?;
            let summary = format!(
                "{total_breaks} breaking changes held behind change request for {}",
                change_request.target
            );
            let violations = breaks
                .iter()
                .take(MAX_REPORTED)
                .map(|found: &ContractBreak| {
                    violation(
                        PackViolationCode::ContractBreakPending,
                        Some(found.item.clone()),
                        format!("{}: {summary}", kind_name(found.kind)),
                    )
                })
                .collect();
            let held = PackReleaseEvidence {
                previous_contract_digest: Some(active.version.contract_digest.clone()),
                contract_digest: next.version.contract_digest.clone(),
                break_count: u64::try_from(total_breaks).unwrap_or(u64::MAX),
                change_request: Some(ChangeRequestRef {
                    target: change_request.target.clone(),
                    proposed_digest: change_request.proposed_digest.clone(),
                }),
            };
            refusal(
                request,
                violations,
                total_breaks > breaks.len().min(MAX_REPORTED),
                Some(held),
            )
        }
    }
}
