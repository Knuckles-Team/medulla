//! Durable known concept ids and `superseded_by` lineage edges
//! (MEDULLA-DURABLE-KERNEL-R094.7 and R094.7.1).
//!
//! Both tables live in the Agent Library owner, keyed by tenant first, so one
//! tenant never reads, claims or links another tenant's ids. Every write goes
//! through the admitted pack-write lifecycle (replay identity, one staged batch,
//! owner rows, receipt and ledger in one transaction), and the decision of
//! whether a claim or an edge is allowed is made by the pure types in
//! `medulla_types::concept_id`, never re-implemented here.

use std::collections::BTreeSet;

use medulla_agent_types::agent_library::AgentLibraryMutationContext;
use medulla_agent_types::concept_lineage::{ConceptEdgeRecord, ConceptReadResult, ConceptRecord};
use medulla_mutation_types::mutation_scope::{DurabilityDomain, MutationSurface};
use medulla_protocol_types::mutation_batch::MutationOperation;
use medulla_protocol_types::protocol::Method;
use medulla_types::concept_id::{
    ConceptId, ConceptIdError, ConceptLineage, ConceptLineageError, ConceptPathSet, LineageEdge,
};
use medulla_types::contract::Digest256;
use redb::ReadableTable;
use serde::{Deserialize, Serialize};

use crate::agent_library::AgentLibraryStore;
use crate::connector_pack::admitted::{PackOwnerWrite, PackWriteEffects, PackWriteIdentity};
use crate::connector_pack::undecodable::decode_stored;

const CLAIM_SCHEMA: &str = "concept-claim-result.v1";
const EDGE_SCHEMA: &str = "concept-lineage-edge-result.v1";

/// The stored value of one `concept_ids` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredConcept {
    repository: String,
    admitted_at_ms: u64,
}

fn id_error(error: ConceptIdError) -> String {
    format!("{}: {error}", error.code())
}

fn lineage_error(error: ConceptLineageError) -> String {
    format!("{}: {error}", error.code())
}

fn parse_id(raw: &str) -> Result<ConceptId, String> {
    ConceptId::parse(raw).map_err(id_error)
}

fn store_error(error: impl ToString) -> String {
    error.to_string()
}

/// The `Lifecycle` operation every concept write stages, named by `event_type`
/// and bound to the request by `digest`.
fn effects(event_type: &str, digest: &Digest256) -> PackWriteEffects {
    PackWriteEffects {
        operations: vec![MutationOperation {
            ordinal: 0,
            surface: MutationSurface::Lifecycle,
            domain: DurabilityDomain::ControlPlane,
            method: Method::ApplyMutation {
                event_type: event_type.into(),
                query: digest.to_hex(),
            },
        }],
        outbox: Vec::new(),
    }
}

/// The concepts of `tenant` named in `ids` that are present in `concept_ids`.
fn known_in(
    write: &PackOwnerWrite<'_>,
    tenant: &str,
    ids: &[&ConceptId],
) -> Result<ConceptPathSet, String> {
    let table = write.open_table(medulla_storage::CONCEPT_IDS)?;
    let mut known = ConceptPathSet::default();
    for id in ids {
        let row = table
            .get((tenant, id.as_str()))
            .map_err(store_error)?
            .map(|row| decode_stored::<StoredConcept>(row.value(), "concept id"))
            .transpose()?;
        if let Some(row) = row {
            known
                .claim((*id).clone(), &row.repository)
                .map_err(id_error)?;
        }
    }
    Ok(known)
}

/// The lineage edges reachable from `start`, read inside the write. A cycle
/// through a new edge `a -> b` is a path `b -> ... -> a`, so these are the only
/// edges the check touches and the read is O(edges reachable).
fn reachable_lineage(
    write: &PackOwnerWrite<'_>,
    tenant: &str,
    start: &ConceptId,
) -> Result<ConceptLineage, String> {
    let table = write.open_table(medulla_storage::CONCEPT_LINEAGE_EDGES)?;
    let mut lineage = ConceptLineage::default();
    let mut seen = BTreeSet::new();
    let mut stack = vec![start.to_string()];
    while let Some(node) = stack.pop() {
        if !seen.insert(node.clone()) {
            continue;
        }
        for row in table
            .range((tenant, node.as_str(), "")..)
            .map_err(store_error)?
        {
            let (key, value) = row.map_err(store_error)?;
            let (row_tenant, from, to) = key.value();
            if row_tenant != tenant || from != node {
                break;
            }
            let evidence: LineageEdge = decode_stored(value.value(), "concept lineage edge")?;
            lineage.restore_edge(parse_id(from)?, parse_id(to)?, evidence);
            stack.push(to.to_string());
        }
    }
    Ok(lineage)
}

fn concept_record(id: &ConceptId, stored: &StoredConcept) -> ConceptRecord {
    ConceptRecord {
        concept_id: id.to_string(),
        repository: stored.repository.clone(),
        admitted_at_ms: stored.admitted_at_ms,
    }
}

fn edge_record(superseded: &str, successor: &str, edge: &LineageEdge) -> ConceptEdgeRecord {
    ConceptEdgeRecord {
        superseded: superseded.to_string(),
        successor: successor.to_string(),
        repository: edge.repository.clone(),
        admitted_at_ms: edge.admitted_at_ms,
    }
}

impl AgentLibraryStore {
    /// Record `concept_id` as owned by `repository`. A second claim by the same
    /// repository returns the first record unchanged; a claim by another
    /// repository refuses `CONCEPT_ID_PATH_COLLISION` and writes nothing.
    pub fn claim_concept(
        &self,
        context: &AgentLibraryMutationContext,
        concept_id: &str,
        repository: &str,
    ) -> Result<ConceptRecord, String> {
        let id = parse_id(concept_id)?;
        let subject = id.to_string();
        let digest = Digest256::framed(
            b"medulla/concept-claim/v1",
            &[id.as_str().as_bytes(), repository.as_bytes()],
        )?;
        let discriminator = digest.to_hex();
        let identity = PackWriteIdentity {
            purpose: "concept-lineage:claim",
            kind: "concept-claim",
            slug: "concept-claim",
            subject: &subject,
            revision: 0,
            discriminator: Some(&discriminator),
            result_schema: CLAIM_SCHEMA,
            noun: "concept claim",
        };
        self.commit_pack_write(
            context,
            identity,
            effects("concept_claim", &digest),
            |write, _staged| {
                let tenant = context.tenant_id.as_str();
                let existing = read_stored(write, tenant, &id)?;
                let mut paths = ConceptPathSet::default();
                if let Some(stored) = &existing {
                    paths
                        .claim(id.clone(), &stored.repository)
                        .map_err(id_error)?;
                }
                paths.claim(id.clone(), repository).map_err(id_error)?;
                if let Some(stored) = existing {
                    return Ok(concept_record(&id, &stored));
                }
                let stored = StoredConcept {
                    repository: repository.to_string(),
                    admitted_at_ms: context.created_at_ms,
                };
                let bytes = medulla_storage::encode_bounded(&stored, "concept id")?;
                write
                    .open_table(medulla_storage::CONCEPT_IDS)?
                    .insert((tenant, id.as_str()), bytes.as_slice())
                    .map_err(store_error)?;
                Ok(concept_record(&id, &stored))
            },
        )
    }

    /// Record that `superseded` is superseded by `successor`. Both must be known
    /// to the tenant; a self edge and an edge that would close a cycle refuse by
    /// code and write nothing. Re-adding an edge returns its first evidence.
    pub fn add_concept_edge(
        &self,
        context: &AgentLibraryMutationContext,
        superseded: &str,
        successor: &str,
        repository: &str,
    ) -> Result<ConceptEdgeRecord, String> {
        let from = parse_id(superseded)?;
        let to = parse_id(successor)?;
        let digest = Digest256::framed(
            b"medulla/concept-lineage-edge/v1",
            &[
                from.as_str().as_bytes(),
                to.as_str().as_bytes(),
                repository.as_bytes(),
            ],
        )?;
        let subject = from.to_string();
        let discriminator = digest.to_hex();
        let identity = PackWriteIdentity {
            purpose: "concept-lineage:add-edge",
            kind: "concept-lineage-edge",
            slug: "concept-lineage-edge",
            subject: &subject,
            revision: 0,
            discriminator: Some(&discriminator),
            result_schema: EDGE_SCHEMA,
            noun: "concept lineage edge",
        };
        self.commit_pack_write(
            context,
            identity,
            effects("concept_lineage_edge", &digest),
            |write, _staged| {
                let tenant = context.tenant_id.as_str();
                let known = known_in(write, tenant, &[&from, &to])?;
                let mut lineage = reachable_lineage(write, tenant, &to)?;
                lineage
                    .add_edge(&known, &from, &to, repository, context.created_at_ms)
                    .map_err(lineage_error)?;
                let existing = {
                    let table = write.open_table(medulla_storage::CONCEPT_LINEAGE_EDGES)?;
                    let row = table
                        .get((tenant, from.as_str(), to.as_str()))
                        .map_err(store_error)?
                        .map(|row| {
                            decode_stored::<LineageEdge>(row.value(), "concept lineage edge")
                        })
                        .transpose()?;
                    row
                };
                if let Some(edge) = existing {
                    return Ok(edge_record(from.as_str(), to.as_str(), &edge));
                }
                let edge = LineageEdge {
                    repository: repository.to_string(),
                    admitted_at_ms: context.created_at_ms,
                };
                let bytes = medulla_storage::encode_bounded(&edge, "concept lineage edge")?;
                write
                    .open_table(medulla_storage::CONCEPT_LINEAGE_EDGES)?
                    .insert((tenant, from.as_str(), to.as_str()), bytes.as_slice())
                    .map_err(store_error)?;
                Ok(edge_record(from.as_str(), to.as_str(), &edge))
            },
        )
    }

    /// Read one concept and its direct successors from one committed snapshot.
    /// Another tenant's rows are never visible: every key starts with `tenant_id`.
    pub fn read_concept(
        &self,
        tenant_id: &str,
        concept_id: &str,
    ) -> Result<ConceptReadResult, String> {
        let id = parse_id(concept_id)?;
        let read = self.read()?;
        let ids = read.open_owner_table(medulla_storage::CONCEPT_IDS)?;
        let concept = ids
            .get((tenant_id, id.as_str()))
            .map_err(store_error)?
            .map(|row| decode_stored::<StoredConcept>(row.value(), "concept id"))
            .transpose()?
            .map(|stored| concept_record(&id, &stored));
        let edges = read.open_owner_table(medulla_storage::CONCEPT_LINEAGE_EDGES)?;
        // Most direct successors one `Read` returns (registry knob
        // `concept_lineage.read_max_successors`); a concept with more reports
        // `truncated` so a caller never mistakes a page for the whole set.
        let max_successors = usize::try_from(
            medulla_server_api::knob_config::knobs().concept_lineage_read_max_successors,
        )
        .unwrap_or(usize::MAX);
        let mut successors = Vec::new();
        let mut truncated = false;
        for row in edges
            .range((tenant_id, id.as_str(), "")..)
            .map_err(store_error)?
        {
            let (key, value) = row.map_err(store_error)?;
            let (row_tenant, from, to) = key.value();
            if row_tenant != tenant_id || from != id.as_str() {
                break;
            }
            if successors.len() >= max_successors {
                truncated = true;
                break;
            }
            let edge: LineageEdge = decode_stored(value.value(), "concept lineage edge")?;
            successors.push(edge_record(from, to, &edge));
        }
        Ok(ConceptReadResult {
            concept,
            successors,
            truncated,
        })
    }
}

fn read_stored(
    write: &PackOwnerWrite<'_>,
    tenant: &str,
    id: &ConceptId,
) -> Result<Option<StoredConcept>, String> {
    let table = write.open_table(medulla_storage::CONCEPT_IDS)?;
    let row = table
        .get((tenant, id.as_str()))
        .map_err(store_error)?
        .map(|row| decode_stored::<StoredConcept>(row.value(), "concept id"))
        .transpose()?;
    Ok(row)
}
