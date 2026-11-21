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
    ContractBreak, ContractBreakKind, PackContractVersion, PackRelease, PendingPackRelease,
};
use medulla_catalog_types::connector_pack::{
    ConnectorPackImportRequest, PackEntryKind, PackImportResult, PackViolation, PackViolationCode,
};
use medulla_core::stewardship::TargetScopes;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::connector_pack::ConnectorPackMemberRow;
use medulla_types::contract::{BoundedVec, Digest256, ResourceId};
use medulla_catalog_types::stewardship::{StewardRole, StewardshipAssignment, StewardshipScope};
use tokio::sync::RwLock;

use super::planning::ReadyPlan;
use super::validation::section_bytes;
use crate::admin::connector_pack::contract_diff::{
    diff_release, ContractHistory, ReleaseContract, ReleaseDisposition,
};
use medulla_server_api::state::ServerState;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
/// Most violations one `Rejected` result carries.
const MAX_REPORTED: usize = 256;

/// Which of `classes` stored data still names, for the graphs that compose a
/// connector's current head.
pub(crate) trait ClassUsage: Send + Sync {
    fn classes_in_use(
        &self,
        connector: &ResourceId,
        record_id: &str,
        classes: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, String>;
}

/// The served implementation: every graph whose `pack:<connector>` schema
/// source was composed from `record_id` is opened (lazily when it is only
/// catalog-known) and its data triples are read for `rdf:type` objects.
pub(crate) struct GraphClassUsage {
    pub state: Arc<RwLock<ServerState>>,
    pub handle: tokio::runtime::Handle,
}

impl ClassUsage for GraphClassUsage {
    fn classes_in_use(
        &self,
        connector: &ResourceId,
        record_id: &str,
        classes: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, String> {
        let source_id = format!("pack:{}", connector.as_str());
        self.handle.block_on(async {
            let names = self.state.read().await.registry.list();
            let cap = medulla_persistence::cold_offload::max_resident_graphs();
            let page_size = medulla_persistence::cold_offload::lazy_open_page_size();
            let mut found = BTreeSet::new();
            for (name, _) in names {
                medulla_persistence::cold_offload::lazy_open(&self.state, &name, cap, page_size)
                    .await;
                let core = {
                    let state = self.state.read().await;
                    let Some(entry) = state.registry.get(&name) else {
                        continue;
                    };
                    entry.core.clone()
                };
                let composes = core
                    .schema_sources()
                    .dynamic
                    .get(&source_id)
                    .is_some_and(|s| {
                        matches!(
                            &s.origin,
                            medulla_graph_core::graph::SchemaSourceOrigin::Pack {
                                record_id: composed, ..
                            } if composed == record_id
                        )
                    });
                if !composes {
                    continue;
                }
                for triple in medulla_rdf::mapping::export_view_triples(&core.analysis_snapshot())?
                {
                    if triple.predicate.as_str() != RDF_TYPE {
                        continue;
                    }
                    if let medulla_rdf::oxrdf::Term::NamedNode(class) = &triple.object {
                        if classes.contains(class.as_str()) {
                            found.insert(class.as_str().to_string());
                        }
                    }
                }
            }
            Ok::<_, String>(found)
        })
    }
}

/// What the check decided for one import.
pub(super) enum Verdict {
    /// Commit, recording this release as the head's active summary.
    Apply(PackRelease),
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

fn release_of(contract: &ReleaseContract) -> PackRelease {
    PackRelease {
        version: contract.version.clone(),
        fields: contract.fields.clone(),
        mappings: contract.mappings.clone(),
    }
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
) -> Result<Verdict, String> {
    Ok(Verdict::Refused(PackImportResult::Rejected {
        pack_digest: Some(request.index.pack_digest),
        violations: BoundedVec::new(violations)?,
        budget_exhausted,
    }))
}

fn kind_name(kind: ContractBreakKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Diff the release `ready` would activate against the head's active release.
pub(super) fn judge(
    store: &AgentLibraryStore,
    chunks: &dyn medulla_server_blob::blob::ChunkStore,
    request: &ConnectorPackImportRequest,
    archive: &[u8],
    ready: &ReadyPlan,
    prior_members: &[ConnectorPackMemberRow],
    usage: &dyn ClassUsage,
) -> Result<Verdict, String> {
    let tenant = request.context.tenant_id.as_str();
    let connector = &request.index.connector;
    let mut next = next_contract(request, archive, ready)?;
    let Some(active) = store.connector_pack_release(tenant, connector)? else {
        return Ok(Verdict::Apply(release_of(&next)));
    };
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
        let in_use = usage.classes_in_use(connector, &active.version.version, &removed)?;
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
            return refusal(request, violations, in_use.len() > MAX_REPORTED);
        }
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
        ReleaseDisposition::Applied { .. } => Ok(Verdict::Apply(release_of(&next))),
        ReleaseDisposition::Pending {
            breaks,
            total_breaks,
            change_request,
        } => {
            let head = request.expected_head.as_ref().ok_or_else(|| {
                "PACK_HEAD_CONFLICT: a held release needs the head it was diffed against"
                    .to_string()
            })?;
            let pending = PendingPackRelease {
                release: release_of(&next),
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
            refusal(
                request,
                violations,
                total_breaks > breaks.len().min(MAX_REPORTED),
            )
        }
    }
}
