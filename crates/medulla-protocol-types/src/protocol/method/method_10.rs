macro_rules! __eg_method_chunk_10 {
    (@acc [$($variants:tt)*]) => {
        __eg_method_chunk_11!(@acc [
$($variants)*


    /// Community detection as a mining family (CONCEPT:MEDULLA-KG.mining.community-writeback):
    /// wraps the EXISTING GDS Louvain / label-propagation kernels
    /// (`medulla_graph_algos::graph_algos`, already exposed on the Cypher `CALL gds.*`
    /// surface) — adds NO new algorithm, only the epistemic writeback. Runs over
    /// the resident graph (optionally restricted to one node `label`, like
    /// `MineSubgraph`). With `writeback=true` materializes each community as a
    /// typed `:Community` node linked to its members — a graph MUTATION,
    /// WAL-replayed by re-detecting deterministically. Gated `mining`.
    #[cfg(feature = "mining")]
    MineCommunity {
        /// Optional: restrict the projected graph to nodes of this one type.
        #[serde(default)]
        label: Option<String>,
        /// Which existing GDS kernel to run.
        #[serde(default)]
        algorithm: CommunityAlgorithm,
        /// Louvain modularity resolution (ignored by label-propagation).
        #[serde(default = "default_resolution")]
        resolution: f64,
        /// Iteration/sweep cap.
        #[serde(default = "default_max_iter")]
        max_iterations: usize,
        /// Seed for Louvain's deterministic shuffle (ignored by label-propagation).
        #[serde(default)]
        seed: u64,
        /// Weight neighbor votes by edge weight (label-propagation only; ignored by Louvain).
        #[serde(default = "default_true")]
        weighted: bool,
        /// Materialize each community as a typed `:Community` node linked to its members.
        #[serde(default)]
        writeback: bool,
        /// ADDITIONALLY materialize a `:Claim` (+ `:Evidence`) per community (E6)
        /// — see [`Method::MineAssociate::as_claim`]. Confidence is seeded from
        /// the community's own internal-edge density (already `[0,1]`). Requires `writeback`.
        #[cfg(all(feature = "mining", feature = "epistemic"))]
        #[serde(default)]
        as_claim: bool,
    },

    // ── Decide layer (RF-ADR-010) ──────────────────────────────────────────
    /// Typed task/capability requirements -> a validated `AgentGraph` draft
    /// with coverage derivations and a verifiable certificate, or a typed
    /// abstention. Reads one tenant-bound agent-library snapshot; commits
    /// nothing. Boxed like `SemanticIndex`/`KgDelegate` so one request cannot
    /// set the size of every `Method`.
    AgentAssemble {
        request: Box<medulla_decision_types::decision::AssemblyRequest>,
    },
    /// Read-only capability coverage (MEDULLA-DECISION-ENGINE-R126.2.1): for each
    /// capability in the closure of native task/capability IRIs, the visible
    /// Agent Library components that cover it (with derivation premises and
    /// evidence class) and the registered A2A agent cards that self-declare it
    /// (always claims). Reads one tenant-bound agent-library snapshot; ranks
    /// nothing, commits nothing.
    CapabilityCoverage {
        request: Box<medulla_decision_outcome_types::decision::coverage::CapabilityCoverageRequest>,
    },
    /// The verified tenant's revision challengers, paged in creation order
    /// (MEDULLA-DECISION-ENGINE-R817). Reads the revision rows of the governed
    /// pointer store; writes nothing.
    RevisionProposalList {
        request: Box<medulla_decision_outcome_types::decision::revision_list::RevisionProposalListRequest>,
    },
    /// Advisory guardrail entailment over the verified tenant's published
    /// Agent Library rules (MEDULLA-DECISION-ENGINE-R127.2.2). Returns subclass
    /// paths and source generations; commits nothing and grants no authority.
    GuardrailEntailment {
        request: Box<medulla_agent_types::agent_guardrail::GuardrailEntailmentRequest>,
    },
    /// Snapshot-checked commit of one assembly `DecisionRecord` as a
    /// `DecisionRecord` component. Re-derives the record and compares the
    /// catalog digest before writing, so a stale decision is refused rather
    /// than stored.
    DecisionCommit {
        request: Box<medulla_decision_types::decision::DecisionCommitRequest>,
    },
    /// Statistical decision over library or RLS-filtered graph candidates.
    /// EVALUATE-ONLY: it answers records and commits none of them.
    Decide {
        request: Box<medulla_decision_types::decision::DecideRequest>,
    },
    /// Admin job: fit a decision head into a Blob CAS draft artifact.
    DecisionFit {
        op: Box<medulla_decision_types::decision::DecisionFitOp>,
    },
    /// Admin job: off-policy and calibration evaluation producing the
    /// promotion receipt a head must carry before it may be published.
    DecisionEval {
        op: Box<medulla_decision_types::decision::DecisionEvalOp>,
    },
    /// The statistical decision log: commit an acted-on record after
    /// verify-replay, join independent outcome evaluations, and read the
    /// outcome aggregate the log answers.
    DecisionLog {
        op: Box<medulla_decision_types::decision::statistical::log::DecisionLogOp>,
    },
    /// Research claims: propose, advance along the lifecycle, record evidence
    /// on five channels, compile a sealed research artifact and read them back.
    /// The `Claims` handler is the single writer of claim state.
    Claims {
        op: Box<medulla_types::claims::ClaimOp>,
    },
    /// Improvement loops: the fenced lease, gaps, the gap-to-work-offer market,
    /// stage beacons, the improvement ledger, the failure-sequence miner and
    /// the capability ratchet. The `Loops` handler is the single writer of
    /// loop state.
    Loops {
        op: Box<medulla_types::loops::LoopOp>,
    },
    /// Belief revision: revise the confidence of claims whose evidence changed,
    /// track contradictions and propose retractions. The `BeliefRevise`
    /// handler is the single writer of belief revisions, friction findings and
    /// contradiction records.
    BeliefRevise {
        request: Box<medulla_types::belief_revision::BeliefReviseRequest>,
    },
    /// General bounded 0-1 integer programme with an independently verifiable
    /// certificate. Pure compute: no store, no clock, no float.
    Solve {
        request: Box<medulla_decision_types::solve::SolveRequest>,
    },

    // ── Connector MCP pack import (RF-ADR-009) ─────────────────────────────
    /// Atomic connector MCP pack import and its admin surface.
    ConnectorPack {
        op: Box<medulla_catalog_types::connector_pack::ConnectorPackOp>,
    },

    /// Workflow registry (MEDULLA-DECISION-ENGINE-R166.3): `Register` stores one
    /// registration as a tenant graph node through the mutation gateway; `Read`
    /// returns one registration by exact identity. Not a `Decide` stage.
    WorkflowRegistry {
        op: Box<medulla_work_types::orchestration::workflows::WorkflowRegistryOp>,
    },

    /// Import one ArchiMate Open Exchange Format document (MEDULLA-CQ-ORCH-R705.6).
    /// `document` is the XML text. The import maps it onto the graph and commits
    /// the nodes and edges as one batch. `expected_version`, when given, must
    /// equal the exchange schema version the document carries.
    ArchitectureModelImport {
        document: String,
        expected_version: Option<u32>,
    },
    /// Export the visible architecture nodes as one Open Exchange Format
    /// document (MEDULLA-CQ-ORCH-R705.6). `scope` names an optional graph
    /// within the caller's tenant. Reads only.
    ArchitectureModelExport {
        scope: Option<String>,
    },

    /// Keep one federated secondary in step with the native graph, the system
    /// of record (MEDULLA-SRC-AUTH-R012.6). The host owns the connector call:
    /// it opened, filled and completed the foreign read `foreign_execution`
    /// with `plan_digest = sha256(connection_iri 0x00 class_iri)` first, and
    /// the engine never calls out. `scope` names the adapter's scope (for an
    /// architecture class, the model identifier). Inbound changes and the sync
    /// state commit as one batch; outbound changes come back as drafts the
    /// router queues as write-back proposals.
    SourceSync {
        connection_iri: String,
        class_iri: String,
        foreign_execution: medulla_catalog_types::foreign_read::ForeignReadExecutionRef,
        scope: Option<String>,
    },

    /// RF-ADR-009 native ingestion authority. Resolves the exact Connector
    /// Manifest mapping under the verified tenant, admits raw records to CAS,
    /// maps them and atomically commits graph material, provenance and cursor.
    SourceIngest {
        request: Box<medulla_source_types::source_ingestion::SourceIngestionRequest>,
    },

    /// Latest authoritative SourceIngest checkpoint for restart/failover CAS
    /// recovery. Tenant and graph are taken only from verified request context.
    SourceIngestStatus {
        connector: medulla_types::contract::ResourceId,
        stream: medulla_types::contract::ResourceId,
    },
    /// MEDULLA-REPO-INGEST-R006.1: the daily paper target measured over the
    /// corpus's `:StageReceipt` nodes. A paper counts once when all four
    /// stages completed inside `[window_end_ms - 86400 s, window_end_ms)`;
    /// `state` is `Unknown` for a partial window or telemetry older than
    /// `knob ingest.window_fresh_for`. Read-only: commits nothing.
    MeasureIngestThroughput {
        corpus: medulla_types::contract::ResourceId,
        window_end_ms: u64,
    },
    /// Independent readback of one ingest write (MEDULLA-REPO-INGEST-R002.100).
    /// Reads the receipt's result ids under the caller principal, compares id,
    /// content digest and revision, and commits one `:ReadbackReceipt` node when
    /// every id matches. Refuses `READBACK_DENIED`, `READBACK_PARTIAL`,
    /// `READBACK_STALE` or `READBACK_MISMATCH` and then changes nothing.
    VerifyIngestReadback {
        request: Box<medulla_types::ingest_readback::VerifyIngestReadbackRequest>,
    },
    /// EG-owned D18 source change sets and append-only write-back receipts.
    /// The engine records governed observations; it never calls a vendor API.
    WriteBack {
        op: Box<medulla_mutation_types::write_back::WriteBackOp>,
    },

    // ── Graph schema sources and the mutation outbox (X9, X10) ─────────────
    /// Attach, replace or detach one keyed schema source on the request graph.
    GraphSchema {
        op: Box<medulla_catalog_types::graph_schema::GraphSchemaOp>,
    },
    /// List the request graph's schema sources and its composed digest.
    GraphSchemaList,
    /// One page of the classes and properties the request graph's composed
    /// schema declares, ordered by `(iri, kind)` (EH-389). `kind` keeps one
    /// term kind; `cursor` is the previous page's `next_cursor`; `limit` is
    /// 1..=`MAX_GRAPH_SCHEMA_TERMS_PAGE`.
    GraphSchemaClasses {
        #[serde(default)]
        kind: Option<medulla_catalog_types::graph_schema::GraphSchemaTermKind>,
        #[serde(default)]
        cursor: Option<String>,
        limit: u32,
    },
    /// Operator view and in-order re-delivery of one owner's mutation outbox.
    MutationOutbox {
        op: Box<medulla_mutation_types::mutation_outbox::MutationOutboxOp>,
    },

    // ── User-managed edge-native indexes (EH-351 / EH-352) ─────────────────
    /// Create, refresh, drop or list the edge indexes of the request graph.
    EdgeIndex {
        op: Box<medulla_index_types::managed_index::EdgeIndexOp>,
    },
    /// Search one edge index of the request graph; edges come back as edges.
    EdgeSearch {
        request: Box<medulla_index_types::managed_index::EdgeSearchRequest>,
    },
    /// MEDULLA-UNIFIED-DATA-PLANE-R763.2: derive the column edges of one SQL
    /// query against a supplied catalog on the shared parser. Pure compute:
    /// no graph row is read and nothing is committed.
    ColumnLineage {
        op: Box<medulla_catalog_types::lineage::ColumnLineageOp>,
    },
    /// MEDULLA-CQ-XSRC-R008.4: answer one cross-source competency question
    /// (`exposed_services`, `host_limit_capabilities`, `contract_incidents`).
    /// Sources come only from the graph's `vg:SourceAuthority` rows. An unknown
    /// question name refuses `CQ_XSRC_UNKNOWN_QUESTION`. Reads, commits nothing.
    CrossSourceQuestion {
        question: String,
    },
    /// MEDULLA-UNIFIED-DATA-PLANE-R764.8: the frequent users of one asset and the joins and
    /// filters common on it, from usage the engine recorded itself. Read only. Refuses
    /// `CATALOG_ASSET_NOT_FOUND` (also for an asset the caller cannot read) and
    /// `CATALOG_LIMIT_INVALID`.
    CatalogUsage {
        asset: String,
        #[serde(default)]
        top_n: Option<u32>,
    },
    // ── Software lifecycle records (MEDULLA-SDLC-R002.3) ────────────────────
    /// Record one published release of a repository: a sealed `Release` node
    /// and a `contains` edge to each changeset after the repository's previous
    /// release. Refuses `RELEASE_HEAD_UNKNOWN` (the head is not on the
    /// repository chain) and `RELEASE_VERSION_TAKEN` (the repository and
    /// version name a different stored release). Identical replays succeed.
    RecordRelease {
        release: medulla_source_types::lifecycle::Release,
    },
    /// Record one rollout of an image: a `Deployment` node linked to the release
    /// whose artifact digests name the image, or `unattributed` with no release.
    RecordDeployment {
        deployment: medulla_source_types::lifecycle::DeploymentInput,
    },
    /// MEDULLA-SRC-AUTH-R004.6.1: which connected source is authoritative for each
    /// class (and facet) within the hop budget, with the premises behind every verdict
    /// and the authority audit built from them. A read; nothing is committed. Refuses
    /// `SOURCE_COVERAGE_NO_CLASSES`, `SOURCE_COVERAGE_TOO_MANY_CLASSES` and
    /// `SOURCE_COVERAGE_HOP_BUDGET_INVALID`.
    SourceCoverage {
        classes: Vec<String>,
        #[serde(default)]
        facets: Vec<medulla_identity_types::source_coverage::SourceCoverageFacet>,
        #[serde(default)]
        hop_budget: Option<u32>,
        #[serde(default)]
        needs_current_state: bool,
    },
        ]);
    };
}

pub(crate) use __eg_method_chunk_10;
