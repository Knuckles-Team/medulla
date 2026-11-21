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
    /// The composed answer to "How can I execute task XYZ?"
    /// (MEDULLA-CQ-ORCH-R008.2): one assembly, the capability coverage, the
    /// guardrails and the task's steps and workflow in one seven-part
    /// envelope. Reads tenant-bound snapshots; commits nothing.
    AgentExecutionAnswer {
        request: Box<medulla_decision_outcome_types::decision::execution_answer::AgentExecutionAnswerRequest>,
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
    /// Failure-pattern mining (MEDULLA-DECISION-ENGINE-R701.3): read the failed
    /// runs of a caller graph, cluster them by signature and file one gap per
    /// recurring pattern; mark a gap addressed; read one gap. The
    /// `FailureIngest` handler is the single writer of gap nodes.
    FailureIngest {
        op: Box<medulla_decision_types::failure_patterns::FailureOp>,
    },
    /// The per-tenant usage ledger (MEDULLA-DECISION-ENGINE-R801.8): append
    /// observed model calls or store a price catalog under the verified tenant.
    /// Both ops need the service-only scope `usage:write`.
    Usage {
        op: Box<medulla_decision_types::usage::UsageOp>,
    },
    /// The feature ledger: record a gap with the matches it came from, attach a
    /// plan, close it by a known change, and read entries
    /// (MEDULLA-DECISION-ENGINE-R707.3). The `FeatureLedger` handler is the
    /// single writer of ledger state.
    FeatureLedger {
        op: Box<medulla_decision_types::decision::feature_match::FeatureLedgerOp>,
    },
    /// Research claims: propose, advance along the lifecycle, record evidence
    /// on five channels, compile a sealed research artifact and read them back.
    /// The `Claims` handler is the single writer of claim state.
    Claims {
        op: Box<medulla_decision_types::claims::ClaimOp>,
    },
    /// Improvement loops: the fenced lease, gaps, the gap-to-work-offer market,
    /// stage beacons, the improvement ledger, the failure-sequence miner and
    /// the capability ratchet. The `Loops` handler is the single writer of
    /// loop state.
    Loops {
        op: Box<medulla_decision_types::loops::LoopOp>,
    },
    /// Belief revision: revise the confidence of claims whose evidence changed,
    /// track contradictions and propose retractions. The `BeliefRevise`
    /// handler is the single writer of belief revisions, friction findings and
    /// contradiction records.
    BeliefRevise {
        request: Box<medulla_decision_types::belief_revision::BeliefReviseRequest>,
    },
    /// Publish a rendered skill package when the run history `DecisionLog`
    /// `learn` recorded for its digest validates it (MEDULLA-DECISION-ENGINE-R160.3).
    /// The handler is the single writer of publication receipts.
    PublishSkillPackage {
        request: Box<medulla_decision_types::decision::package_validation::PublishSkillPackageRequest>,
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
    /// MEDULLA-CQ-ORCH-R010.3: the derived architecture view of one ingested
    /// repository (containers, `calls` edges, support paths) with the snapshot
    /// revision it was read at and its canonical digest. Reads only; refuses
    /// `VIEW_NO_SUPPORTING_SOURCE`, `VIEW_UNKNOWN_REPOSITORY`,
    /// `VIEW_STALE_REVISION` and `VIEW_TOO_LARGE`.
    ArchitectureView {
        scope: medulla_catalog_types::architecture_view::ViewScope,
    },
    /// Export the visible architecture nodes as one Open Exchange Format
    /// document (MEDULLA-CQ-ORCH-R705.6). `scope` names an optional graph
    /// within the caller's tenant. Reads only.
    ArchitectureModelExport {
        scope: Option<String>,
    },

    /// Compile one process of the caller's tenant view into a stored
    /// `WorkflowPattern` version (MEDULLA-CQ-ORCH-R700.3). The write is a
    /// compare-and-set on `expected_version`, the stored version of the process
    /// (0 when none); a mismatch refuses `WORKFLOW_VERSION_CONFLICT`. A
    /// published version is never mutated; an edit writes version `n + 1`.
    ProcessCompile {
        process_iri: String,
        expected_version: Option<u32>,
    },
    /// Store one canonical workflow as the next `WorkflowPattern` version of
    /// `workflow_id` (MEDULLA-CQ-ORCH-R701.3). `admit_canonical` checks the document;
    /// the write is the compare-and-set of `ProcessCompile` on `expected_version`.
    ProcessModelImport {
        workflow: medulla_types::process_import::CanonicalWorkflow,
        workflow_id: String,
        expected_version: Option<u32>,
    },
    /// Read version `version` of the stored workflow `workflow_id`
    /// (MEDULLA-CQ-ORCH-R701.3). With no `target` the canonical form is returned; with
    /// `bpmn20` the deployable BPMN document too; with `aris_epc` the canonical form with
    /// the EPC losses. `strict` refuses `PROCESS_EXPORT_LOSSY` instead of listing losses.
    /// Reads only.
    ProcessModelExport {
        workflow_id: String,
        version: u32,
        target: Option<medulla_types::process_import::ProcessSourceSystem>,
        #[serde(default)]
        strict: bool,
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
        request: Box<medulla_ingestion_types::ingest_readback::VerifyIngestReadbackRequest>,
    },
    /// EG-owned D18 source change sets and append-only write-back receipts.
    /// The engine records governed observations; it never calls a vendor API.
    WriteBack {
        op: Box<medulla_mutation_types::write_back::WriteBackOp>,
    },
    /// Known concept ids and the `superseded_by` lineage DAG between them
    /// (MEDULLA-DURABLE-KERNEL-R094.7): claim an id for a repository, add an
    /// edge, or read one concept with its direct successors. Refuses a
    /// collision, a malformed id, an unknown endpoint, a self edge and a cycle
    /// by registered code.
    ConceptLineage {
        op: Box<medulla_agent_types::concept_lineage::ConceptLineageOp>,
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
    /// MEDULLA-CQ-XSRC-R011: the supply-chain impact report for the named class roots (class
    /// IRIs or local names the graph's schema names). Sources come only from the graph's
    /// `vg:SourceAuthority` rows and the caller's own foreign catalog; an authority source the
    /// caller has not registered is a typed abstention in the answer. An unknown root refuses
    /// `CQ_CLASS_UNKNOWN`; a blank or empty `roots` refuses `PARAMETER_INVALID`; a caller with no
    /// verified tenant refuses `TENANT_DENIED`. Reads, commits nothing.
    SupplyChainImpact {
        roots: Vec<String>,
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
    /// Record one goal of a project (MEDULLA-SDLC-R017.1): a `Goal` node and a
    /// `targetsGoal` edge from each target row or competency question. Refuses
    /// `GOAL_TARGET_REQUIRED` when the goal names no target.
    GoalPut {
        goal: medulla_source_types::lifecycle::Goal,
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

    /// Export the changeset chain of a branch as a version-2 git pack
    /// (MEDULLA-SCM-R014): every changeset from each want back along first
    /// parents to, excluding, a have or the root, with the commit and tree ids
    /// of the import and the side pack objects of every merge. Wants and haves
    /// are git commit ids or branch commit ids. The caller needs read access on
    /// every path of the exported trees. Refusals: `PATH_READ_DENIED`,
    /// `REVISION_NOT_ON_CHAIN`, `GIT_GRAPH_MALFORMED`,
    /// `GIT_EXPORT_TREE_READ_FAILED`, plus the branch refusals of `ReadTree`.
    /// Reads only.
    GitExportPack {
        request: Box<medulla_source_types::source_changeset::GitExportPackRequest>,
    },
        ]);
    };
}

pub(crate) use __eg_method_chunk_10;
