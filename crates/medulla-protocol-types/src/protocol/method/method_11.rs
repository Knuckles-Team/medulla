macro_rules! __eg_method_chunk_11 {
    (@acc [$($variants:tt)*]) => {
        __eg_method_finish!(@acc [
$($variants)*

    PublishRunCheckpoint { request: medulla_work_types::run_checkpoint::PublishRunCheckpointRequest },
    GetRunCheckpoint { request: medulla_work_types::run_checkpoint::GetRunCheckpointRequest },


    PublishCurrentPlan { request: medulla_decision_outcome_types::current_plan::PublishCurrentPlanRequest },
    GetCurrentPlan { tenant: String, run_id: String },
    /// Caller-owned committed measurements from one bounded snapshot.
    GetSessionUsage { tenant: String, context_id: String },
    /// The configured shard-writer count `k` for the verified tenant's graphs
    /// (MEDULLA-DURABLE-KERNEL-R077); write-lane slot grants are capped at it.
    GetShardWriterWidth { tenant: String },
    /// Historical native run admission, visible only to its verified immutable
    /// owner. Missing and foreign rows return null; missing original admission
    /// evidence refuses explicitly. Independent of current lease or terminal state.
    GetRunWorkItem { tenant: String, run_id: String },

    // ── Typed WorkItem reads (EH-219) ──────────────────────────────────────
    /// The caller's view of one native WorkItem row, or `null` when no row with
    /// this id is visible to `tenant`. `tenant` must equal the verified request
    /// tenant (`ACCESS_DENIED` otherwise); lease owner, epoch and fencing token
    /// are never returned. See [`crate::work_item_read`].
    GetWorkItem {
        tenant: String,
        work_item_id: String,
    },
    /// Record, under the live claim, that the worker is about to cause an
    /// external effect (MEDULLA-DURABLE-KERNEL-R820.3). Scope `work:write`.
    MarkWorkItemEffectStarted {
        request: medulla_work_types::work_item_read::MarkWorkItemEffectStartedRequest,
    },
    /// Resolve a started effect with its one receipt for `(work_item_id,
    /// attempt)`. An identical replay returns the stored receipt.
    RecordEffectReceipt {
        request: medulla_work_types::work_item_read::RecordEffectReceiptRequest,
    },
    /// Store the admitted request's digest and payload ref once under
    /// `(work_item_id, attempt)`.
    SubmitAdmittedRequest {
        request: medulla_work_types::work_item_read::SubmitAdmittedRequestRequest,
    },
    /// The receipt of `(work_item_id, attempt)`, or `null`. Scope `work:read`.
    GetEffectReceipt {
        request: medulla_work_types::work_item_read::EffectRecordKey,
    },
    /// The admitted request's digest and payload ref, or `null` when absent
    /// or another tenant's or session's. Scope `work:read`.
    GetAdmittedRequest {
        request: medulla_work_types::work_item_read::EffectRecordKey,
    },
    /// One bounded page of `tenant`'s WorkItems, optionally of one `kind`.
    /// Pages by the limit / scan / byte bounds of `AgentComponent.Search`; an
    /// empty page may still carry `next_cursor`. The cursor is opaque and
    /// bound to the tenant it was minted for.
    ListWorkItems {
        tenant: String,
        #[serde(default)]
        cursor: Option<String>,
        limit: u32,
        #[serde(default)]
        kind: Option<String>,
        /// Keep only items whose top-level `metadata` holds every one of these
        /// key/value pairs exactly (at most 8 keys) -- e.g. a correlation id or
        /// an event subject (graph-os MEDULLA-5). Filters within the same bounded
        /// scan, so it never widens a page's cost.
        #[serde(default)]
        metadata_match: Option<serde_json::Map<String, serde_json::Value>>,
    },

    /// A terminal WorkItem of `tenant` and the provenance (RunTrace /
    /// ToolCall refs and the verified OutcomeEvaluation receipt) its native
    /// `CommitWorkItemResult { outcome_extension }` bound, or `null` when the
    /// item is not visible or carries no committed outcome (graph-os MEDULLA-3).
    GetWorkItemOutcome {
        tenant: String,
        work_item_id: String,
    },

    // ── Native control leases (graph-os MEDULLA-2) ──────────────────────────────
    /// Issue one active control lease: an immutable, time-boxed grant record.
    /// A row already holding the id answers `collision` and is left untouched.
    /// `request.tenant` must equal the verified request tenant. See
    /// [`crate::control_lease`].
    /// Persist a grant from an explicitly authorized host-start issuer.
    IssueHostStartGrant { request: medulla_work_types::host_start::IssueHostStartGrantRequest },
    IssueControlLease {
        request: medulla_work_types::control_lease::IssueControlLeaseRequest,
    },
    /// Move one control lease along a legal edge (`active -> consumed |
    /// revoked | expired`, `consumed -> revoked | expired`), compare-and-set
    /// on the revision the caller read. A lease never returns to `active`.
    TransitionControlLease {
        request: medulla_work_types::control_lease::TransitionControlLeaseRequest,
    },
    /// The caller's view of one control lease, or `null` when no lease with
    /// this id is visible to `tenant` (which must equal the verified tenant).
    GetControlLease {
        tenant: String,
        lease_id: String,
    },

    // ── Declared freshness (EH-400) ────────────────────────────────────────
    /// The request graph's per-class invalidation events with `version >
    /// after_version` (at most `limit`; 0 = the default page), its
    /// `eg:volatilityClass` policy when that changed since `policy_after`
    /// (`None` = always include it), and the watermark freshness of every foreign
    /// source the graph holds a watermark for. A caller that sees `gap`, or an
    /// `epoch` different from the one it last saw, must drop everything it cached
    /// for the graph. See [`crate::freshness`].
    FreshnessFeed {
        after_version: u64,
        #[serde(default)]
        limit: u32,
        #[serde(default)]
        policy_after: Option<u64>,
    },

    // ── Policy evolution (EH-346 / EH-347) ────────────────────────────────
    /// Capture-first open-weight policy evolution: the attested capability,
    /// trajectory captures, immutable model-policy versions, external
    /// training-run receipts and held-out evaluation receipts. Every record is
    /// immutable and content-addressed in the request graph; each write
    /// self-translates into exactly one `CreateNodeIfAbsent`. EG records and
    /// relates -- it never trains. See [`crate::policy_evolution`].
    PolicyEvolution {
        op: Box<medulla_decision_outcome_types::policy_evolution::PolicyEvolutionOp>,
    },
    /// ENGINE-INTERNAL: the durable WorkItem-kernel write that stores one
    /// record `PolicyEvolution` already admitted. Refused from the wire; the
    /// only way a policy-evolution row can be created (generic graph writes to
    /// such rows are refused by the row guard).
    PolicyEvolutionStore {
        request: Box<medulla_decision_outcome_types::policy_evolution::StoredPolicyRecord>,
    },
    /// One bounded page of `request.tenant`'s control leases, filtered by
    /// kind, status and exact grant pairs (graph-os MEDULLA-5: the pending
    /// `action.approval` queue). `null`-free: an empty page may still carry a
    /// cursor.
    ListControlLeases {
        request: medulla_work_types::control_lease::ListControlLeasesRequest,
    },

    // ── Work market (EH-348) ───────────────────────────────────────────────
    /// Fold evidence into `request.tenant`'s ONE canonical Gap and ensure its
    /// native WorkItem, atomically: a new Gap is created together with its
    /// WorkItem or not at all; evidence the Gap has not seen reopens a closed
    /// Gap as a new generation with a new WorkItem; evidence it has seen
    /// changes nothing. See [`crate::work_market`].
    GapUpsert {
        request: medulla_work_types::work_market::GapUpsertRequest,
    },
    /// Move a Gap along a legal lifecycle edge, compare-and-set on the
    /// revision the caller read.
    GapTransition {
        request: medulla_work_types::work_market::GapTransitionRequest,
    },
    /// Record the Gap's current WorkItem outcome, read by the engine from the
    /// WorkItem row, as evidence on the Gap; a terminal outcome closes a live
    /// Gap (succeeded -> resolved, otherwise deferred).
    GapSettle {
        request: medulla_work_types::work_market::GapSettleRequest,
    },
    /// Record the versioned, derived pricing of a live Gap's current
    /// WorkItem, compare-and-set on the offer version the caller read. The
    /// engine computes the deterministic utility rate.
    WorkOfferPut {
        request: medulla_work_types::work_market::WorkOfferPutRequest,
    },
    /// The caller's view of one Gap, or `null` when no Gap with this id is
    /// visible to `tenant` (which must equal the verified tenant).
    GapGet {
        tenant: String,
        gap_id: String,
    },
    /// One bounded page of `request.tenant`'s Gaps in row-key order,
    /// optionally of one status and source. A listing, never a ranking.
    GapList {
        request: medulla_work_types::work_market::GapListRequest,
    },

    // ── Telemetry facts (EH-408 / EH-409) ──────────────────────────────────
    /// Derive ontology-bound telemetry facts in the request graph. Reads the
    /// caller's stored log records (from the named `streams`, each of which
    /// must lie in the caller's tenant namespace), metric samples and spans
    /// over `[from_ms, to_ms)`; binds them to the graph's declared Server /
    /// Service / Host / Workload / Agent individuals; rolls them up, runs the
    /// declared CEP patterns and the declared-health conformance check; and
    /// writes every fact in one `BatchUpdate` against the request graph.
    /// `policy_msgpack` is a MessagePack `medulla_stream::telemetry::TelemetryPolicy`.
    /// See [`crate::telemetry_derive`].
    TelemetryDerive {
        from_ms: u64,
        to_ms: u64,
        #[serde(default)]
        streams: Vec<String>,
        #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
        #[serde(with = "serde_bytes")]
        policy_msgpack: Vec<u8>,
    },

    /// The caller tenant's stored metrics as `{name: latest value}` over the last
    /// `telemetry.snapshot_window` seconds (MEDULLA-ORCH-VOCAB-R804). `tenant`
    /// must be the verified request tenant, otherwise `ACCESS_DENIED`. Read-only;
    /// derives nothing. Gated `telemetry:read`.
    GetTelemetrySnapshot { tenant: String },

    // ── Served incidents (MEDULLA-UNIFIED-DATA-PLANE-R760.3) ───────────────
    /// Record one anomaly in the caller's tenant (from the verified carrier)
    /// and correlate it into an open incident over `window_ms`.
    IncidentRecordAnomaly {
        anomaly: medulla_types::incident::AnomalyReading,
        window_ms: u64,
    },
    /// Correlate a set of anomalies in one pass: grouped by entity and
    /// `window_ms` bucket, each group opens or joins an open incident.
    IncidentCorrelate {
        anomalies: Vec<medulla_types::incident::AnomalyReading>,
        window_ms: u64,
    },
    /// One incident of the caller's tenant with its evidence. Read-only.
    IncidentEvidence { incident_id: String },
    /// The `Claim` nodes with an `about` edge to an entity in the incident's
    /// evidence. Read-only.
    IncidentRelatedClaims {
        incident_id: String,
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Move an incident one status forward (open -> acknowledged -> resolved).
    IncidentSetStatus {
        incident_id: String,
        status: medulla_types::incident::IncidentStatus,
    },

    // ── Governed actions (MEDULLA-UNIFIED-DATA-PLANE-R700.3) ───────────────
    /// Validate and register one action spec in the request graph's registry.
    ActionRegister {
        spec: medulla_work_types::action_spec::ActionSpec,
    },
    /// Submit typed parameters to a registered action: a failing criterion
    /// applies no effect; a passing submission applies every effect in one
    /// batch. Either way the run is recorded.
    ActionSubmit {
        action: String,
        params: std::collections::BTreeMap<String, medulla_work_types::action_spec::ParamValue>,
    },

    // ── Dataset lineage (MEDULLA-UNIFIED-DATA-PLANE-R702.3) ─────────────────
    /// Drain lineage run events into the request graph: each event maps to
    /// content-addressed `Dataset` nodes and `derivesInto` edges; an event that
    /// cannot map is kept as one `QuarantinedLineageEvent` node with its payload
    /// digest. The batch commits as one governed ChangeEnvelope keyed by its
    /// write-set digest, so a replayed event adds no edge.
    LineageIngest {
        events: Vec<medulla_catalog_types::lineage::LineageEvent>,
    },

    // ── Market bars and trend signals (EH-413..EH-418) ─────────────────────
    /// Typed OHLCV bars over the time-series store, deterministic integer
    /// indicators, the per-series trend signal with its flip records, the
    /// Decide-calibrated flip confidence and the backtest-run record. Pure
    /// compute over what the request carries; informational only — no op
    /// authorises an order. See [`crate::compute_result::market`].
    #[cfg(feature = "finance")]
    FinanceMarket {
        op: Box<medulla_result_types::compute_result::market::FinanceMarketOp>,
    },

    // ── Signal fusion and the strategic-insider model (EH-423 / AUD-30) ─────
    /// Sequential Bayesian fusion of directional calls weighted by measured
    /// priors, and the Kyle insider's equilibrium under dynamic legal risk
    /// with its schedule and penalty-design verdict. Pure compute over what
    /// the request carries; informational only. See
    /// [`crate::compute_result::signal_models`].
    #[cfg(feature = "finance")]
    FinanceSignalModels {
        op: Box<medulla_result_types::compute_result::signal_models::FinanceSignalModelsOp>,
    },

    // ── Numeric kernel evaluation (MEDULLA-FINANCE-PRIMITIVES-R002.2.5) ──────
    /// One shared numeric kernel evaluated over the arrays the request carries:
    /// rolling z-score, EWMA, skill and breadth, backtest validation, Kalman
    /// filters, Ornstein-Uhlenbeck, ADF and the regime HMM. Pure compute; no
    /// store, clock or graph core. See [`medulla_types::numeric_kernel`].
    NumericEvaluate {
        op: Box<medulla_types::numeric_kernel::NumericEvaluateOp>,
    },

    // ── Sealed records (EH-558) ─────────────────────────────────────────────
    /// Retire one sealed record: the owning op, and the only way to remove a
    /// sealed record's content. With the record's digest it replaces the row by
    /// an audited tombstone (class, digest, who, when, why) in one durable,
    /// idempotent native transaction; generic node writes may not change or
    /// remove a sealed row. `request.tenant` must equal the verified request
    /// tenant, and the engine stamps `retired_by` from the verified caller. See
    /// [`crate::sealed_record`].
    RetireSealedRecord {
        request: medulla_mutation_types::sealed_record::RetireSealedRecordRequest,
    },

    // ── Web page ingest (MEDULLA-UNIFIED-DATA-PLANE-R651.7, MEDULLA-DECISION-ENGINE-R802) ──
    /// Chunk one held Document of the request graph, embed the chunks and
    /// commit the chunks, their `chunkOf` edges, their vectors and one
    /// enrichment record as one governed ChangeEnvelope. Idempotent by the
    /// Document's content digest; a new digest supersedes the earlier chunks.
    /// See [`crate::source_ingestion::web_document`].
    EnrichDocument {
        request: Box<medulla_source_types::source_ingestion::web_document::EnrichDocumentRequest>,
    },
    /// Choose which web search candidates to fetch: canonical-URL and digest
    /// dedupe against each other and the request graph's held Documents,
    /// held-fresh skips, deterministic relevance/rank/freshness scoring and a
    /// fetch budget. Reads only; no model runs. See
    /// [`crate::decision::web_fetch_selection`].
    SelectWebPages {
        request: Box<medulla_decision_outcome_types::decision::web_fetch_selection::SelectWebPagesRequest>,
    },

    // ── Branches (MEDULLA-BRANCH-MERGE-R001) ──
    /// Create one branch of `graph`: one branch registry row, no graph data
    /// copied. `tenant` must be the verified request tenant
    /// (`BRANCH_TENANT_DENIED`). `base_engine_version` defaults to the graph
    /// head and may not exceed it (`BRANCH_BASE_UNKNOWN`). The name matches
    /// `[a-z0-9][a-z0-9._/-]{0,127}` (`BRANCH_NAME_INVALID`); `main` and the
    /// `eg/` prefix are reserved (`BRANCH_NAME_RESERVED`); a name with a row
    /// refuses `BRANCH_EXISTS`. Every refusal writes nothing. See
    /// [`medulla_types::branch`].
    CreateBranch {
        tenant: String,
        graph: String,
        name: String,
        #[serde(default)]
        base_engine_version: Option<u64>,
    },

    /// Apply one mutation to a branch (MEDULLA-BRANCH-MERGE-R002). The mutation
    /// commits to the branch's own target graph, so `main` and sibling branches
    /// never see it, then one `BranchCommit` is appended to the branch's single
    /// commit chain and the head advances in the same registry transaction.
    /// `tenant` must be the verified request tenant (`BRANCH_TENANT_DENIED`).
    /// `BRANCH_UNKNOWN` and `BRANCH_DELETED` refuse before any graph change, as
    /// does a `mutation` that is not a graph mutation (`BRANCH_WRITE_INVALID`).
    /// A replay under the same idempotency key returns the original receipt.
    BranchWrite {
        tenant: String,
        graph: String,
        branch: String,
        mutation: Box<Method>,
    },

    /// Submit one source changeset to a branch (MEDULLA-SCM-R002). File bytes
    /// were uploaded first through `BlobBegin`/`BlobChunkPut`/`BlobCommit`; the
    /// request names them by digest. The changeset is the payload of the
    /// branch commit that carries it, appended with the head advance in one
    /// registry transaction. Refusals: `CHANGESET_EMPTY`, `CHANGESET_INVALID`,
    /// `CHANGESET_PARENT_NOT_ON_BRANCH`, `BLOB_NOT_FOUND` (and
    /// `BLOB_TOO_LARGE`, `BLOB_DIGEST_MISMATCH`), plus the branch refusals of
    /// `BranchWrite`; every refusal leaves the chain unchanged.
    SubmitChangeset {
        request: Box<medulla_types::source_changeset::SubmitChangesetRequest>,
    },

    // ── Context facts (MEDULLA-DECISION-ENGINE-R152) ──
    /// Cited facts of ranked nodes inside a token budget: for each node id in
    /// the caller's order, the visible literal properties and outgoing edges as
    /// facts that name their source and evidence class. Reads only, through the
    /// caller's graph view, so a hidden node yields no fact. See
    /// [`medulla_types::context_facts`].
    ContextFacts {
        request: medulla_types::context_facts::ContextFactsRequest,
    },
    // ── Source spans (MEDULLA-DECISION-ENGINE-R153) ──
    /// A bounded line range of one ingested text source, located through the
    /// table of contents `SourceIngest` stored for the document revision.
    /// `line_start` and `line_end` are 1-based and inclusive. Every answer names
    /// the table digest and the source digest. Reads only, through the caller's
    /// graph view. See [`medulla_types::source_toc`].
    ReadSpan {
        document_id: String,
        revision: u64,
        line_start: u64,
        line_end: u64,
    },
    // ── Graph change feed (MEDULLA-DURABLE-KERNEL-R043.100) ──
    /// One consistent snapshot digest of the request graph, with the cursor
    /// that continues after it (`high_water_cursor`). Reads only, through the
    /// caller's graph view. See [`medulla_types::graph_change_feed`].
    OpenGraphSnapshot,
    /// One page of committed graph changes after `cursor`, at most `limit`
    /// (1 to 256) scanned events. Delivery is at-least-once under stable ids.
    /// An expired, stale or revoked cursor is refused with a typed code.
    ReadGraphChanges {
        cursor: String,
        limit: u32,
    },
    // ── Agent bus roster (MEDULLA-DURABLE-KERNEL-R095.1002) ────────────────
    /// Register (replace) one session's participant row on the request
    /// graph's bus. `participant.last_seen_ms` is the request clock. The engine
    /// stamps `participant.tenant` and `principal` from the verified request
    /// context; caller values are overwritten. Needs `bus:write`.
    BusRegister {
        participant: medulla_types::agent_bus::BusParticipant,
        #[serde(default)]
        principal: String,
    },
    /// Set one registered session's `last_seen_ms` to `now_ms`. Refuses a
    /// session registered by another principal. Needs `bus:write`.
    BusHeartbeat {
        session_ref: String,
        now_ms: u64,
        #[serde(default)]
        principal: String,
    },
    /// The tenant's participants by `session_ref`, each marked live when
    /// `now_ms - last_seen_ms` is within `bus.participant_ttl_s`. `tenant`
    /// must equal the verified request tenant. Needs `bus:read`.
    BusRoster { tenant: String, now_ms: u64 },
    // ── Meeting transcripts (MEDULLA-MEDIA-VOCAB-R001.101.3 / .4 / .5) ──────
    /// Apply one transcript revision event (`append`, `correct`,
    /// `delete_segment`, `finalize`) to the request graph's meeting transcript
    /// and commit its `TranscriptSegment` nodes and revision node as one
    /// ChangeEnvelope. An exact replay answers the stored revision.
    MeetingTranscriptRevision {
        request: Box<medulla_types::meeting_transcript::MeetingTranscriptRevision>,
    },
    /// Cut a bounded summary context from the current finalized transcript and
    /// record one `SummaryContext` node keyed by its digest.
    MeetingSummaryContext {
        request: Box<medulla_types::meeting_transcript::MeetingSummaryContextRequest>,
    },
    /// Commit a produced summary: the receipt must match its stored context and
    /// every citation must lie inside a context span.
    MeetingSummaryCommit {
        request: Box<medulla_types::meeting_transcript::MeetingSummaryCommitRequest>,
    },

    // ── Document delete (MEDULLA-UNIFIED-DATA-PLANE-R651.3..R651.5) ─────────
    /// Delete one RLS-visible Document of the request graph and everything
    /// derived from it (chunks, extracted facts, enrichment records) as one
    /// atomic batch that also writes one content-free `DocumentDeletion`
    /// record. Refuses with `DOCUMENT_NOT_FOUND`, `DOCUMENT_HAS_REFERENCES` or
    /// `CASCADE_TOO_LARGE` and then changes nothing. See
    /// [`medulla_query_types::document_ops`].
    DocumentDelete {
        request: Box<medulla_query_types::document_ops::DocumentDeleteRequest>,
    },

    // ── Data-prep declarations (MEDULLA-INGEST-WORKER-R001.1007/.1008) ──────
    /// Replace one tenant's declaration of a data-prep capability: the
    /// declaration node and its `declares_field` edges commit as one
    /// ChangeEnvelope, an equal content digest writes nothing, and a payload
    /// outside the grammar refuses `DATA_PREP_DECLARATION_INVALID` or
    /// `DATA_PREP_DECLARATION_TOO_LARGE` and changes nothing. See
    /// [`medulla_types::data_prep_declaration`].
    DataPrepDeclare {
        request: Box<medulla_types::data_prep_declaration::DataPrepDeclareRequest>,
    },
    /// Read one tenant's stored declaration of a data-prep capability
    /// (absent when none was declared), under the caller's row authority.
    DataPrepDeclarationRead {
        request: Box<medulla_types::data_prep_declaration::DataPrepDeclarationReadRequest>,
    },

    // ── Memory snapshots (MEDULLA-DURABLE-KERNEL-R085.4) ────────────────────
    /// Read one document's head, or the retained `revision`, with its bytes.
    /// Needs `memory:read`. See [`medulla_types::memory_sync`].
    ReadMemorySnapshot {
        doc_id: String,
        revision: Option<u64>,
    },
    /// List document heads whose id starts with `prefix`, in `doc_id` order.
    /// `limit` defaults to 50 (maximum 200). Needs `memory:read`.
    ListMemorySnapshots {
        prefix: String,
        limit: Option<u32>,
        cursor: Option<String>,
    },
    /// Observe, submit, resolve or acknowledge one document. The engine fills
    /// `stamp` from the verified request context. Needs `memory:write`.
    SyncMemorySnapshot {
        request: Box<medulla_types::memory_sync::SyncRequest>,
        #[serde(default)]
        stamp: Option<medulla_types::memory_sync::SyncStamp>,
    },

    // ── Data products (MEDULLA-TYPED-PACKS-R805.1) ──────────────────────────
    /// Register one data product with its contract in the `rbac.redb` policy
    /// store, keyed by tenant and product id. `tenant` must equal the verified
    /// request tenant. See [`medulla_types::data_product`].
    DataProductRegister {
        tenant: String,
        product: medulla_types::data_product::DataProduct,
        contract: medulla_types::data_product::DataContract,
    },
    /// Open one access request against a registered product (state `pending`).
    AccessRequestOpen {
        tenant: String,
        request: medulla_types::data_product::AccessRequest,
    },
    /// Approve or reject the pending access request of `principal` for `product`.
    AccessRequestDecide {
        tenant: String,
        product: String,
        principal: String,
        approve: bool,
    },
    /// The served engine contract (MEDULLA-CONTRACT-R054): one bounded page of
    /// method descriptors in ascending `id` byte order plus, on the first page,
    /// the scope registry. Scope `contract:read` (user class). `cursor` is the
    /// `id` of the last descriptor of the previous page (at most 128
    /// characters); the page starts at the first `id` strictly greater. `limit`
    /// is 1 to 500, defaulting to 500. Refuses `DESCRIBE_CURSOR_INVALID` and
    /// `DESCRIBE_LIMIT_INVALID`.
    DescribeContract {
        #[serde(default)]
        cursor: Option<String>,
        #[serde(default)]
        limit: Option<u16>,
    },
        ]);
    };
}

pub(crate) use __eg_method_chunk_11;
