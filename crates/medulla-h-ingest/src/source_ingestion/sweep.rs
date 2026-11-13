//! Source deletion reconciliation (MEDULLA-INGEST-WORKER-R903.2).
//!
//! An identifier-only enumeration arrives in pages under one `run_id`. Each
//! page's identifier digests are staged on a durable run node; the last page of
//! a `complete` run checks the declared `ids_digest`, merges the pages and plans
//! the held records the source no longer lists. Every missing record waits out
//! the grace window (`ingest.reconcile_grace_s`); the sweep job then deletes the
//! expired entries through the tombstone write of `admit_tombstones`, and a
//! Document node follows the cascade of `DocumentDelete`. Staged runs and plans
//! are graph facts: `admit_enumeration` returns methods for the batch's own
//! commit, and the sweep commits through the internal mutation path.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, ResultPayload};
use medulla_server_api::state::ServerState;
use medulla_source_types::source_ingestion::batch_kind::EnumerationPage;
use medulla_source_types::source_ingestion::SourceEntityRef;
use medulla_types::contract::Digest256;
use medulla_types::ingest_rules::plan_reconcile_digests;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

const RUN_NODE_TYPE: &str = "SourceEnumerationRun";
const PLAN_NODE_TYPE: &str = "SourceReconcilePlan";
/// Reason written on the tombstone of a record the sweep deleted.
pub const SWEEP_REASON: &str = "absent_from_complete_enumeration";

/// States of one enumeration run (machine `enumeration_run`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnumerationRunState {
    /// Pages are still arriving.
    Staging,
    /// The last page of a complete run planned the missing records.
    Planned,
    /// The last page of an incomplete run closed it; nothing is ever planned.
    Discarded,
}

/// Events of the `enumeration_run` machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumerationRunEvent {
    PageStaged,
    Completed,
    Incomplete,
}

/// An event the machine refuses in the current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumerationTransitionError {
    pub state: EnumerationRunState,
    pub event: EnumerationRunEvent,
}

impl std::fmt::Display for EnumerationTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SOURCE_ENUMERATION_TRANSITION_ILLEGAL: machine enumeration_run in state {:?} refuses event {:?}",
            self.state, self.event
        )
    }
}

impl std::error::Error for EnumerationTransitionError {}

/// The only writer of `StagedRun.state`.
pub fn enumeration_transition(
    state: EnumerationRunState,
    event: EnumerationRunEvent,
) -> Result<EnumerationRunState, EnumerationTransitionError> {
    use EnumerationRunEvent as E;
    use EnumerationRunState as S;
    match (state, event) {
        (S::Staging, E::PageStaged) => Ok(S::Staging),
        (S::Staging, E::Completed) => Ok(S::Planned),
        (S::Staging, E::Incomplete) => Ok(S::Discarded),
        _ => Err(EnumerationTransitionError { state, event }),
    }
}

/// The durable record of one run: sorted identifier digests per page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedRun {
    #[serde(rename = "type")]
    pub node_type: String,
    pub connector: String,
    pub stream: String,
    pub run_id: String,
    pub state: EnumerationRunState,
    /// Page number to the concatenated 32-byte digests of that page, sorted.
    pub pages: BTreeMap<u32, serde_bytes::ByteBuf>,
    pub last_page: Option<u32>,
    pub complete: bool,
    pub ids_digest: Option<String>,
}

/// One planned deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedMissing {
    pub record: SourceEntityRef,
    /// The grace window ends at this time, in milliseconds.
    pub due_ms: u64,
}

/// The durable plan of one source stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPlan {
    #[serde(rename = "type")]
    pub node_type: String,
    pub tenant: String,
    pub connector: String,
    pub stream: String,
    pub run_id: String,
    pub ids_digest: String,
    pub missing: Vec<PlannedMissing>,
}

/// Everything `admit_enumeration` reads.
pub struct EnumerationInput<'a> {
    pub tenant: &'a str,
    pub connector: &'a str,
    pub stream: &'a str,
    pub page: &'a EnumerationPage,
    /// The live set the source's marker holds.
    pub held: &'a [SourceEntityRef],
    pub staged: Option<StagedRun>,
    pub existing_plan: Option<StoredPlan>,
    pub now_ms: u64,
    pub grace_s: u64,
    pub admin_override: bool,
}

/// What one enumeration page changes.
#[derive(Debug, Clone)]
pub struct EnumerationAdmission {
    pub run: StagedRun,
    /// Present when this page closed a complete run.
    pub plan: Option<StoredPlan>,
    pub missing_count: u64,
    pub ids_digest: String,
    /// The two node writes of the page: the run, then the plan when planned.
    pub methods: Vec<Method>,
}

/// SHA-256 of one identifier (`stream`, NUL, `record_id`).
pub fn ref_digest(entity: &SourceEntityRef) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(entity.stream.as_str().as_bytes());
    hasher.update([0u8]);
    hasher.update(entity.record_id.as_bytes());
    hasher.finalize().into()
}

/// Digest of the sorted identifier digests of a whole run.
pub fn ids_digest_of(sorted: &[[u8; 32]]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"eg/source-enumeration-ids/v1");
    for digest in sorted {
        hasher.update(digest);
    }
    hex::encode(hasher.finalize())
}

fn page_bytes(page: &EnumerationPage) -> serde_bytes::ByteBuf {
    let mut digests: Vec<[u8; 32]> = page.ids.iter().map(ref_digest).collect();
    digests.sort_unstable();
    digests.dedup();
    serde_bytes::ByteBuf::from(digests.concat())
}

fn page_digests(bytes: &serde_bytes::ByteBuf) -> Result<Vec<[u8; 32]>, String> {
    if bytes.len() % 32 != 0 {
        return Err("SOURCE_ENUMERATION_STAGE_INVALID: staged page is not whole digests".into());
    }
    Ok(bytes
        .chunks_exact(32)
        .map(|chunk| {
            let mut digest = [0u8; 32];
            digest.copy_from_slice(chunk);
            digest
        })
        .collect())
}

fn scoped_id(prefix: &str, tenant: &str, connector: &str, stream: &str) -> Result<String, String> {
    Ok(format!(
        "{prefix}:{}",
        Digest256::framed(
            b"eg/source-reconcile-identity/v1",
            &[
                prefix.as_bytes(),
                tenant.as_bytes(),
                connector.as_bytes(),
                stream.as_bytes(),
            ],
        )?
        .to_hex()
    ))
}

/// Id of the staged-run node of one source stream.
pub fn run_node_id(tenant: &str, connector: &str, stream: &str) -> Result<String, String> {
    scoped_id("source-enum-run", tenant, connector, stream)
}

/// Id of the plan node of one source stream.
pub fn plan_node_id(tenant: &str, connector: &str, stream: &str) -> Result<String, String> {
    scoped_id("source-reconcile-plan", tenant, connector, stream)
}

fn node_method(node_id: String, value: &impl Serialize) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id,
        properties_msgpack: rmp_serde::to_vec_named(value)
            .map_err(|error| format!("reconcile node encoding failed: {error}"))?,
    })
}

/// Stage one enumeration page; on the last page of a complete run check the
/// declared digest and plan the missing records. An incomplete run never plans.
pub fn admit_enumeration(input: EnumerationInput<'_>) -> Result<EnumerationAdmission, String> {
    let page = input.page;
    let mut run = match input.staged {
        Some(run) if run.run_id == page.run_id => run,
        _ => StagedRun {
            node_type: RUN_NODE_TYPE.into(),
            connector: input.connector.into(),
            stream: input.stream.into(),
            run_id: page.run_id.clone(),
            state: EnumerationRunState::Staging,
            pages: BTreeMap::new(),
            last_page: None,
            complete: page.complete,
            ids_digest: None,
        },
    };
    run.state = enumeration_transition(run.state, EnumerationRunEvent::PageStaged)
        .map_err(|error| error.to_string())?;
    run.pages.insert(page.page, page_bytes(page));
    run.complete = page.complete;
    let run_id = run_node_id(input.tenant, input.connector, input.stream)?;
    if !page.last_page {
        let methods = vec![node_method(run_id, &run)?];
        return Ok(EnumerationAdmission {
            run,
            plan: None,
            missing_count: 0,
            ids_digest: String::new(),
            methods,
        });
    }
    let last = page.page;
    if (0..=last).any(|number| !run.pages.contains_key(&number)) {
        return Err(format!(
            "SOURCE_ENUMERATION_PAGES_MISSING: run {} lacks a page below {last}",
            page.run_id
        ));
    }
    run.last_page = Some(last);
    run.ids_digest = page.ids_digest.clone();
    let mut enumerated: Vec<[u8; 32]> = Vec::new();
    for bytes in run.pages.values() {
        enumerated.extend(page_digests(bytes)?);
    }
    enumerated.sort_unstable();
    enumerated.dedup();
    let computed = ids_digest_of(&enumerated);
    let declared = page.ids_digest.clone().ok_or_else(|| {
        "SOURCE_ENUMERATION_DIGEST_MISSING: the last page carries no ids_digest".to_string()
    })?;
    if declared != computed {
        return Err(format!(
            "SOURCE_ENUMERATION_DIGEST_MISMATCH: declared {declared}, computed {computed}"
        ));
    }
    if !page.complete {
        run.state = enumeration_transition(run.state, EnumerationRunEvent::Incomplete)
            .map_err(|error| error.to_string())?;
        return Ok(EnumerationAdmission {
            methods: vec![node_method(run_id, &run)?],
            run,
            plan: None,
            missing_count: 0,
            ids_digest: computed,
        });
    }
    let mut held: Vec<([u8; 32], &SourceEntityRef)> = input
        .held
        .iter()
        .map(|entity| (ref_digest(entity), entity))
        .collect();
    held.sort_unstable_by_key(|(digest, _)| *digest);
    held.dedup_by_key(|(digest, _)| *digest);
    let held_digests: Vec<[u8; 32]> = held.iter().map(|(digest, _)| *digest).collect();
    let missing = plan_reconcile_digests(&held_digests, &enumerated, true, input.admin_override)
        .map_err(|refusal| refusal.to_string())?;
    run.state = enumeration_transition(run.state, EnumerationRunEvent::Completed)
        .map_err(|error| error.to_string())?;
    let due_new = input
        .now_ms
        .saturating_add(input.grace_s.saturating_mul(1_000));
    let earlier: BTreeMap<[u8; 32], u64> = input
        .existing_plan
        .iter()
        .flat_map(|plan| plan.missing.iter())
        .map(|entry| (ref_digest(&entry.record), entry.due_ms))
        .collect();
    let by_digest: BTreeMap<[u8; 32], &SourceEntityRef> = held.into_iter().collect();
    let planned: Vec<PlannedMissing> = missing
        .iter()
        .filter_map(|digest| {
            by_digest.get(digest).map(|record| PlannedMissing {
                record: (*record).clone(),
                due_ms: earlier.get(digest).copied().unwrap_or(due_new),
            })
        })
        .collect();
    let plan = StoredPlan {
        node_type: PLAN_NODE_TYPE.into(),
        tenant: input.tenant.into(),
        connector: input.connector.into(),
        stream: input.stream.into(),
        run_id: page.run_id.clone(),
        ids_digest: computed.clone(),
        missing: planned,
    };
    let methods = vec![
        node_method(run_id, &run)?,
        node_method(
            plan_node_id(input.tenant, input.connector, input.stream)?,
            &plan,
        )?,
    ];
    Ok(EnumerationAdmission {
        run,
        missing_count: plan.missing.len() as u64,
        plan: Some(plan),
        ids_digest: computed,
        methods,
    })
}

/// Split a plan at `now_ms`: the entries whose grace window ended, and the plan
/// that remains.
pub fn split_due(plan: &StoredPlan, now_ms: u64) -> (Vec<SourceEntityRef>, StoredPlan) {
    let (due, waiting): (Vec<_>, Vec<_>) = plan
        .missing
        .iter()
        .cloned()
        .partition(|entry| entry.due_ms <= now_ms);
    let mut remaining = plan.clone();
    remaining.missing = waiting;
    (
        due.into_iter().map(|entry| entry.record).collect(),
        remaining,
    )
}

/// The cascade view of a graph core, without row-level visibility: the sweep is
/// engine-internal maintenance of records the source already withdrew.
struct CoreReader<'a> {
    core: &'a medulla_graph_core::graph::GraphCore,
}

impl CoreReader<'_> {
    fn key(&self, node_id: &str) -> medulla_document::cascade::NodeKey {
        medulla_document::cascade::NodeKey {
            label: node_type(self.core, node_id).unwrap_or_default(),
            id: node_id.to_string(),
        }
    }

    fn edges(&self, source: &str, target: &str) -> Vec<medulla_document::cascade::EdgeKey> {
        let from = self.key(source);
        let to = self.key(target);
        self.core
            .typed_edges_between(source, target)
            .into_iter()
            .map(|(label, _)| medulla_document::cascade::EdgeKey {
                from: from.clone(),
                label,
                to: to.clone(),
            })
            .collect()
    }
}

impl medulla_document::cascade::CascadeReader for CoreReader<'_> {
    fn in_edges(
        &self,
        node: &medulla_document::cascade::NodeKey,
    ) -> Vec<medulla_document::cascade::EdgeKey> {
        self.core
            .get_predecessors(&node.id)
            .unwrap_or_default()
            .iter()
            .flat_map(|source| self.edges(source, &node.id))
            .collect()
    }

    fn out_edges(
        &self,
        node: &medulla_document::cascade::NodeKey,
    ) -> Vec<medulla_document::cascade::EdgeKey> {
        self.core
            .get_successors(&node.id)
            .unwrap_or_default()
            .iter()
            .flat_map(|target| self.edges(&node.id, target))
            .collect()
    }
}

fn node_type(core: &medulla_graph_core::graph::GraphCore, node_id: &str) -> Option<String> {
    let blob = core.get_node_properties(node_id)?;
    let value = medulla_types::msgpack::decode_property_value(&blob).ok()?;
    value
        .get("type")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

/// The writes that delete the due records of one plan: a Document node follows
/// the `DocumentDelete` cascade, every other record the tombstone write. The
/// live-set marker drops the deleted identifiers and the plan keeps the rest.
pub fn sweep_methods(
    core: &medulla_graph_core::graph::GraphCore,
    plan: &StoredPlan,
    now_ms: u64,
) -> Result<(Vec<Method>, u64), String> {
    let (due, remaining) = split_due(plan, now_ms);
    if due.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let reader = CoreReader { core };
    let max_nodes =
        usize::try_from(medulla_server_api::knob_config::knobs().document_max_cascade_nodes)
            .unwrap_or(usize::MAX);
    let checkpoint = format!("reconcile:{}", plan.run_id);
    let mut methods = Vec::new();
    for record in &due {
        let node_id = super::source_node_id(
            &plan.tenant,
            &plan.connector,
            record.stream.as_str(),
            &record.record_id,
        )?;
        if node_type(core, &node_id).as_deref() == Some(medulla_document::cascade::DOCUMENT_LABEL) {
            if let Ok(cascade) =
                medulla_document::cascade::plan_cascade(&reader, &plan.tenant, &node_id, max_nodes)
            {
                let (deletion, _) = crate::document_delete::deletion_record(
                    &cascade,
                    medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
                    "engine:reconcile-sweep",
                );
                methods.extend(crate::document_delete::lower(&cascade, &deletion)?.methods);
                continue;
            }
        }
        methods.push(super::tombstone_method(
            &node_id,
            SWEEP_REASON,
            &checkpoint,
        )?);
    }
    let marker_id = super::source_marker_id_for(
        &plan.tenant,
        &medulla_types::contract::ResourceId::new(plan.connector.as_str())?,
        &medulla_types::contract::ResourceId::new(plan.stream.as_str())?,
    )?;
    if let Some(blob) = core.get_node_properties(&marker_id) {
        let mut marker: super::SourceLiveSetMarker = rmp_serde::from_slice(&blob)
            .map_err(|error| format!("SOURCE_RECONCILIATION_MARKER_INVALID: {error}"))?;
        marker.live_ids.retain(|entity| !due.contains(entity));
        marker.live_set_digest = Some(super::digest_live_set(&marker.live_ids)?);
        methods.push(node_method(marker_id, &marker)?);
    }
    methods.push(node_method(
        plan_node_id(&plan.tenant, &plan.connector, &plan.stream)?,
        &remaining,
    )?);
    Ok((methods, due.len() as u64))
}

/// One sweep pass over every resident graph; returns the records deleted.
pub async fn reconcile_sweep(state: &Arc<RwLock<ServerState>>, now_ms: u64) -> Result<u64, String> {
    let (graphs, persistence) = {
        let current = state.read().await;
        let graphs: Vec<(String, Arc<medulla_graph_core::graph::GraphCore>)> = current
            .registry
            .all_entries()
            .into_iter()
            .map(|entry| (entry.name.clone(), entry.core.clone()))
            .collect();
        (graphs, current.persistence.clone())
    };
    let mut deleted = 0u64;
    for (graph, core) in graphs {
        for (_, blob) in core.get_nodes_by_label(PLAN_NODE_TYPE, 0) {
            let plan: StoredPlan = rmp_serde::from_slice(&blob)
                .map_err(|error| format!("SOURCE_RECONCILE_PLAN_INVALID: {error}"))?;
            let (methods, count) = sweep_methods(&core, &plan, now_ms)?;
            if count == 0 {
                continue;
            }
            let batch_id = sweep_batch_id(&plan, now_ms)?;
            let result = ResultPayload::Json(serde_json::json!({ "deleted": count }));
            medulla_server_mutation::mutation_batch::commit_internal_graph_methods_with_nonce(
                medulla_server_mutation::mutation_batch::InternalGraphCommitRequest::new(
                    persistence.as_ref(),
                    &core,
                    medulla_server_mutation::mutation_batch::CommitOrigin {
                        request_id: 0,
                        principal: Some("engine:reconcile-sweep"),
                    },
                    &graph,
                    &batch_id,
                    methods,
                    &result,
                )
                .with_tenant_scope(&plan.tenant),
            )
            .await?;
            deleted += count;
        }
    }
    Ok(deleted)
}

fn sweep_batch_id(plan: &StoredPlan, now_ms: u64) -> Result<String, String> {
    let (due, _) = split_due(plan, now_ms);
    let mut fields: Vec<Vec<u8>> = vec![plan.run_id.as_bytes().to_vec()];
    fields.extend(due.iter().map(|entity| ref_digest(entity).to_vec()));
    let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
    Ok(format!(
        "source-reconcile-sweep:{}",
        Digest256::framed(b"eg/source-reconcile-sweep/v1", &refs)?.to_hex()
    ))
}

/// Start the periodic sweep. The interval is read from the registry each round,
/// so a change of `ingest.reconcile_sweep_interval_s` applies at the next wait.
pub fn spawn_reconcile_sweep(state: Arc<RwLock<ServerState>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let interval =
                medulla_server_api::knob_config::knobs().ingest_reconcile_sweep_interval_s;
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
            if let Err(error) = reconcile_sweep(&state, now_ms).await {
                tracing::warn!("reconcile sweep failed: {error}");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::source_ingestion::batch_kind::MAX_ENUMERATION_IDS;
    use medulla_types::contract::{BoundedVec, ResourceId};
    use medulla_types::ingest_rules::MASS_DELETE_PERCENT;

    fn entity(id: &str) -> SourceEntityRef {
        SourceEntityRef {
            stream: ResourceId::new("items").unwrap(),
            record_id: id.into(),
        }
    }

    fn page(run: &str, number: u32, last: bool, complete: bool, ids: &[&str]) -> EnumerationPage {
        let all: Vec<SourceEntityRef> = ids.iter().map(|id| entity(id)).collect();
        EnumerationPage {
            run_id: run.into(),
            page: number,
            last_page: last,
            complete,
            ids_digest: None,
            ids: BoundedVec::<SourceEntityRef, MAX_ENUMERATION_IDS>::new(all).unwrap(),
        }
    }

    fn digest_of_run(ids: &[&str]) -> String {
        let mut digests: Vec<[u8; 32]> = ids.iter().map(|id| ref_digest(&entity(id))).collect();
        digests.sort_unstable();
        digests.dedup();
        ids_digest_of(&digests)
    }

    fn input<'a>(
        page: &'a EnumerationPage,
        held: &'a [SourceEntityRef],
        staged: Option<StagedRun>,
        plan: Option<StoredPlan>,
        now_ms: u64,
    ) -> EnumerationInput<'a> {
        EnumerationInput {
            tenant: "tenant",
            connector: "wiki",
            stream: "items",
            page,
            held,
            staged,
            existing_plan: plan,
            now_ms,
            grace_s: 86_400,
            admin_override: false,
        }
    }

    /// Two pages that together list everything but `gone`.
    fn run_without(
        held: &[SourceEntityRef],
        listed: &[&str],
        now_ms: u64,
        plan: Option<StoredPlan>,
    ) -> EnumerationAdmission {
        let (first, second) = listed.split_at(listed.len() / 2);
        let first_page = page("run-1", 0, false, true, first);
        let staged = admit_enumeration(input(&first_page, held, None, plan.clone(), now_ms))
            .unwrap()
            .run;
        let mut last = page("run-1", 1, true, true, second);
        last.ids_digest = Some(digest_of_run(listed));
        admit_enumeration(input(&last, held, Some(staged), plan, now_ms)).unwrap()
    }

    // spec: MEDULLA-INGEST-WORKER-R903.2
    #[test]
    fn missing_records_delete_after_grace() {
        let ids: Vec<String> = (0..10).map(|n| format!("r{n}")).collect();
        let held: Vec<SourceEntityRef> = ids.iter().map(|id| entity(id)).collect();
        let listed: Vec<&str> = ids.iter().take(9).map(String::as_str).collect();
        let t0 = 1_000_000u64;
        let admission = run_without(&held, &listed, t0, None);
        assert_eq!(admission.run.state, EnumerationRunState::Planned);
        assert_eq!(admission.missing_count, 1);
        let plan = admission.plan.expect("a complete run plans");
        assert_eq!(plan.missing[0].record, entity("r9"));
        assert_eq!(plan.missing[0].due_ms, t0 + 86_400_000);

        // Inside the window nothing is due.
        let (due, remaining) = split_due(&plan, t0 + 86_399_999);
        assert!(due.is_empty());
        assert_eq!(remaining.missing.len(), 1);
        // Past the window the record is due and leaves the plan.
        let (due, remaining) = split_due(&plan, t0 + 86_400_000);
        assert_eq!(due, vec![entity("r9")]);
        assert!(remaining.missing.is_empty());

        // The sweep writes the shared tombstone for it.
        let node_id =
            crate::source_ingestion::source_node_id("tenant", "wiki", "items", "r9").unwrap();
        let tombstone =
            crate::source_ingestion::tombstone_method(&node_id, SWEEP_REASON, "c").unwrap();
        assert!(matches!(tombstone, Method::CompareAndSetNodeFields { .. }));
    }

    // spec: MEDULLA-INGEST-WORKER-R903.2
    #[test]
    fn a_reappearing_record_leaves_the_plan_and_keeps_its_first_window() {
        let ids: Vec<String> = (0..10).map(|n| format!("r{n}")).collect();
        let held: Vec<SourceEntityRef> = ids.iter().map(|id| entity(id)).collect();
        let nine: Vec<&str> = ids.iter().take(9).map(String::as_str).collect();
        let t0 = 5_000u64;
        let plan = run_without(&held, &nine, t0, None).plan.unwrap();
        // A later complete run still lacks r9 and r8: r9 keeps its first window.
        let eight: Vec<&str> = ids.iter().take(8).map(String::as_str).collect();
        let second = run_without(&held, &eight, t0 + 10, Some(plan.clone()))
            .plan
            .unwrap();
        let r9 = second
            .missing
            .iter()
            .find(|m| m.record == entity("r9"))
            .unwrap();
        assert_eq!(r9.due_ms, plan.missing[0].due_ms);
        // A run that lists everything again empties the plan.
        let all: Vec<&str> = ids.iter().map(String::as_str).collect();
        let third = run_without(&held, &all, t0 + 20, Some(second))
            .plan
            .unwrap();
        assert!(third.missing.is_empty());
    }

    // spec: MEDULLA-INGEST-WORKER-R903.2
    #[test]
    fn an_incomplete_run_never_plans_and_a_mass_delete_is_refused() {
        let ids: Vec<String> = (0..10).map(|n| format!("r{n}")).collect();
        let held: Vec<SourceEntityRef> = ids.iter().map(|id| entity(id)).collect();
        let listed = ["r0"];
        let mut last = page("run-2", 0, true, false, &listed);
        last.ids_digest = Some(digest_of_run(&listed));
        let admission = admit_enumeration(input(&last, &held, None, None, 1)).unwrap();
        assert_eq!(admission.run.state, EnumerationRunState::Discarded);
        assert!(admission.plan.is_none());
        // The same listing marked complete deletes 90 percent: refused.
        let mut complete = page("run-3", 0, true, true, &listed);
        complete.ids_digest = Some(digest_of_run(&listed));
        let error = admit_enumeration(input(&complete, &held, None, None, 1)).unwrap_err();
        assert!(error.starts_with("SOURCE_MASS_DELETE_GUARD"), "{error}");
        assert!(MASS_DELETE_PERCENT < 90);
        // A wrong declared digest is refused before any plan.
        let mut wrong = page("run-4", 0, true, true, &listed);
        wrong.ids_digest = Some("00".repeat(32));
        let error = admit_enumeration(input(&wrong, &held, None, None, 1)).unwrap_err();
        assert!(
            error.starts_with("SOURCE_ENUMERATION_DIGEST_MISMATCH"),
            "{error}"
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R903.2
    #[test]
    fn the_enumeration_run_machine_allows_only_its_transitions() {
        use EnumerationRunEvent as E;
        use EnumerationRunState as S;
        let legal = [
            (S::Staging, E::PageStaged, S::Staging),
            (S::Staging, E::Completed, S::Planned),
            (S::Staging, E::Incomplete, S::Discarded),
        ];
        for (state, event, next) in legal {
            assert_eq!(enumeration_transition(state, event), Ok(next));
        }
        for state in [S::Planned, S::Discarded] {
            for event in [E::PageStaged, E::Completed, E::Incomplete] {
                let error = enumeration_transition(state, event).unwrap_err();
                assert!(error.to_string().contains("enumeration_run"));
            }
        }
    }
}
