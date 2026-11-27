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
    /// Human corrections (MEDULLA-DECISION-ENGINE-R702.3): record one correction
    /// with its rule candidate, independent outcome and eval case in one atomic
    /// graph batch, read the nearest prior corrections, or read one correction
    /// by receipt id. The `CorrectionRecord` handler is the single writer of
    /// correction state.
    CorrectionRecord {
        op: Box<medulla_decision_types::correction::CorrectionOp>,
    },
    /// Knowledge flags (MEDULLA-DECISION-ENGINE-R708.3): any agent raises a flag that a skill, a
    /// domain term or a data access source is missing or wrong; a curator resolves it. The
    /// `KnowledgeFlag` handler is the single writer of flag state.
    KnowledgeFlag {
        op: Box<medulla_decision_types::knowledge_flag::KnowledgeFlagOp>,
    },
    /// The knowledge curation cycle (MEDULLA-DECISION-ENGINE-R709.3): serve the curation inputs,
    /// check one proposal for contradiction and record it as an evaluated revision, close the
    /// flags of an adopted proposal and measure the closed loop.
    KnowledgeCuration {
        op: Box<medulla_decision_types::knowledge_curation::CurationOp>,
    },
    /// Friction reports (MEDULLA-DECISION-ENGINE-R710.3): agents file what they tried, expected
    /// and saw while using a skill; the engine groups the reports by skill and moves each group
    /// along the friction machine, every move naming its evidence.
    FrictionReport {
        op: Box<medulla_decision_types::friction::FrictionOp>,
    },
    /// Skill telemetry (MEDULLA-DECISION-ENGINE-R713.3): a service ingests skill invocations, and
    /// callers read the twelve measures of a skill, a quality rank, peer skills and the run traces
    /// of a skill by eval outcome. No invocation holds free text.
    SkillTelemetry {
        op: Box<medulla_decision_types::skill_telemetry::SkillTelemetryOp>,
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
        /// What the pattern needs to hold before it applies, stored on the workflow node
        /// (MEDULLA-ORCH-VOCAB-R800.3). At most 32 entries of at most 512 characters.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        preconditions: Vec<medulla_work_types::orchestration::definitions::PatternPrecondition>,
    },
    /// Store one canonical workflow as the next `WorkflowPattern` version of
    /// `workflow_id` (MEDULLA-CQ-ORCH-R701.3). `admit_canonical` checks the document;
    /// the write is the compare-and-set of `ProcessCompile` on `expected_version`.
    ProcessModelImport {
        workflow: medulla_types::process_import::CanonicalWorkflow,
        workflow_id: String,
        expected_version: Option<u32>,
        /// As on `ProcessCompile` (MEDULLA-ORCH-VOCAB-R800.3).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        preconditions: Vec<medulla_work_types::orchestration::definitions::PatternPrecondition>,
    },
    /// Import one BPMN 2.0 or DMN 1.3 document as the next version of the model `model_iri`
    /// (MEDULLA-CQ-ORCH-R702.9). The document is parsed, mapped onto element nodes and
    /// committed with the model node as one `BatchUpdate` under a compare-and-set on
    /// `expected_version`, the stored version of the model (0 when none); a mismatch refuses
    /// `WORKFLOW_VERSION_CONFLICT`. A document the reader refuses writes nothing.
    ProcessDocumentImport {
        document: medulla_types::process_exchange::ProcessDocument,
        model_iri: String,
        expected_version: Option<u32>,
    },
    /// Write one document from a stored model or workflow (MEDULLA-CQ-ORCH-R702.10): a model
    /// through `write_bpmn`, a stored workflow through `workflow_to_bpmn` and then
    /// `write_bpmn`. Reads only.
    ProcessDocumentExport {
        source: medulla_types::process_exchange::ExportSource,
        format: medulla_types::process_exchange::ProcessDocumentFormat,
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

    /// Queue one stored process model for deployment to a process source
    /// (MEDULLA-CQ-ORCH-R703.3). Gateway-routed like `SourceSync`: the model is exported
    /// through the document export of MEDULLA-CQ-ORCH-R702.10 and one outbound proposal,
    /// bound to the document digest, is queued through the write-back queue of
    /// MEDULLA-SRC-AUTH-R012.5. The engine never calls the source; the connector deploys the
    /// released proposal.
    ProcessModelDeploy {
        model_iri: String,
        connection_iri: String,
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
    /// List the request graph's schema sources and its composed digest. Read only, scope
    /// `owl:read` (MEDULLA-TYPED-PACKS-R813).
    GraphSchemaList,
    /// One schema source of the request graph with its stored shapes and ontology
    /// documents (MEDULLA-TYPED-PACKS-R813). Read only, scope `owl:read`; `GraphSchema`
    /// stays the admin write. A key the request graph does not carry refuses
    /// `SCHEMA_SOURCE_NOT_FOUND`.
    GraphSchemaGet {
        source_id: String,
    },
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
    /// MEDULLA-SUPPLY-VOCAB-R005.9: `contracts` are the `:CustomerContract` node ids the caller
    /// asks about; the report lists the services each covers through its `coversService` edges
    /// and names a contract with none as `ContractUncovered`. Empty reads no contract. A contract
    /// id that names no node the caller may read refuses `PARAMETER_INVALID`.
    SupplyChainImpact {
        roots: Vec<String>,
        #[serde(default)]
        contracts: Vec<String>,
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
    /// MEDULLA-TYPED-PACKS-R108.17: one catalog read, `Browse` (kind counts and a page of
    /// assets), `Search` (ranked text search) or `Detail` (one asset's owners, terms, lineage
    /// neighbours, ratings and comments). Every result is narrowed to what the caller may read.
    /// Refuses `ASSET_NOT_FOUND` (unknown or unreadable asset, one body), `INVALID_ARGUMENT`
    /// (unknown kind, bad cursor, page size out of bounds) and `CATALOG_QUERY_TOO_LONG`.
    CatalogRead {
        op: Box<medulla_catalog_types::catalog_read::CatalogReadOp>,
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
    /// Record one gate verdict of a changeset (MEDULLA-SCM-R008): a sealed, create-only
    /// `GateVerdict` node and one `judges` edge to `commit:<changeset>`, in one batch. Refuses
    /// `GATE_INPUT_DIGEST_MISMATCH` (the digest is not the changeset tree),
    /// `GATE_VERDICT_SEALED` (a different verdict is stored for the gate) and
    /// `LIFECYCLE_ENDPOINT_UNKNOWN` (the changeset node is absent). Identical replays succeed.
    RecordGateVerdict {
        changeset: String,
        /// The branch whose chain holds the changeset; `input_digest` is checked against the
        /// tree of that chain.
        branch: String,
        gate_id: String,
        input_digest: String,
        verdict: medulla_work_types::step_verdict::VerificationVerdict,
        evidence_digest: String,
        duration_ms: u64,
    },
    /// File one symptom against the defects of a repository (MEDULLA-SDLC-R003.3): an open
    /// defect with the same normalized signature takes it, a closed one reopens, otherwise a
    /// `Defect` node opens. Each call carries the intake path, the source record id and the time.
    FileDefect {
        request: medulla_source_types::lifecycle::FileDefectRequest,
    },
    /// Move one defect to another status (MEDULLA-SDLC-R003.4). The move must be an edge of the
    /// defect machine (`DEFECT_TRANSITION_ILLEGAL`), and a move to `verified` needs a passing
    /// verdict of the bound test on the fix changeset (`DEFECT_UNVERIFIED`). A refusal writes
    /// nothing.
    TransitionDefect {
        request: medulla_source_types::lifecycle::TransitionDefectRequest,
    },
    /// Read one defect with its symptom list (MEDULLA-SDLC-R003.5). Reads only.
    GetDefect {
        request: medulla_source_types::lifecycle::GetDefectRequest,
    },
    /// Set the landing policy of one branch (MEDULLA-SCM-R008): the gates a landing
    /// needs. Lowering the stored policy needs an operator approval
    /// (`LANDING_POLICY_APPROVAL_REQUIRED`).
    LandingPolicyPut {
        request: medulla_source_types::gate_verdict::LandingPolicyPutRequest,
    },
    /// Record one rollout of an image: a `Deployment` node linked to the release
    /// whose artifact digests name the image, or `unattributed` with no release.
    RecordDeployment {
        deployment: medulla_source_types::lifecycle::DeploymentInput,
    },
    /// Record one applied deployment plan (MEDULLA-SDLC-R002.7): a sealed `DeployRecord` node
    /// keyed by the verified tenant and the plan digest, carrying the target, the outcome of every
    /// node, the image digests, the verified actor and the engine-clock time. The same body again
    /// replays the stored record and writes nothing; a different body for the same plan digest
    /// refuses `DEPLOY_RECORD_SEALED`; a field out of bounds refuses `DEPLOY_RECORD_INVALID`.
    DeployRecordPut {
        record: medulla_source_types::deploy_record::DeployRecordInput,
    },
    /// Read the deploy record of one plan digest of the verified tenant (MEDULLA-SDLC-R002.8). A
    /// missing, foreign or invisible record refuses the same `DEPLOY_RECORD_NOT_FOUND`. Reads only.
    DeployRecordGet { plan_digest: String },
    /// One cursor-paged, newest-first page of the verified tenant's deploy records
    /// (MEDULLA-SDLC-R002.8). `limit` is 1..=100 (100 when absent); the cursor is opaque and bound
    /// to the tenant it was minted for. Reads only.
    DeployRecordList {
        #[serde(default)]
        limit: Option<u32>,
        #[serde(default)]
        cursor: Option<String>,
    },
    /// Ask which approval gate applies to an operation (MEDULLA-BUSINESS-OPS-R004): `none`,
    /// `single`, `two_person`, or `unresolved` when no `ApprovalRule` of the request graph covers
    /// it, with the gate class, approver roles and expiry. The graph is the only authority.
    /// Reads only.
    ApprovalGateResolve {
        request: medulla_source_types::approval_gate::ApprovalGateRequest,
    },
    /// One cursor-paged, newest-first page of the verified tenant's agent goals
    /// (MEDULLA-DECISION-ENGINE-R901). `limit` is 1..=100 (100 when absent); the cursor is opaque
    /// and bound to the tenant it was minted for. Reads only.
    GoalList {
        #[serde(default)]
        limit: Option<u32>,
        #[serde(default)]
        cursor: Option<String>,
    },
    /// The iterations of one agent goal of the verified tenant, oldest first
    /// (MEDULLA-DECISION-ENGINE-R901). A missing, foreign or invisible goal refuses the same
    /// `GOAL_NOT_FOUND`. Reads only.
    GoalIterations { goal_id: String },
    /// Create, run, finish or cancel one agent goal (MEDULLA-DECISION-ENGINE-R901). The goal
    /// moves only along the edges of the goal state machine; a refused event is
    /// `TRANSITION_REFUSED` and writes nothing. A run opens a numbered iteration with a derived
    /// run id. Every time is the engine clock.
    GoalControl {
        op: medulla_source_types::agent_goal::GoalControlOp,
    },
    /// One cursor-paged, newest-first page of the verified tenant's connector source-sync runs,
    /// optionally of one connection and one state (MEDULLA-UNIFIED-DATA-PLANE-R790). A connection
    /// no run of the tenant names refuses `CONNECTION_NOT_FOUND`. Reads only.
    ConnectorRuns {
        #[serde(default)]
        connection_id: Option<String>,
        #[serde(default)]
        state: Option<medulla_source_types::connector_run::RunState>,
        #[serde(default)]
        limit: Option<u32>,
        #[serde(default)]
        cursor: Option<String>,
    },
    /// Report one connector run as it starts or ends (MEDULLA-UNIFIED-DATA-PLANE-R790): the
    /// engine stamps its start and end with its own clock; a finished run is sealed.
    ConnectorRunRecord {
        run: medulla_source_types::connector_run::ConnectorRunInput,
    },
    /// Read one connector run of the verified tenant (MEDULLA-UNIFIED-DATA-PLANE-R790). An
    /// absent, foreign or invisible run refuses the same `CONNECTOR_RUN_NOT_FOUND`. Reads only.
    ConnectorRunGet { run_id: String },
    /// Read one source connection of the verified tenant (MEDULLA-UNIFIED-DATA-PLANE-R791). An
    /// absent, foreign or invisible connection refuses the same `CONNECTION_NOT_FOUND`. Reads
    /// only.
    SourceConnectionGet { connection_id: String },
    /// One cursor-paged page of the verified tenant's source connections, by connection id
    /// (MEDULLA-UNIFIED-DATA-PLANE-R791). `limit` is 1..=100 (100 when absent). Reads only.
    SourceConnectionList {
        #[serde(default)]
        limit: Option<u32>,
        #[serde(default)]
        cursor: Option<String>,
    },
    /// Put or delete one source connection (MEDULLA-UNIFIED-DATA-PLANE-R791). The credential is
    /// a locator (`env://` or `openbao://`), never a value.
    SourceConnectionControl {
        op: medulla_source_types::source_connection::SourceConnectionOp,
    },
    /// Read the mapping of one source connection (MEDULLA-UNIFIED-DATA-PLANE-R791): the mapping,
    /// its digest and whether it is proposed or approved. Reads only.
    SourceMappingGet { connection_id: String },
    /// Propose a mapping for a source connection, or approve the stored one at the digest the
    /// approver reviewed (MEDULLA-UNIFIED-DATA-PLANE-R791). A stale digest refuses
    /// `MAPPING_CHANGED`.
    SourceMappingControl {
        op: medulla_source_types::source_connection::SourceMappingOp,
    },
    /// Record one goal of a project (MEDULLA-SDLC-R017.1): a `Goal` node and a
    /// `targetsGoal` edge from each target row or competency question. Refuses
    /// `GOAL_TARGET_REQUIRED` when the goal names no target.
    GoalPut {
        goal: medulla_source_types::lifecycle::Goal,
    },
    /// Record one proposal of the learning loop (MEDULLA-SDLC-R017.3): a
    /// `Proposal` node and an `evidences` edge from each cited record. Refuses
    /// `PROPOSAL_EVIDENCE_REQUIRED`, `PROPOSAL_EVIDENCE_UNRESOLVED` and
    /// `PROPOSAL_EVIDENCE_DIGEST_MISMATCH`.
    ProposalPut {
        proposal: medulla_source_types::lifecycle::Proposal,
    },
    /// List the proposals of one project for review (MEDULLA-SDLC-R017.6),
    /// optionally narrowed to one state and one kind. Reads only.
    ProposalList {
        request: medulla_source_types::lifecycle::ProposalListRequest,
    },
    /// Answer one competency question over the spec artifact graph of `graph`
    /// (MEDULLA-SDLC-R014.6). `subject` is the phase for `next_unblocked_task`, the goal node
    /// id for `changes_advancing_goal` and the changeset id for `principles_at_risk`; a subject
    /// given where none is taken, or missing where required, refuses `PARAMETER_INVALID`, and
    /// an unknown goal or phase refuses `COMPETENCY_SUBJECT_UNKNOWN`. Reads only.
    SpecQuestion {
        question: medulla_source_types::spec_question::SpecQuestion,
        #[serde(default)]
        subject: Option<String>,
        graph: String,
    },
    /// Walk the derived lifecycle edges from one anchor record (MEDULLA-SDLC-R006.3): breadth
    /// first to `depth`, filtered by `kinds`, bounded by the `lifecycle.trace_max_depth` and
    /// `lifecycle.trace_node_budget` knobs of the request's policy. A depth outside 1 and the
    /// ceiling refuses `LIFECYCLE_DEPTH_INVALID`; an anchor the caller cannot see answers no
    /// records. Reads only.
    LifecycleTrace {
        request: medulla_source_types::lifecycle::LifecycleTraceRequest,
    },
    /// Open one delivery for a dispatched slice (MEDULLA-SDLC-R019.3), or record the
    /// `would_dispatch` of an unprompted slice in `development` mode (MEDULLA-SDLC-R019.4).
    /// Refuses `DELIVERY_SCOPE_EMPTY` and `MODE_REFERENCE_NO_PROPOSALS`.
    DeliveryOpen {
        request: medulla_source_types::delivery::DeliveryOpenRequest,
    },
    /// Route one source-control feedback event to the deliveries that follow its proposal
    /// (MEDULLA-SDLC-R019.5). Refuses `FEEDBACK_SOURCE_UNKNOWN`, `FEEDBACK_PROPOSAL_UNKNOWN`
    /// and `FEEDBACK_EVENT_INVALID`.
    DeliveryFeedback {
        event: medulla_source_types::delivery::ScmFeedbackEvent,
    },
    /// Apply one agent report to a delivery (MEDULLA-SDLC-R019.7). Refuses
    /// `ASSIGNMENT_UNKNOWN`, `ASSIGNMENT_ENDED`, `REPORT_EVIDENCE_MISSING`, `QUESTION_TOO_LONG`.
    DeliveryReport {
        write: medulla_source_types::delivery::DeliveryReportWrite,
    },
    /// Read one delivery with its derived stage. Another tenant's delivery refuses
    /// `DELIVERY_NOT_FOUND`. Reads only.
    DeliveryStatus {
        request: medulla_source_types::delivery::DeliveryRef,
    },
    /// List the deliveries of the tenant with their derived stage. Reads only.
    DeliveryList {
        request: medulla_source_types::delivery::DeliveryListRequest,
    },
    /// Stop one delivery: it ends `stopped` and receives no further feedback.
    DeliveryStop {
        request: medulla_source_types::delivery::DeliveryRef,
    },
    /// Open a new pending assignment on a live delivery whose assignments all ended.
    DeliveryRetry {
        request: medulla_source_types::delivery::DeliveryRetryRequest,
    },
    /// Triage one defect of the tenant graph (MEDULLA-SDLC-R004.3): the engine reads the defect,
    /// its stack frames, their symbols and blame, decides through the `defect_triage` question
    /// family, commits the sealed record through `DecisionLog` and writes the `affects` edges.
    /// An unknown defect refuses `DEFECT_UNKNOWN` and a closed one `DEFECT_CLOSED`.
    TriageDefect {
        request: Box<medulla_decision_types::decision::defect_triage::TriageDefectRequest>,
    },
    /// List the quarantined schema drifts of the caller's tenant, optionally of one stream
    /// (MEDULLA-TYPED-PACKS-R806). Reads only.
    SchemaDriftList {
        request: medulla_source_types::schema_drift_methods::SchemaDriftListRequest,
    },
    /// Read one quarantined schema drift of the caller's tenant with its report per stream.
    /// An unknown id and another tenant's drift refuse `DRIFT_NOT_FOUND`. Reads only.
    SchemaDriftGet {
        request: medulla_source_types::schema_drift_methods::SchemaDriftRef,
    },
    /// Derive the repair candidate of one drifted stream and verify it on a shadow check of
    /// the request graph. Applies nothing; answers the candidate and its digest.
    SchemaRepairPropose {
        request: medulla_source_types::schema_drift_methods::SchemaRepairTarget,
    },
    /// Activate the repair candidate of one drifted stream. Needs the approval receipt bound
    /// to the candidate digest; refuses `REPAIR_APPROVAL_MISSING` and
    /// `REPAIR_DIGEST_MISMATCH`. The activation commits through the graph schema gateway.
    SchemaRepairApprove {
        request: medulla_source_types::schema_drift_methods::SchemaRepairApproveRequest,
    },
    /// Read the derived state of every requirement row of `repo` at the changeset `rev`
    /// (MEDULLA-SDLC-R005.3), from the `:Requirement`, changeset and test-binding records
    /// the caller can see in `graph`. `spec_id` keeps only the rows of that spec. A `rev` the
    /// repository chain does not hold refuses `SPEC_STATUS_REV_UNKNOWN`; a repository with no
    /// ingested spec tree refuses `SPEC_TREE_ABSENT`. Reads only.
    SpecStatus {
        repo: String,
        rev: String,
        #[serde(default)]
        spec_id: Option<String>,
        graph: String,
    },
    /// Record a reviewer's approval or rejection of one open proposal
    /// (MEDULLA-SDLC-R017.6). Refuses `PROPOSAL_NOT_FOUND` and `PROPOSAL_NOT_OPEN`.
    ProposalDecide {
        request: medulla_source_types::lifecycle::ProposalDecideRequest,
    },
    /// Set the mode of one project (MEDULLA-SDLC-R012.8): `reference`,
    /// `development` or `evolving`. A move to `evolving` needs an operator
    /// approval (`MODE_APPROVAL_REQUIRED`).
    ProjectModeSet {
        request: medulla_source_types::lifecycle::ProjectModeSet,
    },
    /// Read the mode record of one project (MEDULLA-SDLC-R012.9): the typed
    /// counterpart of `ProjectModeSet`. A project with no visible record of the
    /// verified tenant answers `reference` with `recorded: false`. Reads only.
    ProjectModeGet {
        request: medulla_source_types::lifecycle::ProjectModeGetRequest,
    },
    /// Read the configuration layers of the verified caller (MEDULLA-IDENTITY-R740): the
    /// company layer, the layer of every team the caller belongs to and the caller's own user
    /// layer, each a `ConfigLayer` node of the verified tenant the caller may see. The request
    /// names nothing; a layer that is absent, foreign or hidden is left out. Reads only.
    ConfigLayers {
        request: medulla_source_types::config_layers::ConfigLayersRequest,
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

    /// List the refs of a repository (MEDULLA-SCM-R851): `refs/heads/<branch>` and the git
    /// commit id of the head, for every live branch of the verified tenant that has a head
    /// changeset, in branch-name order. The caller needs read access on every path of every
    /// listed branch chain, the check `GitExportPack` makes; one denied path refuses the whole
    /// request `PATH_READ_DENIED`. A repository with no branch answers an empty list, so an
    /// unknown repository and an empty one read alike. Reads only.
    GitRefsList {
        request: Box<medulla_source_types::source_changeset::GitRefsListRequest>,
    },
        ]);
    };
}

pub(crate) use __eg_method_chunk_10;
