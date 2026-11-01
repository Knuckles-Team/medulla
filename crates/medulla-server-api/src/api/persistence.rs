//! Persistence and raft items moved down below their users.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use tokio::sync::RwLock;

use crate::state::ServerState;
use medulla_types::change_envelope::ChangeCursor;
use medulla_types::change_envelope::ChangeEnvelope;
use medulla_types::change_envelope::ChangeEnvelopeCommit;
use medulla_types::change_envelope::ChangeEnvelopeRecord;
use medulla_types::change_envelope::ContentVersion;
use medulla_types::epistemic_operations::DevelopmentLaneQueryRequest;
use medulla_types::epistemic_operations::DevelopmentLaneQueryResult;
use medulla_types::epistemic_operations::DevelopmentLaneStatusRequest;
use medulla_types::epistemic_operations::DevelopmentLaneStatusResult;
use medulla_types::epistemic_operations::ResourceReservationResult;
use medulla_types::epistemic_operations::ResourceReservationStatusRequest;
use medulla_types::epistemic_operations::ResourceReservationStatusResult;
#[cfg(feature = "redb")]
use medulla_types::epistemic_operations_ext::WorkItemClaimCapabilityMintRequest;
use medulla_types::epistemic_operations_ext::WorkItemClaimCapabilityResult;
use medulla_types::epistemic_operations_ext::WorkItemClaimCapabilityVerifyRequest;
use medulla_types::mutation_batch::MutationBatch;
use medulla_types::mutation_batch::MutationBatchCommit;
use medulla_types::mutation_batch::MutationBatchRecord;
use medulla_types::mutation_batch::MutationOutboxLease;
use medulla_types::mutation_batch::MutationOutboxRecord;
use medulla_types::mutation_batch::MutationProjectionCursor;
use medulla_types::native_control::CapacityStatusResult;
use medulla_types::protocol::Method;
// The sweep-scoped claim budget carrying DESIGN.md's per-tenant weighted round
// robin between graphs. It is a kernel type, so the claim it parameterises is
// gated on `redb` exactly like the two WorkItem-capability methods below: the
// durable outbox ledger only exists in that build.
#[cfg(feature = "redb")]
use medulla_transaction::{OutboxClaimBudget, OutboxClaimOutcome};

/// A durable store that can copy its own committed image into a fresh bundle file.
///
/// Implemented next to each store's own identity, because the copy is the STORAGE
/// KERNEL's whole-image backup (`medulla_storage::backup_recovery_store`): it carries the
/// ledger and every declared owner table, and it derives a fresh destination root and
/// rebinds each scope to it, which is what makes the bundled file adoptable at
/// restore. The hand-rolled table-by-table stream this replaced produced a plain redb
/// file with no physical root or owner manifest, which a restore could not adopt.
pub trait BundledStoreSource: Send + Sync {
    /// Bundle-local file name — MUST be a `BackupScope::Bundled` entry in
    /// `DURABLE_STORES`.
    fn file_name(&self) -> &'static str;

    /// Copy the store's latest committed state verbatim into a FRESH file at
    /// `destination`, returning the number of rows copied. Must refuse to overwrite.
    fn copy_into(&self, destination: &std::path::Path) -> Result<u64, String>;

    /// `false` when this store has no on-disk file (an in-memory/test adapter). A
    /// non-durable store is skipped by the backup rather than failing it — there is
    /// nothing to lose and nothing to restore.
    fn is_durable(&self) -> bool {
        true
    }
}

/// Durable verification of a connector-pack source-approval receipt against
/// the stored member, head, body and registration. Implemented by the Agent
/// Library store (MEDULLA-CONTRACT-R056.8); federation reads see only this view.
pub trait SourceApprovalStore {
    /// Verify that `receipt` names exactly the durably approved registration.
    fn verify_connector_pack_source_approval(
        &self,
        receipt: &medulla_types::connector_pack::ConnectorPackSourceApprovalResult,
    ) -> Result<(), String>;
}

/// A durable persistence tier for the graph registry.
///
/// Recovery and mutation APIs are asynchronous because every acknowledged
/// mutation must cross the durable commit barrier. `shutdown` remains synchronous
/// so process teardown can stop the backend after request admission has closed.
#[async_trait::async_trait]
pub trait PersistenceBackend: Send + Sync {
    /// Whether this backend exposes the complete native resource-reservation
    /// authority (atomic MutationBatch rows plus the linearizable readers).
    ///
    /// Capability discovery must not infer support from a backend merely being
    /// present: the trait's default is fail-closed for test/memory backends that
    /// do not implement the native tables.
    fn supports_native_resource_reservations(&self) -> bool {
        false
    }

    /// Whether this backend owns the native capacity-cell/lease authority.
    /// Test and memory backends remain fail-closed until they implement the
    /// same atomic lease tables.
    fn supports_native_capacity_leases(&self) -> bool {
        false
    }

    /// Whether this backend owns the native WorkItem command-log admission
    /// transaction (dedupe, dependency validation, quota, sequence, rows, and
    /// outbox). Non-authoritative test/memory backends fail closed.
    fn supports_native_work_item_submission(&self) -> bool {
        false
    }

    /// Commit one native capacity lease mutation in the writer-owned durable
    /// transaction. The returned bytes are already MessagePack encoded.
    async fn commit_capacity_lease(
        &self,
        _graph_fname: &str,
        _method: Method,
    ) -> Result<Vec<u8>, String> {
        Err("persistence backend does not support native capacity leases".to_string())
    }

    /// Read a bounded native capacity status/reconciliation page from an MVCC
    /// snapshot. The default fails closed for non-authoritative backends.
    async fn read_capacity_status(
        &self,
        _graph_fname: &str,
        _request: &medulla_types::native_control::CapacityStatusRequest,
    ) -> Result<CapacityStatusResult, String> {
        Err("persistence backend does not support native capacity status".to_string())
    }

    /// What each named cell still admits for `priority`, read from one MVCC
    /// snapshot under the acquire path's own pricing rule. The default fails
    /// closed for non-authoritative backends.
    async fn read_capacity_headroom(
        &self,
        _graph_fname: &str,
        _cells: &[String],
        _priority: medulla_types::capacity_lease::LeasePriority,
    ) -> Result<Vec<medulla_types::decision::CapacityHeadroom>, String> {
        Err("persistence backend does not support native capacity headroom".to_string())
    }

    /// Authoritative MVCC read of a repository enrichment snapshot's global
    /// compute budget. `None` means no source-commit authority was seeded;
    /// consumers must not derive one from an outbox payload alone.
    async fn read_enrichment_budget_checkpoint(
        &self,
        _graph_fname: &str,
        _source_envelope: &str,
    ) -> Result<Option<medulla_types::native_control::EnrichmentBudgetCheckpoint>, String> {
        Err("persistence backend does not support enrichment budget reads".to_string())
    }

    /// Committed operator ceiling and source sequence used by the private
    /// top-up producer. Legacy rows decode with a zero ceiling and deny.
    #[cfg(feature = "redb")]
    async fn read_enrichment_policy_revision(
        &self,
        _graph_fname: &str,
        _source_envelope: &str,
    ) -> Result<
        Option<medulla_store::redb_store::enrichment_budget::RepositoryEnrichmentPolicyRevision>,
        String,
    > {
        Err("persistence backend does not support enrichment policy reads".to_string())
    }

    /// Graph-wide pause observed before every enrichment outbox claim.
    async fn read_enrichment_budget_park(
        &self,
        _graph_fname: &str,
    ) -> Result<Option<medulla_types::native_control::EnrichmentBudgetPark>, String> {
        Err("persistence backend does not support enrichment park reads".to_string())
    }

    /// Replicated owner-row proof that one immutable source was replaced.
    /// A node with a stale local outbox event may ACK it without spending or
    /// re-parking only when this proof binds its exact snapshot digest.
    async fn read_enrichment_supersession(
        &self,
        _graph_fname: &str,
        _source_envelope: &str,
        _snapshot_digest: &str,
    ) -> Result<bool, String> {
        Err("persistence backend does not support enrichment supersession reads".to_string())
    }

    /// Persist an underfunded graph-wide pause before the consumer releases
    /// its held outbox lease. Completion means the marker has crossed the
    /// durable writer barrier; callers must never ACK unpaid work.
    async fn park_enrichment_budget(
        &self,
        _graph_fname: &str,
        _park: medulla_types::native_control::EnrichmentBudgetPark,
    ) -> Result<(), String> {
        Err("persistence backend does not support enrichment budget parking".to_string())
    }

    /// Reconstruct the registry from durable storage at boot. Returns the number
    /// of graphs loaded. Backends without a recovery implementation fail closed.
    async fn load_all(&self, _state: &Arc<RwLock<ServerState>>) -> Result<usize, String> {
        Err("persistence backend does not support graph recovery".to_string())
    }

    /// Populate the registry's CATALOG ONLY at boot (CONCEPT:MEDULLA-KG.sharding.lazy-graph-catalog, DIST-P2-3) —
    /// every graph's identity (name/type), with NO node/edge data read. Each
    /// graph's `GraphCore` then materializes lazily on first access (see
    /// `server::persistence::cold_offload::lazy_open`) via
    /// [`Self::read_graph_material_blocking`]. The default falls back to the eager
    /// `load_all` — only the redb backend (the only one with a `graph_meta`-keyed
    /// durable catalog cheaper than a full dump) overrides it. Selected by
    /// `main.rs` when `MEDULLA_LAZY_STARTUP` resolves true. Explicit env
    /// values win; an unset production profile defaults to catalog-only recovery,
    /// while development preserves eager recovery.
    async fn load_catalog(&self, state: &Arc<RwLock<ServerState>>) -> Result<usize, String> {
        self.load_all(state).await
    }

    // ── VIZ-1: hierarchical cluster-tree cache (CONCEPT:MEDULLA-KG.compute.leiden-hierarchy) ──
    //
    // Fail-closed defaults, per this trait's own "capability discovery must not
    // infer support from a backend merely being present" convention (see
    // `supports_native_resource_reservations` above): a test/memory backend simply
    // never caches, and `Method::ClusterHierarchyRefresh` still WORKS against one
    // (it just recomputes on every call instead of serving a cache) rather than
    // failing outright. Only `RedbBackend` overrides these, delegating to
    // `cluster_hierarchy_store::ClusterHierarchyStore`.

    /// Whether this backend durably caches computed cluster hierarchies.
    fn supports_cluster_hierarchy_cache(&self) -> bool {
        false
    }

    /// Replace the cached hierarchy for `graph_fname` with `blob` (a MessagePack-
    /// encoded `medulla_compute::algorithms::ClusterHierarchyResult`). Not a graph
    /// mutation — see the trait section doc above.
    async fn save_cluster_hierarchy(
        &self,
        _graph_fname: &str,
        _blob: Vec<u8>,
    ) -> Result<(), String> {
        Err("persistence backend does not support cluster hierarchy caching".to_string())
    }

    /// Fetch the cached hierarchy blob for `graph_fname`, if any has been
    /// computed (and not since evicted/superseded) for it.
    async fn load_cluster_hierarchy(&self, _graph_fname: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }

    /// Record a single DATA mutation and await its durable commit
    /// (CONCEPT:MEDULLA-KG.backend.authoritative-dispatch commit-before-ack barrier).
    /// Dispatch awaits this before acknowledging the write, so a write is never
    /// acknowledged unless it is durable. The enqueue must
    /// still fold into the SAME group-commit batch as every other concurrent
    /// awaiting writer (one fsync per batch, NOT one fsync per op) — it returns
    /// only after the `WriteTransaction` carrying this op has committed. On a
    /// durable-commit failure it returns `Err(_)`, and dispatch turns that into an
    /// ERROR response (the write did NOT land).
    ///
    /// Backpressure (NOT drop): a full writer queue must block/await capacity or
    /// fail loudly — never silently discard the mutation.
    ///
    /// Implementations that support writes must provide a real awaited commit barrier.
    async fn record_durable(&self, _graph_fname: &str, _method: &Method) -> Result<(), String> {
        Err("persistence backend does not support durable graph writes".to_string())
    }

    /// Commit one canonical mutation batch as the authoritative all-or-nothing
    /// unit.  Implementations must persist operation rows, terminal status,
    /// idempotency, result, and outbox before returning `Ok`; they must apply
    /// backpressure rather than drop work when a bounded writer queue is full.
    ///
    /// The default fails closed. Falling back to independent single-operation commits
    /// would reintroduce the exact acknowledged-partial-commit bug this contract
    /// exists to remove.
    async fn commit_mutation_batch(
        &self,
        _graph_fname: &str,
        _batch: &MutationBatch,
        _result_msgpack: Option<&[u8]>,
        _committed_at_ms: u64,
    ) -> Result<MutationBatchCommit, String> {
        Err("persistence backend does not support atomic MutationBatch commits".to_string())
    }

    /// Commit authenticated staged graph material with its MutationBatch metadata
    /// in one transaction. Implementations must verify the descriptor digest before
    /// replacing or updating any row; the default fails closed.
    ///
    /// `audited`: the caller's already-resolved `MutationPlan::audited` (or
    /// equivalent `medulla_capabilities::policy(method).audited`) for the ORIGINAL
    /// method, captured BEFORE it was compiled into this batch's operations.
    /// State-backed operations are compiled into an opaque digest receipt that no
    /// longer carries the original method's identity, so an implementation must
    /// use this flag -- not the compiled operation -- to decide whether to append
    /// a tamper-evident audit-chain entry (e.g. `TouchNodes` is durable but
    /// intentionally unaudited).
    async fn commit_mutation_batch_state(
        &self,
        _graph_fname: &str,
        _batch: &MutationBatch,
        _authoritative_state_msgpack: Vec<u8>,
        _result_msgpack: Option<&[u8]>,
        _committed_at_ms: u64,
        _audited: bool,
    ) -> Result<MutationBatchCommit, String> {
        Err("persistence backend does not support atomic staged-state commits".to_string())
    }

    /// Commit graph rows and non-topology cross-modal projections through the
    /// universal MutationBatch kernel. Implementations must write every modality,
    /// terminal status, OCC/fencing state, idempotency row, result and outbox in one
    /// durability transaction. The default fails closed; a data-only
    /// `commit_crossmodal` is not an acceptable substitute for a public mutation.
    async fn commit_mutation_batch_crossmodal(
        &self,
        _args: CrossModalCommitArgs<'_>,
    ) -> Result<MutationBatchCommit, String> {
        Err(
            "persistence backend does not support atomic cross-modal MutationBatch commits"
                .to_string(),
        )
    }

    /// Read durable terminal status for retry/restart reconciliation.  `None`
    /// means no batch with this id committed on the graph's owning shard.
    async fn read_mutation_batch(
        &self,
        _graph_fname: &str,
        _batch_id: &str,
    ) -> Result<Option<MutationBatchRecord>, String> {
        Ok(None)
    }

    /// Durable OCC version used by compact row-local batches without fetching a
    /// complete graph image.
    async fn read_mutation_graph_version(&self, _graph_fname: &str) -> Result<Option<u64>, String> {
        Ok(None)
    }

    /// Read immutable transactional outbox rows for projection/replay workers.
    async fn read_mutation_outbox(
        &self,
        _graph_fname: &str,
        _batch_id: &str,
    ) -> Result<Vec<MutationOutboxRecord>, String> {
        Ok(Vec::new())
    }

    /// Durably subscribe this projection consumer to one outbox topic on
    /// `graph_fname`'s scope.
    ///
    /// A subscription is a real precondition, not a formality: it is what bounds
    /// the ordered stream the consumer's cursor names, so the kernel refuses a
    /// claim by a consumer that has none. It is idempotent for the same topic and
    /// refused for a different one, because changing it would move every position
    /// the existing watermark already names. The default fails closed; only a
    /// backend with the durable ledger can hold a subscription.
    #[cfg(feature = "redb")]
    async fn subscribe_mutation_outbox(
        &self,
        _graph_fname: &str,
        _consumer: &str,
        _topic: &str,
    ) -> Result<(), String> {
        Err("persistence backend does not support durable outbox subscriptions".to_string())
    }

    /// Atomically lease pending outbox rows of `graph_fname`'s scope for one
    /// durable consumer. The default fails closed because a read-then-mark
    /// implementation can duplicate or lose projection work under concurrency.
    ///
    /// `budget` is the caller's, and it is deliberately a `&mut` rather than a
    /// `(now, lease, limit)` triple: a claim is bound to exactly ONE scope by
    /// construction, so DESIGN.md's per-tenant weighted round robin and its 25%
    /// consecutive-claim cap can only live in the value a sweep carries from
    /// graph to graph. A budget rebuilt inside the call would cap nothing.
    ///
    /// A full in-flight queue or a spent allowance claims nothing, writes no
    /// cursor and leaves every durable intention pending — the bounded queue's
    /// backpressure, never a drop.
    #[cfg(feature = "redb")]
    async fn claim_mutation_outbox(
        &self,
        _graph_fname: &str,
        _consumer: &str,
        _budget: &mut OutboxClaimBudget,
    ) -> Result<OutboxClaimOutcome, String> {
        Err("persistence backend does not support durable outbox claims".to_string())
    }

    /// Acknowledge an exact lease and return the watermark it advanced to.
    ///
    /// The acknowledgement IS the cursor advance: the delivery row and the
    /// `(scope, consumer)` cursor row are written in ONE transaction, so the
    /// retired two-step's crash window — acknowledged but not advanced, or
    /// advanced but not acknowledged — is not representable, and there is no
    /// second call and nothing to re-read afterwards. A superseded, expired or
    /// released lease fails `STALE_OUTBOX_LEASE` without advancing; an
    /// acknowledgement that would skip an undelivered predecessor fails
    /// `OUTBOX_ORDER_GAP`.
    ///
    /// There is no `projection` parameter. The lease carries the consumer it was
    /// issued to, and under one scope key the consumer and the projection are the
    /// same name — the retired ledger kept both only because its cursor key was
    /// `(projection, tenant, graph)` while its lease key was
    /// `(batch, ordinal, consumer)`.
    async fn ack_mutation_outbox(
        &self,
        _graph_fname: &str,
        _lease: &MutationOutboxLease,
        _now_ms: u64,
    ) -> Result<MutationProjectionCursor, String> {
        Err("persistence backend does not support durable outbox acknowledgements".to_string())
    }

    /// Release an exact held lease without acknowledging its event. An
    /// underfunded enrichment consumer first persists a graph-wide park, then
    /// calls this; future claims must read that marker before entering the
    /// outbox claim path, including after restart.
    async fn release_mutation_outbox(
        &self,
        _graph_fname: &str,
        _lease: &MutationOutboxLease,
    ) -> Result<(), String> {
        Err("persistence backend does not support durable outbox release".to_string())
    }

    /// One consumer's durable projection watermark on `graph_fname`'s scope, or
    /// `None` when it has never acknowledged a row.
    ///
    /// The cursor key is `(scope, consumer)`. There is no `tenant` parameter:
    /// a graph's ledger scope is `(GRAPH_SHARD_TENANT, graph)`, so the calling
    /// tenant is not part of the key and passing one could only be ignored or
    /// wrong. There is no separate `projection` name either, for the reason
    /// [`Self::ack_mutation_outbox`] gives.
    async fn read_mutation_projection_cursor(
        &self,
        _graph_fname: &str,
        _consumer: &str,
    ) -> Result<Option<MutationProjectionCursor>, String> {
        Ok(None)
    }

    /// Answer one operator view of a consumer's stream on `graph_fname`'s
    /// scope from one snapshot (X10). The default fails closed: only a backend
    /// with the durable ledger has a stream to show.
    #[cfg(feature = "redb")]
    async fn read_mutation_outbox_view(
        &self,
        _graph_fname: &str,
        _view: crate::outbox_operator::OutboxView,
    ) -> Result<crate::outbox_operator::OutboxViewAnswer, String> {
        Err("persistence backend does not support durable outbox views".to_string())
    }

    /// Apply one delivery-side write -- a consumer's `reject` or an
    /// operator's `rewind` -- on the single-writer authority of
    /// `graph_fname`'s shard (X10).
    #[cfg(feature = "redb")]
    async fn write_mutation_outbox(
        &self,
        _graph_fname: &str,
        _write: crate::outbox_operator::OutboxWrite,
    ) -> Result<crate::outbox_operator::OutboxWriteReply, String> {
        Err("persistence backend does not support durable outbox writes".to_string())
    }

    /// Latest lifecycle generation for a graph, used to fence stale retries.
    async fn read_mutation_lifecycle_head(
        &self,
        _graph_fname: &str,
    ) -> Result<Option<String>, String> {
        Ok(None)
    }

    /// Commit the engine-native governed ingest unit. Implementations must not
    /// decompose this into graph/object/governance/cursor writes.
    async fn commit_change_envelope(
        &self,
        _graph_fname: &str,
        _envelope: &ChangeEnvelope,
        _committed_at_ms: u64,
    ) -> Result<ChangeEnvelopeCommit, String> {
        Err("persistence backend does not support atomic ChangeEnvelope commits".to_string())
    }

    /// Engine-only repository source commit with a trusted budget seed.
    /// Public ApplyChangeEnvelope calls cannot invoke this authority path.
    #[cfg(feature = "redb")]
    #[allow(private_interfaces)]
    async fn commit_repository_change_envelope(
        &self,
        _graph_fname: &str,
        _envelope: &ChangeEnvelope,
        _authority: medulla_store::redb_store::enrichment_budget::SourceBudgetAuthority,
        _committed_at_ms: u64,
    ) -> Result<ChangeEnvelopeCommit, String> {
        Err("persistence backend does not support atomic repository budget seed".into())
    }

    /// Apply one sealed, graph-Raft enrichment top-up with an exact source
    /// supersession and replacement parent in the writer's graph transaction.
    #[cfg(all(feature = "redb", feature = "raft"))]
    async fn commit_repository_enrichment_top_up(
        &self,
        _graph_fname: &str,
        _transition: medulla_store::redb_store::enrichment_top_up::EnrichmentTopUpTransition,
        _committed_at_ms: u64,
    ) -> Result<MutationBatchCommit, String> {
        Err("persistence backend does not support replicated enrichment top-up".into())
    }

    /// Commit a BATCH of engine-native governed ingest units that all target
    /// `graph_fname`, as ONE coalesced transaction/fsync (CONCEPT:MEDULLA-KG.ingest.batched-change-envelopes).
    /// Returns one `ChangeEnvelopeCommit` per envelope, in order. `Err((index, msg))`
    /// means the batch aborted at that envelope — because the whole group shares one
    /// atomic transaction, no envelope in it committed.
    async fn commit_change_envelopes(
        &self,
        _graph_fname: &str,
        _envelopes: &[ChangeEnvelope],
        _committed_at_ms: u64,
    ) -> Result<Vec<ChangeEnvelopeCommit>, (usize, String)> {
        Err((
            0,
            "persistence backend does not support batched ChangeEnvelope commits".to_string(),
        ))
    }

    async fn read_change_envelope(
        &self,
        _graph_fname: &str,
        _envelope_id: &str,
    ) -> Result<Option<ChangeEnvelopeRecord>, String> {
        Ok(None)
    }

    async fn read_content_version(
        &self,
        _graph_fname: &str,
        _tenant: &str,
        _object_id: &str,
    ) -> Result<Option<ContentVersion>, String> {
        Ok(None)
    }

    async fn read_change_cursor(
        &self,
        _graph_fname: &str,
        _tenant: &str,
        _source: &str,
        _partition: &str,
    ) -> Result<Option<ChangeCursor>, String> {
        Ok(None)
    }

    /// Linearizable native reservation query.  Implementations must read the
    /// authoritative native rows, not a GraphCore mirror; the default fails
    /// closed so an unsupported backend cannot silently answer from stale state.
    async fn read_resource_reservation(
        &self,
        _graph_fname: &str,
        _request: &ResourceReservationStatusRequest,
    ) -> Result<ResourceReservationResult, String> {
        Err("persistence backend does not support native reservation queries".to_string())
    }

    /// Bounded native reservation status/reconciliation read.  The default is
    /// deliberately an error because status is an authority read, not an
    /// optional local projection.
    async fn read_resource_reservation_status(
        &self,
        _graph_fname: &str,
        _request: &ResourceReservationStatusRequest,
    ) -> Result<ResourceReservationStatusResult, String> {
        Err("persistence backend does not support native reservation status".to_string())
    }

    /// Native WorkItem claim-capability minting.  The default fails closed;
    /// only the redb authority owns the private capability ledger.
    #[cfg(feature = "redb")]
    async fn mint_work_item_claim_capability(
        &self,
        _graph_fname: &str,
        _request: WorkItemClaimCapabilityMintRequest,
        _authority: medulla_store::redb_store::work_item_capability::AuthenticatedAuthority,
    ) -> Result<WorkItemClaimCapabilityResult, String> {
        Err("persistence backend does not support native WorkItem claim capabilities".to_string())
    }

    /// Linearizable native WorkItem claim-capability verification.  The
    /// authority read must fail closed when a backend does not implement the
    /// private ledger.
    #[cfg(feature = "redb")]
    async fn verify_work_item_claim_capability(
        &self,
        _graph_fname: &str,
        _request: WorkItemClaimCapabilityVerifyRequest,
        _authority: medulla_store::redb_store::work_item_capability::AuthenticatedAuthority,
    ) -> Result<WorkItemClaimCapabilityResult, String> {
        Err("persistence backend does not support native WorkItem claim capabilities".to_string())
    }

    /// Native development-lane hold/quota mutation (RMDD-28): Reserve/Renew/
    /// Observe/Finish/Cleanup/UpdateQuota. `redb_store::development_lane`
    /// deliberately stops at the redb transaction boundary -- no MutationBatch/
    /// result/outbox/CDC projection, same posture as the WorkItem claim-capability
    /// ledger above. `method` must be one of the six DevelopmentLane write
    /// variants; the kernel itself validates and rejects anything else. The
    /// returned bytes are the ALREADY-msgpack-encoded typed result (the kernel's
    /// own `rmp_serde::to_vec_named` output) -- callers hand them straight to
    /// `ResultPayload::Raw`, never re-encode them. The default fails closed:
    /// only the redb authority owns the private lane ledger.
    async fn commit_development_lane(
        &self,
        _graph_fname: &str,
        _method: Method,
        _now_ms: u64,
    ) -> Result<Vec<u8>, String> {
        Err("persistence backend does not support native development-lane mutations".to_string())
    }

    /// Exact authenticated native development-lane hold/tombstone read (RMDD-28).
    /// An MVCC snapshot read, same posture as [`Self::read_resource_reservation`]
    /// above -- never routed through the writer thread. The default fails closed.
    async fn read_development_lane(
        &self,
        _graph_fname: &str,
        _request: &DevelopmentLaneQueryRequest,
        _now_ms: u64,
    ) -> Result<DevelopmentLaneQueryResult, String> {
        Err("persistence backend does not support native development-lane queries".to_string())
    }

    /// Bounded native development-lane tenant status page (RMDD-28), served from
    /// the maintained indexes/counters. An MVCC snapshot read, same posture as
    /// [`Self::read_resource_reservation_status`] above. The default fails closed.
    async fn read_development_lane_status(
        &self,
        _graph_fname: &str,
        _request: &DevelopmentLaneStatusRequest,
        _now_ms: u64,
    ) -> Result<DevelopmentLaneStatusResult, String> {
        Err("persistence backend does not support native development-lane status".to_string())
    }

    /// Durably register a graph's identity (CONCEPT:MEDULLA-KG.backend.authoritative-dispatch). The
    /// per-mutation write path persists node/edge rows but not the graph's
    /// name/type, yet `load_all` rebuilds the registry from a durable graph manifest
    /// (redb `graph_meta`). So dispatch calls this when a graph is created (and it
    /// must commit durably) so a `kill -9` between creation and the next checkpoint
    /// still recovers the graph under its real name/type. Idempotent and fail-closed
    /// unless implemented by the backend.
    async fn register_graph(
        &self,
        _graph_fname: &str,
        _name: &str,
        _graph_type: medulla_types::protocol::GraphType,
    ) -> Result<(), String> {
        Err("persistence backend cannot register graph identity".to_string())
    }

    /// Durably remove ALL state for a graph when its tenant is DELETED
    /// (CONCEPT:MEDULLA-KG.backend.tenant-delete-recreate-same). The default per-mutation write path persists rows keyed by
    /// the graph's sanitized name but has NO row-removal on `DeleteGraph`; that left
    /// a deleted tenant's nodes/edges/meta resident in the durable tier, so a
    /// recreate of the SAME name inherited them via the read-through-on-RAM-miss path
    /// and via `load_all` — silently corrupting/dropping the new tenant's writes.
    /// Dispatch awaits this on `DeleteGraph` so the purge is
    /// COMMITTED before the delete is acked (commit-before-ack), making a same-name
    /// recreate start from a clean durable slate with no race. Idempotent and
    /// fail-closed unless implemented by the backend.
    async fn purge_graph(&self, _graph_fname: &str) -> Result<(), String> {
        Err("persistence backend cannot purge graph state".to_string())
    }

    /// Read a single node's stored properties back from the durable tier
    /// (CONCEPT:MEDULLA-KG.backend.authoritative-dispatch read-through). Returns `Ok(None)` when the node is not
    /// present durably, `Ok(Some(props))` when it is. The default is `Ok(None)`
    /// — only an authoritative backend that can serve a RAM-miss read implements
    /// it. Provided so a future read-through-on-eviction path has a backend seam
    /// without another trait revision; see the eviction note in `persist.rs`.
    async fn read_node(
        &self,
        _graph_fname: &str,
        _node_id: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }

    /// Confirm durable presence for a batch of eviction candidates. The default
    /// fails closed (all false); an authoritative backend overrides this with one
    /// storage snapshot so a memory sweep never opens one transaction per node.
    fn durable_node_presence(
        &self,
        _graph_fname: &str,
        node_ids: &[String],
    ) -> Result<Vec<bool>, String> {
        Ok(vec![false; node_ids.len()])
    }

    /// SYNC read-through of a single node's stored properties (CONCEPT:MEDULLA-KG.storage.read-through-seam-exercised).
    /// The medulla-core read path (`GraphCore::get_node_properties`) is synchronous, so
    /// the read-through it consults on a RAM miss must be sync too. The redb backend
    /// already performs its point-read over a blocking channel to its off-reactor
    /// writer thread, so a sync variant is natural and adds no async runtime to
    /// medulla-core. Same return contract as [`read_node`].
    fn read_node_blocking(
        &self,
        _graph_fname: &str,
        _node_id: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }

    /// SYNC durable-material fetch for a lazy first-open (CONCEPT:MEDULLA-KG.sharding.lazy-graph-catalog,
    /// DIST-P2-3): the WHOLE graph's nodes/edges/semantic-store blob, replayed into
    /// a freshly constructed `GraphCore` on catalog-only → resident promotion
    /// (mirrors `read_node_blocking`'s sync read-through contract, but for a whole
    /// graph rather than one node). Default `Ok(None)` — only the redb backend
    /// (which alone has a durable per-graph dump cheaper than reconstructing from
    /// scratch) overrides it; a backend with no lazy-materialize support just
    /// gets an empty core on lazy-open (the rebuildable-cache model's contract).
    fn read_graph_material_blocking(
        &self,
        _graph_fname: &str,
    ) -> Result<Option<medulla_core::registry::GraphMaterial>, String> {
        Ok(None)
    }

    /// Fetch the complete authoritative persistent graph image for an isolated
    /// runtime-result mutation. Unlike the lazy materializer shape this includes
    /// ledger state, and unlike a live RAM snapshot it cannot omit evicted rows.
    async fn read_authoritative_graph_snapshot(
        &self,
        _graph_fname: &str,
    ) -> Result<Option<(medulla_core::graph::GraphSnapshot, u64)>, String> {
        Err("persistence backend cannot stage an authoritative graph snapshot".to_string())
    }

    /// SYNC bounded-PAGE durable-material fetch for a paged lazy first-open
    /// (CONCEPT:MEDULLA-KG.sharding.paged-lazy-open, L38 "paged adjacency") — the working-subset
    /// counterpart of [`Self::read_graph_material_blocking`] backing
    /// `medulla_core::registry::GraphRegistry::open_lazy_paged`/`page_in`, mirroring
    /// `medulla_core::registry::GraphMaterializer::materialize_page`'s own default/
    /// override split one layer up. The default here falls back to ONE full
    /// [`Self::read_graph_material_blocking`] fetch and then slices it in memory —
    /// correct for ANY backend, but it does not avoid the full-fetch cost AT THE
    /// SOURCE (documented in `medulla_core::registry::GraphMaterializer::materialize_page`'s
    /// own doc comment). Only `RedbBackend` overrides this with a
    /// genuinely bounded scan straight off its durable store (never collecting the
    /// whole graph into memory first) — the concrete fix for the L38 ledger item.
    fn read_graph_material_page_blocking(
        &self,
        graph_fname: &str,
        cursor: Option<medulla_core::registry::MaterializeCursor>,
        page_size: usize,
    ) -> Result<Option<medulla_core::registry::MaterialPage>, String> {
        // Reuse medulla-core's OWN `materialize_page` default slicing algorithm (rather
        // than duplicating its offset arithmetic here) by wrapping the one full
        // fetch in a throwaway `GraphMaterializer` that just returns it. The trait
        // must be IN SCOPE for its default `materialize_page` method to be callable.
        use medulla_core::registry::GraphMaterializer as _;
        struct Once(Option<medulla_core::registry::GraphMaterial>);
        impl medulla_core::registry::GraphMaterializer for Once {
            fn materialize(
                &self,
                _graph_name: &str,
            ) -> Option<medulla_core::registry::GraphMaterial> {
                self.0.clone()
            }
        }
        let material = self.read_graph_material_blocking(graph_fname)?;
        Ok(Once(material).materialize_page(graph_fname, cursor, page_size))
    }

    /// **Cross-modal ACID commit (CONCEPT:MEDULLA-KG.txn.reader-never-sees-node + MEDULLA-360).** Land a graph + vector +
    /// blob-ref + measurement + property write-set for ONE graph atomically. The redb
    /// backend overrides this to commit ALL modalities in ONE `WriteTransaction`
    /// (commit-before-ack): on any error nothing lands (full rollback — no partial
    /// cross-modal commit). The default implementation durably commits graph methods;
    /// vectors/blob-refs/measurements require a backend that implements their atomic
    /// storage rather than silently dropping a modality.
    ///
    /// Note (MEDULLA-360): staged OWL axioms + SPARQL CONSTRUCT triples are lowered to
    /// graph-native `AddNode`/`AddEdge` at STAGE time and arrive folded into `methods`,
    /// so they ride this same durable path as ordinary graph mutations; only the
    /// genuinely non-graph modalities (vectors/blob-refs/measurements) get their own
    /// arguments + off-redb guard.
    async fn commit_crossmodal(
        &self,
        graph_fname: &str,
        methods: &[Method],
        vectors: &[(String, Vec<f32>)],
        blob_refs: &[(String, String)],
        measurements: &[medulla_store::MeasurementBatch],
    ) -> Result<(), String> {
        if !vectors.is_empty() || !blob_refs.is_empty() || !measurements.is_empty() {
            return Err(
                "cross-modal txn (vectors / blob-refs / measurements) requires the redb persistence backend"
                    .to_string(),
            );
        }
        for m in methods {
            if crate::mutation_apply::is_durable_mutation(m) {
                self.record_durable(graph_fname, m).await?;
            }
        }
        Ok(())
    }

    /// Flush and stop any background writer threads at graceful shutdown.
    /// Idempotent.
    fn shutdown(&self);

    /// Downcast hook (CONCEPT:MEDULLA-KG.storage.one-fsync-covers-raft / KG-2.231). The Raft store needs the CONCRETE
    /// `RedbBackend` to reach its durable-log API (which is not part
    /// of this trait — the log is a Raft concern, not a general persistence one); the
    /// `security` audit path likewise needs it for `audit_verify_blocking`. So this
    /// lets a caller recover the concrete type from the `Arc<dyn PersistenceBackend>`
    /// in `ServerState`. The default returns `None`; only the redb backend overrides it.
    /// Gated on `redb` (the only build where `RedbBackend` exists) — also consumed by the
    /// M3 catalog-driven resharding admin RPC (CONCEPT:MEDULLA-KG.backend.m3-admin-dispatch).
    ///
    /// Returns the backend as [`std::any::Any`] so the persistence crate's
    /// `AsRedb::as_redb` extension can downcast to the concrete `RedbBackend`.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }

    /// Walk one graph's tamper-evident audit chain on the owner thread. Only a
    /// backend that stores an audit chain implements it.
    #[cfg(feature = "security")]
    fn audit_verify_blocking(
        &self,
        _graph_fname: &str,
    ) -> Result<medulla_types::protocol::AuditReport, String> {
        Err("audit verification is not supported by this backend".to_string())
    }
}

/// Borrowed carrier for [`PersistenceBackend::commit_mutation_batch_crossmodal`]'s
/// arguments, bundled so the trait method (and every implementation of it) stays
/// under the clippy argument-count ceiling.
pub struct CrossModalCommitArgs<'a> {
    pub graph_fname: &'a str,
    pub batch: &'a MutationBatch,
    pub methods: &'a [Method],
    pub vectors: &'a [(String, Vec<f32>)],
    pub blob_refs: &'a [(String, String)],
    pub measurements: &'a [medulla_store::MeasurementBatch],
    pub result_msgpack: Option<&'a [u8]>,
    pub committed_at_ms: u64,
}

/// Parse `MEDULLA_RAFT_GROUPS` (DIST-P2-2 / ADR-2 W1.2): `None`/empty/`"0"` all
/// collapse to `default` rather than erroring — an operator who never heard of this knob
/// gets the cores-derived default ([`default_raft_group_count`]). A non-empty,
/// non-integer value is a loud misconfig (same posture as `parse_peers`). A configured
/// value is clamped to `1..=MAX_SHARD_COUNT` because under raft K (redb shards) == N
/// (groups), so N can never exceed the shard ceiling ([`crate::redb_layout::MAX_SHARD_COUNT`]).
/// Pure (no env access) so it is unit tested directly without the process-global
/// env-var races a `from_env` test would need.
pub fn parse_groups(raw: Option<String>, default: u64) -> Result<u64, String> {
    let ceiling = medulla_store::redb_layout::MAX_SHARD_COUNT as u64;
    // Absent / empty / an explicit `0` all mean "unspecified" ⇒ the cores-derived
    // `default` (preserving the pre-ADR-2 "None/empty/0 collapse together" semantic,
    // only the collapse target changed from a hardcoded 1 to `default`). A positive
    // value is honored, clamped to `1..=MAX_SHARD_COUNT` (K == N ≤ the shard ceiling).
    let n = match raw {
        Some(v) if !v.trim().is_empty() => v
            .trim()
            .parse::<u64>()
            .map_err(|_| "MEDULLA_RAFT_GROUPS is not an integer".to_string())?,
        _ => 0,
    };
    let resolved = if n == 0 { default } else { n };
    Ok(resolved.clamp(1, ceiling))
}

/// The cores-derived default group/shard count when `MEDULLA_RAFT_GROUPS` is
/// unset (ADR-2 / W1.2, `reports/wave1/ADR-scale-trio.md` §ADR-2 decision 2): the raft
/// group count defaults to the non-raft durable-shard auto-size
/// `clamp(effective-cgroup-cpu/2, 1, …)` but
/// with the raised [`crate::redb_layout::MAX_SHARD_COUNT`] ceiling — so turning on raft
/// (for HA) gets the SAME write-sharding a single-node deployment gets by default rather
/// than collapsing to one group. Under raft K (redb shards) == N (groups), so this is the
/// ONE default both `resolve_shard_count` and [`raft_group_count`] derive from.
pub fn default_raft_group_count() -> u64 {
    let cpus = medulla_kernel::autosize::detect_capacity().reserved_cpus() as u64;
    (cpus / 2).clamp(1, medulla_store::redb_layout::MAX_SHARD_COUNT as u64)
}

/// The number of raft groups (== redb shards K, ADR-2 / W1.2) this node runs, resolved
/// from `MEDULLA_RAFT_GROUPS` with the [`default_raft_group_count`] fallback. Read
/// by `resolve_shard_count` so the durable store opens EXACTLY N shards — group `g` owns
/// shard `g`. A malformed value falls back to the default here; the loud validation error
/// is raised once by `RaftClusterConfig::from_env`, which gates startup, so this
/// accessor (reached only on an already-accepted raft-configured node) never needs to
/// re-report it.
pub fn raft_group_count() -> u64 {
    parse_groups(
        std::env::var("MEDULLA_RAFT_GROUPS").ok(),
        default_raft_group_count(),
    )
    .unwrap_or_else(|_| default_raft_group_count())
}

#[cfg(test)]
mod raft_group_tests {
    use super::*;

    #[test]
    fn parse_groups_uses_default_when_absent_or_empty_or_zero() {
        // ADR-2 / W1.2: absent/empty/0 fall back to the supplied cores-derived default
        // (not a hardcoded 1), because under raft K == N and the default is the same
        // write-sharding the non-raft path uses.
        assert_eq!(parse_groups(None, 4).unwrap(), 4);
        assert_eq!(parse_groups(Some("".into()), 4).unwrap(), 4);
        assert_eq!(parse_groups(Some("  ".into()), 4).unwrap(), 4);
        assert_eq!(parse_groups(Some("0".into()), 4).unwrap(), 4);
        // A default of 0 (impossible from the resolver, defensive) still floors to 1.
        assert_eq!(parse_groups(None, 0).unwrap(), 1);
    }

    #[test]
    fn parse_groups_accepts_n_clamps_ceiling_and_rejects_garbage() {
        assert_eq!(parse_groups(Some("4".into()), 1).unwrap(), 4);
        assert_eq!(parse_groups(Some(" 7 ".into()), 1).unwrap(), 7);
        // K == N ≤ MAX_SHARD_COUNT: an over-ceiling group count clamps rather than
        // opening more shards than the durable layout allows.
        assert_eq!(
            parse_groups(Some("9999".into()), 1).unwrap(),
            medulla_store::redb_layout::MAX_SHARD_COUNT as u64
        );
        assert!(parse_groups(Some("nope".into()), 1).is_err());
    }

    #[test]
    fn default_raft_group_count_is_bounded() {
        // Cores-derived, always in 1..=MAX_SHARD_COUNT regardless of the host.
        let n = default_raft_group_count();
        assert!((1..=medulla_store::redb_layout::MAX_SHARD_COUNT as u64).contains(&n));
    }
}

/// Adapts a persistence backend to the kernel's durable-presence probe.
pub struct BackendPresence(pub Arc<dyn PersistenceBackend>);

impl medulla_kernel::kernel::persistence::DurableNodePresence for BackendPresence {
    fn durable_node_presence(
        &self,
        graph_fname: &str,
        node_ids: &[String],
    ) -> Result<Vec<bool>, String> {
        self.0.durable_node_presence(graph_fname, node_ids)
    }
}

/// Deserializes a field that must be present (an explicit `null` is allowed).
pub fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer)
}

/// Privacy-safe request authority replicated with an ordinary Raft graph write.
/// The principal is already a one-way fingerprint; raw caller identity is never
/// copied into the consensus log or durable mutation ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RaftMutationContext {
    pub batch_id: String,
    pub request_id: u64,
    /// Authenticated transport nonce carried into the mutation envelope on the
    /// state-machine apply path. Internal control-plane entries leave this
    /// absent; replicated caller mutations must preserve it end to end.
    pub attempt_nonce: Option<medulla_types::contract::Nonce>,
    /// Opaque tenant scope derived from the verified carrier. Raw tenant names are
    /// never copied into consensus or MutationBatch authority.
    pub tenant_scope: String,
    pub principal_fingerprint: String,
    /// True only when the verified leader boundary admitted the exact one-time
    /// identity bootstrap. Followers use this mandatory bit to apply the same
    /// atomic bootstrap transition rather than re-authorizing from opaque claims.
    pub identity_bootstrap: bool,
    pub placement_epoch: u64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub fencing_token: Option<u64>,
    pub created_at_ms: u64,
}

/// The timing and placement fence attached to an already verified mutation.
/// Grouping these adjacent fields keeps the request-boundary constructor small
/// without changing the serialized authority shape.
pub struct RaftMutationTiming {
    pub placement_epoch: u64,
    pub fencing_token: Option<u64>,
    pub created_at_ms: u64,
}

impl RaftMutationContext {
    /// Construct caller authority only from the already-verified, privacy-safe
    /// carrier facts at the dispatch boundary.
    pub fn from_verified_request(
        batch_id: String,
        request_id: u64,
        attempt_nonce: Option<medulla_types::contract::Nonce>,
        tenant_scope: &str,
        principal_fingerprint: String,
        identity_bootstrap: bool,
        timing: RaftMutationTiming,
    ) -> Result<Self, String> {
        let context = Self {
            batch_id,
            request_id,
            attempt_nonce,
            tenant_scope: tenant_scope.to_string(),
            principal_fingerprint,
            identity_bootstrap,
            placement_epoch: timing.placement_epoch,
            fencing_token: timing.fencing_token,
            created_at_ms: timing.created_at_ms,
        };
        context.validate()?;
        if context.tenant_scope == Self::internal_tenant_scope() {
            return Err(
                "verified Raft authority cannot claim the internal tenant scope".to_string(),
            );
        }
        Ok(context)
    }

    /// Construct engine-owned control-plane authority. The tenant and principal
    /// are fixed opaque digests, while the child batch id deterministically binds
    /// the operation to its graph and coordinator without persisting either raw id.
    pub fn internal(
        namespace: &str,
        graph: &str,
        coordinator_id: &str,
        request_id: u64,
        created_at_ms: u64,
    ) -> Self {
        Self {
            batch_id: crate::mutation_batch::opaque_coordinator_key(
                namespace,
                graph,
                coordinator_id,
            ),
            request_id,
            attempt_nonce: None,
            tenant_scope: Self::internal_tenant_scope(),
            principal_fingerprint: Self::internal_principal_fingerprint(),
            identity_bootstrap: false,
            placement_epoch: 0,
            fencing_token: None,
            created_at_ms,
        }
    }

    pub fn internal_tenant_scope() -> String {
        crate::mutation_batch::opaque_coordinator_key(
            "raft-internal-tenant",
            "control-plane",
            "authority",
        )
    }

    pub fn internal_principal_fingerprint() -> String {
        crate::mutation_batch::opaque_coordinator_key(
            "principal:sha256",
            "medulla-raft-control-plane",
            "authority",
        )
    }

    pub fn is_internal(&self) -> bool {
        self.tenant_scope == Self::internal_tenant_scope()
            && self.principal_fingerprint == Self::internal_principal_fingerprint()
            && self.attempt_nonce.is_none()
            && !self.identity_bootstrap
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.batch_id.trim().is_empty() {
            return Err("Raft mutation batch authority must not be empty".to_string());
        }
        if !opaque_scope_is_valid(&self.tenant_scope) {
            return Err("Raft mutation tenant authority must be an opaque scope".to_string());
        }
        let principal = self
            .principal_fingerprint
            .strip_prefix("principal:sha256:")
            .filter(|digest| lowercase_sha256_is_valid(digest));
        if principal.is_none() {
            return Err("Raft mutation principal authority must be an opaque digest".to_string());
        }
        if self.placement_epoch > 0 && self.fencing_token.is_none() {
            return Err("placed Raft mutation authority requires a fencing token".to_string());
        }
        Ok(())
    }
}

/// Whether an opaque scope string is well formed.
pub fn opaque_scope_is_valid(value: &str) -> bool {
    value.rsplit_once(':').is_some_and(|(namespace, digest)| {
        !namespace.is_empty() && lowercase_sha256_is_valid(digest)
    })
}

fn lowercase_sha256_is_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

// ── Materialized views (CONCEPT:MEDULLA-KG.storage.feature) ──────────────────────────────────────

/// A named, incrementally-maintained materialized view of a distributed-compute result
/// (CONCEPT:MEDULLA-KG.storage.feature). Persisted in the redb durable tier so it survives restart; the
/// handler refreshes it incrementally on a delta. The definition (graphs + algo) is
/// stored alongside the rows so a `RefreshMatView` can recompute without re-specifying.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MatView {
    pub name: String,
    pub graphs: Vec<String>,
    pub algo: medulla_types::protocol::DistAlgo,
    pub result: medulla_types::compute_result::distributed::DistResult,
}

/// A flat map of named materialized views the engine maintains (CONCEPT:MEDULLA-KG.storage.feature). The
/// in-RAM index; the durable copy lives in redb (the handler persists + reloads). The
/// `BTreeMap` keeps a deterministic order for listing.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MatViewStore {
    pub views: std::collections::BTreeMap<String, MatView>,
}

impl MatViewStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn put(&mut self, view: MatView) {
        self.views.insert(view.name.clone(), view);
    }
    pub fn get(&self, name: &str) -> Option<&MatView> {
        self.views.get(name)
    }
}
