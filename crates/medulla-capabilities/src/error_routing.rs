//! One runtime and generated-contract error-set derivation per method.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use medulla_catalog_types::connector_pack::result::PackWriteErrorCode;
use medulla_catalog_types::graph_schema::GraphSchemaErrorCode;
use medulla_decision_types::decision::harness_contrast::HarnessContrastErrorCode;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::DecisionErrorCode;
use medulla_decision_types::solve::SolveErrorCode;
use medulla_types::contract::{EngineErrorCode, ServerErrorCode, UqlCode};
use medulla_work_types::orchestration::HarnessVocabularyErrorCode;

use crate::MethodDescriptor;

pub(crate) fn typed_codes<T: Copy>(
    variants: &[T],
    code: impl Fn(T) -> &'static str,
) -> Vec<&'static str> {
    variants.iter().copied().map(code).collect()
}

/// Shared refusals apply before the request reaches its method handler.
const SHARED_ENGINE_ERRORS: &[&str] = &[
    // The request boundary refuses any request naming an engine-internal method.
    "ENGINE_INTERNAL_METHOD",
    "INVALID_ARGUMENT",
    "ACCESS_DENIED",
    "AUTH_TENANT_MISMATCH",
    "AUTH_AUDIENCE_MISMATCH",
    "AUTH_POLICY_VERSION_MISMATCH",
    "AUTH_EXPIRED",
    "AUTH_BAD_SIGNATURE",
    "AUTH_UNKNOWN_PRINCIPAL",
    "NODE_MISMATCH",
    "CAPACITY_DENIED",
    "ENGINE_UNAVAILABLE",
    "ENGINE_RESOURCE_EXHAUSTED",
    "ENGINE_DEADLINE_EXCEEDED",
    "OPERATION_REDIRECTED",
];
const SHARED_SERVER_ERRORS: &[&str] = &[
    // Any graph read can meet a partly materialized graph (MEDULLA-OBSERVE-R001.1).
    "PARTIAL_MATERIALIZATION",
    "AUTHENTICATION_REQUIRED",
    "BUSY",
    "CANCELLED",
    "INTERNAL",
    "METHOD_NOT_YET_SERVED",
    "TIMEOUT",
    "STALE_ROUTE",
    "STALE_OUTBOX_LEASE",
];

/// A method-specific family wins over the wider domain family.
const METHOD_PREFIXES: &[(&str, &[&str])] = &[
    ("ArgumentMap", &["AIF_"]),
    ("LiveArtifact", &["LIVE_ARTIFACT_"]),
    (
        "Source",
        &[
            "SOURCE_",
            "CONNECTOR_SCHEMA_",
            "STALE_CONTENT_VERSION",
            "STALE_MAPPING_",
            "UNKNOWN_MAPPING_",
            "UNKNOWN_RELATIONSHIP_",
        ],
    ),
    (
        "ConnectorPack",
        &[
            "PACK_",
            // A pack head whose schema source did not advance past the
            // attached one (reproject of an unchanged head).
            "SCHEMA_SOURCE_REGRESSION",
            "CONNECTOR_PACK_",
            "CONNECTOR_RELATIONSHIP_",
            "ARCHIVE_",
            "MALFORMED_",
            "SEED_",
            "SOURCE_APPROVAL_",
            "SOURCE_CONTRACT_",
            "SOURCE_REGISTRATION_",
            "IMPORTER_",
            "INVALID_IMPORTER",
            "INVALID_RETIREMENT",
            "INVALID_FACTS",
            "UNKNOWN_PACK_ENTRY",
            "CORRUPT_CONNECTOR_",
            "COMPONENT_BODY_",
            "SEED_",
            "SOURCE_APPROVAL_",
            "SOURCE_BINDING_",
        ],
    ),
    (
        "GraphSchema",
        &[
            "ATTACH_PACK_",
            // MEDULLA-TYPED-PACKS-R651.4: the ontology edit chain's refusals.
            "EDIT_",
            "NAMING_VIOLATION",
            "PACK_PROJECTION_",
            "OWL_",
            "REASONING_",
            "STALE_RECOMPUTE_",
        ],
    ),
    (
        "Decision",
        &["DECISION_", "COMPONENT_", "CORRUPT_DECISION_"],
    ),
    ("Decide", &["DECISION_", "COMPONENT_", "CORRUPT_DECISION_"]),
    // MEDULLA-DECISION-ENGINE-R652.4: the claim surface's refusal codes.
    ("Claims", &["ARTIFACT_", "CLAIM_", "EVIDENCE_"]),
    // MEDULLA-DECISION-ENGINE-R653.4: the loop surface's refusal codes.
    ("Loops", &["LOOP_", "GAP_", "RATCHET_"]),
    // MEDULLA-DECISION-ENGINE-R700.4: the belief surface's refusal codes.
    ("BeliefRevise", &["BELIEF_", "CONTRADICTION_"]),
    ("RbacElevation", &["ELEVATION_"]),
    ("GovernedChange", &["GOVERNED_"]),
    ("Identity", &["IDENTITY_"]),
];
const DOMAIN_PREFIXES: &[(&str, &[&str])] = &[
    ("cluster", &["CLUSTER_", "FLEET_", "REGISTRY_", "RAFT_"]),
    (
        "query",
        &["AST_", "SQL_OWNER_", "RESULT_TOO_LARGE", "FOREIGN_READ_"],
    ),
    (
        "transactions",
        &["CHANGE_BATCH_", "REPLAY_NONCE_", "WRITE_CONFLICT"],
    ),
];
/// The node-write admission of run traces, step records, observation windows
/// and durable units (MEDULLA-ORCH-VOCAB-R009.15..R009.18, R009.700/701,
/// R013.1/R013.2) refuses with these codes on every node-write carrier.
const HARNESS_RECORD_ADMISSION_ERRORS: &[&str] = &[
    "DURABLE_UNIT_RUN_CONFLICT",
    "DURABLE_UNIT_RUN_MISSING",
    "RUN_TRACE_SHAPE_INVALID",
    "STEP_DIGEST_INVALID",
    "STEP_ORDER_INVALID",
    "WINDOW_CONTENT_CONFLICT",
    "WINDOW_RUN_REVISION_MISMATCH",
];

const EXACT_METHOD_ERRORS: &[(&str, &[&str])] = &[
    // MEDULLA-CONTRACT-R054: the served contract read's pagination refusals.
    (
        "DescribeContract",
        &["DESCRIBE_CURSOR_INVALID", "DESCRIBE_LIMIT_INVALID"],
    ),
    // MEDULLA-FEDERATED-QUERY-R709: a GeoSPARQL operand that is not WKT or
    // names an unsupported coordinate reference.
    ("Sparql", &["GEO_CRS_UNSUPPORTED", "GEO_LITERAL_INVALID"]),
    // MEDULLA-UNIFIED-DATA-PLANE-R038.3: a source that cannot give one snapshot.
    (
        "UnifiedQuery",
        &["SNAPSHOT_UNAVAILABLE", "CODE_MAP_STALE", "CODE_MAP_INVALID"],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R038.5.1: an out-of-range search capability.
    ("RegisterForeignSource", &["SEARCH_CAPABILITY_INVALID"]),
    // MEDULLA-TYPED-PACKS-R809: an unparsable or oversized Turtle payload.
    ("OntologyCanonicalHash", &["TURTLE_INVALID"]),
    // MEDULLA-DURABLE-KERNEL-R078: the served edge chokepoint refuses an
    // inverted validity interval; invalidation and supersession keep their
    // closed-edge and missing-edge refusals.
    ("AddEdge", &["VALIDITY_INVERTED"]),
    (
        "InvalidateEdge",
        &["VALIDITY_INVERTED", "EDGE_ALREADY_CLOSED", "EDGE_NOT_FOUND"],
    ),
    (
        "SupersedeEdge",
        &["VALIDITY_INVERTED", "EDGE_ALREADY_CLOSED", "EDGE_NOT_FOUND"],
    ),
    // The n-ary hypothesis read's typed refusals (medulla_query_types::graphlearn_nary).
    (
        "GraphLearnPredictNary",
        &[
            "NARY_ARITY_INVALID",
            "NARY_CONSTRAINT_INVALID",
            "NARY_REVISION_STALE",
            "NARY_SOURCE_UNAUTHORIZED",
            "NARY_SOURCE_UNRESOLVED",
        ],
    ),
    // MEDULLA-IDENTITY-R008.4/R008.5: the stored run-scope bound.
    (
        "AgentAssemble",
        &["SCOPE_EXCEEDS_POLICY", "SCOPE_POLICY_UNREADABLE"],
    ),
    // MEDULLA-PLACEMENT-R750.753: the job store's typed refusals
    // (`medulla_h_jobs::jobs::job_refusal`).
    (
        "AnalyticsJob",
        &[
            "CLAIM_PACED",
            "CONFLICT",
            "INVALID_CONFIGURATION",
            "INVALID_SCHEDULE",
            "PRIORITY_OUT_OF_RANGE",
            "SCHEDULE_EXISTS",
            "SCHEDULE_NOT_FOUND",
            "SCHEDULE_NOT_OWNER",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R652.3: a stale write and a malformed op.
    ("Claims", &["PARAMETER_INVALID", "REVISION_CONFLICT"]),
    ("BeliefRevise", &["PARAMETER_INVALID", "REVISION_CONFLICT"]),
    ("Loops", &["PARAMETER_INVALID", "REVISION_CONFLICT"]),
    ("GetRunWorkItem", &["RUN_ADMISSION_UNAVAILABLE"]),
    // MEDULLA-UNIFIED-DATA-PLANE-R040.4: a blocked embedding-model promotion.
    ("SemanticIndex", &["PROMOTION_BLOCKED"]),
    // Local-only authority: a clustered server refuses it with the typed
    // `LOCAL_ONLY_CLUSTER_REFUSAL` (server-mutation) instead of proposing it.
    ("SqlSourceBatch", &["CLUSTER_MUTATION_UNAVAILABLE"]),
    // MEDULLA-TYPED-PACKS-R041: a caller-minted pack-reserved component id.
    (
        "AgentComponent",
        &[
            "COMPONENT_WITHDRAWN",
            "LIFECYCLE_RETIRED_TERMINAL",
            "LIFECYCLE_STATE_MALFORMED",
            "LIFECYCLE_TRANSITION_INVALID",
            "MALFORMED_GUARDRAIL_RULE",
            "RESERVED_COMPONENT_ID",
            "WITHDRAWN_NOT_ALLOWED",
        ],
    ),
    (
        "GuardrailEntailment",
        &["GUARDRAIL_SCOPE_TOO_LARGE", "MALFORMED_GUARDRAIL_RULE"],
    ),
    // `ChangeEnvelope::validate` refuses carried engine-internal/native operations.
    (
        "ApplyChangeEnvelope",
        &[
            "CARRIER_INNER_METHOD_REFUSED",
            "CHANGE_ENVELOPE_CONTEXT_MISMATCH",
        ],
    ),
    (
        "ApplyChangeEnvelopeDraft",
        &["CARRIER_INNER_METHOD_REFUSED"],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R651.4.
    (
        "DocumentDelete",
        &[
            "CASCADE_TOO_LARGE",
            "DOCUMENT_DELETE_UNAVAILABLE",
            "DOCUMENT_HAS_REFERENCES",
            "DOCUMENT_NOT_FOUND",
        ],
    ),
    // MEDULLA-INGEST-WORKER-R001.1009.
    (
        "DataPrepDeclare",
        &[
            "DATA_PREP_DECLARATION_INVALID",
            "DATA_PREP_DECLARATION_TOO_LARGE",
            "INGEST_TENANT_MISMATCH",
        ],
    ),
    // MEDULLA-INGEST-WORKER-R001.1008.
    (
        "DataPrepDeclarationRead",
        &[
            "DATA_PREP_DECLARATION_INVALID",
            "INGEST_TENANT_MISMATCH",
        ],
    ),
    // MEDULLA-REPO-INGEST-R002.100.
    (
        "VerifyIngestReadback",
        &[
            "READBACK_DENIED",
            "READBACK_MISMATCH",
            "READBACK_PARTIAL",
            "READBACK_STALE",
        ],
    ),
    // MEDULLA-SUPPLY-VOCAB-R009.3: ecosystem package risk propagation.
    (
        "MineRiskPropagation",
        &["ECOSYSTEM_DEPTH_OUT_OF_RANGE", "ECOSYSTEM_PACKAGE_UNKNOWN"],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R651.7.4 / MEDULLA-DECISION-ENGINE-R802.4.
    (
        "EnrichDocument",
        &[
            "CHUNK_CONFIG_INVALID",
            "DOCUMENT_NOT_FOUND",
            "DOCUMENT_PROPERTIES_INVALID",
            "DOCUMENT_TEXT_MISSING",
            "EMBEDDING_DIMENSION_MISMATCH",
            "EMBEDDING_UNAVAILABLE",
            "ENRICHMENT_UNAVAILABLE",
            "RUNG_OUTRANKED",
            "WEB_BACKEND_INVALID",
        ],
    ),
    // MEDULLA-MEDIA-VOCAB-R001.101.3-.5 (codes of R001.101.6).
    (
        "MeetingSummaryCommit",
        &[
            "CITATION_OUTSIDE_TEXT",
            "RECEIPT_MISMATCH",
            "RECEIPT_UNKNOWN",
            "TRANSCRIPT_NOT_FINAL",
            "TRANSCRIPT_STALE",
        ],
    ),
    (
        "MeetingSummaryContext",
        &[
            "SUMMARY_ADMISSION_MISSING",
            "SUMMARY_LIMIT_EXCEEDED",
            "SUMMARY_LIMIT_INVALID",
            "TRANSCRIPT_NOT_FINAL",
            "TRANSCRIPT_STALE",
        ],
    ),
    (
        "MeetingTranscriptRevision",
        &["TRANSCRIPT_BASE_CONFLICT", "TRANSCRIPT_EVENT_CONFLICT"],
    ),
    (
        "BusHeartbeat",
        &[
            "BUS_FIELD_TOO_LARGE",
            "BUS_SCOPE_REQUIRED",
            "BUS_SESSION_FOREIGN",
        ],
    ),
    (
        "BusRegister",
        &[
            "BUS_FIELD_TOO_LARGE",
            "BUS_ROSTER_FULL",
            "BUS_SCOPE_REQUIRED",
            "BUS_SESSION_FOREIGN",
        ],
    ),
    ("BusRoster", &["BUS_SCOPE_REQUIRED"]),
    // MEDULLA-DURABLE-KERNEL-R085.5.
    (
        "ReadMemorySnapshot",
        &["MEMSYNC_NOT_AVAILABLE", "MEMSYNC_VERSION_MALFORMED"],
    ),
    (
        "ListMemorySnapshots",
        &[
            "MEMSYNC_CURSOR_EXPIRED",
            "MEMSYNC_CURSOR_INVALID",
            "MEMSYNC_NOT_AVAILABLE",
        ],
    ),
    (
        "SyncMemorySnapshot",
        &[
            "MEMSYNC_CONTENT_MALFORMED",
            "MEMSYNC_CONTENT_TOO_LARGE",
            "MEMSYNC_DIGEST_MISMATCH",
            "MEMSYNC_NOT_AVAILABLE",
            "MEMSYNC_VERSION_MALFORMED",
        ],
    ),
    (
        "SelectWebPages",
        &[
            "SELECTION_UNAVAILABLE",
            "WEB_SELECTION_BUDGET_INVALID",
            "WEB_SELECTION_NO_CANDIDATES",
            "WEB_SELECTION_QUERY_MISSING",
            "WEB_SELECTION_TOO_MANY_CANDIDATES",
        ],
    ),
    // MEDULLA-REPO-INGEST-R007/R008: an over-budget batch is refused whole.
    // MEDULLA-REPO-INGEST-R801: a cycle among architecture layers, and a malformed
    // commit digest of the scoped history, refuse the batch.
    (
        "IndexRepository",
        &[
            "INVALID_COMMIT_DIGEST",
            "LAYER_CYCLE",
            "REPOSITORY_BATCH_TOO_LARGE",
        ],
    ),
    // `ensure_job_workers` refuses an invalid analytics worker timing.
    ("AnalyticsJob", &["INVALID_CONFIGURATION"]),
    // MEDULLA-FINANCE-PRIMITIVES-R018.3: mismatched factor record lengths.
    ("FinanceFactorRank", &["FACTOR_RECORD_INVALID"]),
    // MEDULLA-FINANCE-PRIMITIVES-R018.5: an empty provenance list or an unresolved key.
    ("FinanceFactorRecord", &["FACTOR_RECORD_INVALID"]),
    // MEDULLA-FINANCE-PRIMITIVES-R750.752: stable codes of the research kernels.
    (
        "FinancePatternClassify",
        &["PATTERN_WINDOW_SHORT", "PATTERN_INPUT_INVALID"],
    ),
    (
        "FinanceProfitAttribution",
        &["ATTRIBUTION_WEIGHTS_INVALID", "ATTRIBUTION_INPUT_INVALID"],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R904: no current record for the binding in the caller's tenant.
    ("WalletKeyTrustGet", &["WALLET_KEY_TRUST_NOT_FOUND"]),
    // MEDULLA-FEDERATED-QUERY-R724: a put with bad bounds, an unregistered scope or
    // an unknown mapping.
    (
        "SchemaProjections",
        &["SCHEMA_BOUNDARY_INVALID", "BOUNDARY_MAPPING_UNKNOWN"],
    ),
    // MEDULLA-BRANCH-MERGE-R001: the five CreateBranch refusals.
    (
        "CreateBranch",
        &[
            "BRANCH_BASE_UNKNOWN",
            "BRANCH_EXISTS",
            "BRANCH_NAME_INVALID",
            "BRANCH_NAME_RESERVED",
            "BRANCH_TENANT_DENIED",
        ],
    ),
    // MEDULLA-BRANCH-MERGE-R002: the branch-scoped write refusals, all before any graph change.
    (
        "BranchWrite",
        &[
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "BRANCH_WRITE_INVALID",
        ],
    ),
    // MEDULLA-SCM-R002: the changeset refusals, all before the chain changes.
    (
        "SubmitChangeset",
        &[
            "BLOB_DIGEST_MISMATCH",
            "BLOB_NOT_FOUND",
            "BLOB_TOO_LARGE",
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "CHANGESET_EMPTY",
            "CHANGESET_INVALID",
            "CHANGESET_PARENT_NOT_ON_BRANCH",
        ],
    ),
    // MEDULLA-IDENTITY-R701: no delegation record for the work item in the caller's tenant.
    ("GetDelegationRecord", &["DELEGATION_UNKNOWN"]),
    // MEDULLA-DECISION-ENGINE-R152.4: a zero budget, too many node ids or a fact with no source.
    (
        "ContextFacts",
        &[
            "CONTEXT_FACTS_BUDGET_ZERO",
            "CONTEXT_FACTS_TOO_MANY_NODES",
            "FACT_PROVENANCE_MISSING",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R153.4: a span the stored table cannot serve.
    (
        "ReadSpan",
        &[
            "DOCUMENT_NOT_FOUND",
            "SPAN_INVERTED",
            "SPAN_OUT_OF_BOUNDS",
            "SPAN_TOO_LARGE",
            "TOC_MISSING",
        ],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R004.3.2: the record put's closed refusals.
    (
        "FinanceRecordPut",
        &[
            "FINANCE_RECORD_VERSION_UNKNOWN",
            "RECORD_DERIVED_READONLY",
            "RECORD_HEAD_CONFLICT",
            "RECORD_SHAPE_INVALID",
            // MEDULLA-FINANCE-PRIMITIVES-R751.752: the research records' refusals.
            "DEBATE_ROUND_OUT_OF_ORDER",
            "RECORD_IMMUTABLE",
            "RECORD_TENANT_MISMATCH",
            "SCORE_EVIDENCE_MISSING",
            "SCORE_RANGE_INVALID",
            "SCORE_WEIGHTS_INVALID",
            "STRATEGY_CARD_LICENCE_MISSING",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R166.3/R166.4: the workflow registry's closed refusals.
    (
        "WorkflowRegistry",
        &[
            "WORKFLOW_ID_MISSING",
            "WORKFLOW_VERSION_MISSING",
            "WORKFLOW_DIGEST_INVALID",
            "WORKFLOW_IDENTITY_UNKNOWN",
            "WORKFLOW_UNAUTHORIZED",
        ],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R011.2.3: the alert door's closed refusals.
    (
        "FinanceAlertRaise",
        &["ALERT_ENVELOPE_INVALID", "ALERT_REVISION_CONFLICT"],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R011.4: an empty receipt idempotency key.
    ("CreateNodeIfAbsent", &["RECEIPT_KEY_INVALID"]),
    ("GetNodes", &["RESULT_TOO_LARGE"]),
    ("GetEdges", &["RESULT_TOO_LARGE"]),
    (
        "CreateGraph",
        &["REPLAY_NONCE_CONSUMED", "RESERVED_GRAPH_NAME"],
    ),
    (
        "AddNode",
        &[
            "CLIENT_SESSION_PRINCIPAL_MISMATCH",
            "COMMIT_QUEUE_FULL",
            "REPLAY_NONCE_CONSUMED",
            "WORKSTREAM_PRINCIPAL_MISMATCH",
        ],
    ),
    ("CypherQuery", &["REPLAY_NONCE_CONSUMED"]),
    ("GraphQl", &["REPLAY_NONCE_CONSUMED"]),
    // MEDULLA-DURABLE-KERNEL-R865 (child input), R750.750.1 (signed agent-turn
    // envelope) and R750.752 (conversation head).
    (
        "SubmitWorkItem",
        &[
            "CARRIER_SIGNATURE_INVALID",
            "CHILD_TARGET_TERMINAL",
            // MEDULLA-DURABLE-KERNEL-R750.752: a moved conversation head.
            "CONVERSATION_CONFLICT",
            "ENVELOPE_FIELD_MISSING",
            "IDEMPOTENCY_CONFLICT",
            "REPLAY_NONCE_CONSUMED",
            "REVISION_CONFLICT",
            "WORK_QUEUE_FULL",
        ],
    ),
    ("ClaimWorkItem", &["REPLAY_NONCE_CONSUMED"]),
    ("PublishRunCheckpoint", &["REPLAY_NONCE_CONSUMED"]),
    ("GetRunCheckpoint", &["CONFLICT"]),
    // MEDULLA-DURABLE-KERNEL-R820.4 uncertain-effect refusals.
    (
        "MarkWorkItemEffectStarted",
        &[
            "REPLAY_NONCE_CONSUMED",
            "REVISION_CONFLICT",
            "WORK_ITEM_CLAIM_STALE",
        ],
    ),
    (
        "RecordEffectReceipt",
        &[
            "EFFECT_RECEIPT_CONFLICT",
            "EFFECT_RECEIPT_MISMATCH",
            "REPLAY_NONCE_CONSUMED",
            "REVISION_CONFLICT",
        ],
    ),
    (
        "SubmitAdmittedRequest",
        &[
            "ADMITTED_REQUEST_DIGEST_MISMATCH",
            "ADMITTED_REQUEST_TOO_LARGE",
            "CONFLICT",
            "REPLAY_NONCE_CONSUMED",
            "REVISION_CONFLICT",
        ],
    ),
    ("TsListSeries", &["REPLAY_NONCE_CONSUMED"]),
    // delegation/validation.rs refuses a retired Agent Library head or entry.
    (
        "KgDelegate",
        &[
            "CLUSTER_CONFIGURATION_INVALID",
            "REPLAY_NONCE_CONSUMED",
            "STALE_AGENT_LIBRARY_REVISION",
        ],
    ),
    ("ApplyChangeEnvelopes", &["ABORTED_ATOMIC_GRAPH_BATCH"]),
    ("ApplyChangeEnvelopeDrafts", &["ABORTED_ATOMIC_GRAPH_BATCH"]),
    (
        "RegisterForeignSource",
        &["FOREIGN_SOURCE_POLICY_UNBOOTSTRAPPED"],
    ),
    // MEDULLA-FEDERATED-QUERY-R006.1: temporal rule-derivation refusals.
    // MEDULLA-FEDERATED-QUERY-R053.100 / R053.700 / R053.701: foreign Cypher,
    // Trino and Spark refusals on the served `Uql` FOREIGN path.
    (
        "Uql",
        &[
            "CYPHER_KEY_INVALID",
            "DERIVED_BOUND_MALFORMED",
            "DERIVED_ORIGIN_MISSING",
            "DERIVED_WINDOW_CONFLICT",
            "DERIVED_WRITE_UNAUTHORIZED",
            "FENCE_FIELD_MISSING",
            "PRESET_UNAVAILABLE",
            "RULE_VERSION_UNSUPPORTED",
            "SNAPSHOT_UNKNOWN",
        ],
    ),
    (
        "MutationOutbox",
        &[
            "CORRUPT_OUTBOX",
            "INVALID_CONFIGURATION",
            "OUTBOX_OWNER_UNAVAILABLE",
            "OUTBOX_OWNER_UNKNOWN",
            "OUTBOX_SCOPE_MISMATCH",
        ],
    ),
    ("AgentLibrary", &["STALE_AGENT_LIBRARY_REVISION"]),
    // MEDULLA-UNIFIED-DATA-PLANE-R702.6.
    ("LineageIngest", &["LINEAGE_EVENT_MALFORMED"]),
    ("WriteBack", &["CORRUPT_WRITE_BACK"]),
    ("TxnPlanWriteback", &["CORRUPT_WRITE_BACK"]),
    ("ActionRegister", &["ACTION_SPEC_INVALID"]),
    (
        "ActionSubmit",
        &[
            "ACTION_CRITERION_FAILED",
            "ACTION_MISSING_PARAMETER",
            "ACTION_NOT_REGISTERED",
            "ACTION_PARAMETER_TYPE_MISMATCH",
            "ACTION_UNKNOWN_PARAMETER",
            "ACTION_WRITE_BACK_UNATTACHED",
        ],
    ),
    (
        "IncidentCorrelate",
        &["TENANT_SCOPE_REQUIRED", "ANOMALY_RESOURCE_REQUIRED"],
    ),
    (
        "IncidentEvidence",
        &["TENANT_SCOPE_REQUIRED", "INCIDENT_NOT_FOUND"],
    ),
    (
        "IncidentRecordAnomaly",
        &["TENANT_SCOPE_REQUIRED", "ANOMALY_RESOURCE_REQUIRED"],
    ),
    (
        "IncidentRelatedClaims",
        &["TENANT_SCOPE_REQUIRED", "INCIDENT_NOT_FOUND"],
    ),
    // MEDULLA-SDLC-R002.4/R002.5: the served release record's refusals.
    (
        "RecordRelease",
        &[
            "RELEASE_HEAD_UNKNOWN",
            "RELEASE_VERSION_TAKEN",
            "RELEASE_SEALED",
        ],
    ),
    (
        "IncidentSetStatus",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INCIDENT_STATUS_SKIP",
            "INCIDENT_NOT_FOUND",
        ],
    ),
    (
        "TelemetryDerive",
        &[
            "TELEMETRY_UNAVAILABLE",
            "TELEMETRY_WINDOW_TOO_LARGE",
            "SCHEMA_INCONSISTENT",
            "GRAPH_NOT_FOUND",
        ],
    ),
    (
        "Decide",
        &["DECISIONS", "UNSUPPORTED_COALITION", "CAPACITY_UNAVAILABLE"],
    ),
    // The identity store's namespace is fenced off the two older RBAC
    // writers (identity_store_admin.rs). `RegisterIdentity` is refused for a
    // principal the store manages AND for a role in the store's `idm:`
    // namespace; `RbacAdmin` writes roles and grants only, so it meets the
    // namespace fence alone.
    (
        "RegisterIdentity",
        &["IDENTITY_STORE_MANAGED", "IDENTITY_STORE_NAMESPACE"],
    ),
    ("RbacAdmin", &["IDENTITY_STORE_NAMESPACE"]),
    // MEDULLA-FEDERATED-QUERY-R720.5: the DECAY and CLUSTER BY EMBEDDING stage refusals.
    ("Uql", &["UQL_E_CLUSTER_NO_VECTOR", "UQL_E_DECAY_NO_TIME"]),
    (
        "AuditAppend",
        &[
            "AUDIT_CLASS_REQUIRED",
            "AUDIT_CLASS_UNKNOWN",
            "AUDIT_IDEMPOTENCY_CONFLICT",
            "AUDIT_RESERVATION_MISMATCH",
            "AUDIT_RESERVATION_REQUIRED",
            "AUDIT_WRITER_UNAVAILABLE",
        ],
    ),
];

fn add_engine_errors(d: &MethodDescriptor, codes: &mut BTreeSet<&'static str>) {
    for code in EngineErrorCode::ALL {
        let token = code.as_str();
        if SHARED_ENGINE_ERRORS.contains(&token)
            || (d.domain == "query" && token.starts_with("UQL_"))
        {
            codes.insert(token);
        }
    }
}

fn server_prefixes(d: &MethodDescriptor) -> &'static [&'static str] {
    METHOD_PREFIXES
        .iter()
        .find_map(|(method, prefixes)| d.id.as_str().starts_with(method).then_some(*prefixes))
        .or_else(|| {
            DOMAIN_PREFIXES
                .iter()
                .find_map(|(domain, prefixes)| (d.domain == *domain).then_some(*prefixes))
        })
        .unwrap_or(&[])
}

fn add_server_errors(d: &MethodDescriptor, codes: &mut BTreeSet<&'static str>) {
    let prefixes = server_prefixes(d);
    for code in ServerErrorCode::ALL {
        let token = code.as_str();
        if SHARED_SERVER_ERRORS.contains(&token)
            || prefixes.iter().any(|prefix| token.starts_with(prefix))
        {
            codes.insert(token);
        }
    }
}

fn add_owner_errors(d: &MethodDescriptor, codes: &mut BTreeSet<&'static str>) {
    // Foreign reads revalidate current connector-pack approvals at every step.
    // Add those refusal families without replacing each method's domain errors.
    let id = d.id.as_str();
    if matches!(id, "Uql" | "UnifiedQuery" | "RegisterForeignSource")
        || id.starts_with("ForeignRead")
    {
        let prefixes = [
            "SOURCE_APPROVAL_",
            "SOURCE_BINDING_",
            "SOURCE_CONTRACT_",
            "SOURCE_REGISTRATION_",
        ];
        codes.extend(
            ServerErrorCode::ALL
                .iter()
                .map(|code| code.as_str())
                .filter(|code| prefixes.iter().any(|prefix| code.starts_with(prefix))),
        );
        // These two codes already belong to the connector-pack typed catalog.
        codes.extend(["SOURCE_CONTRACT_MISMATCH", "SOURCE_REGISTRATION_NOT_FOUND"]);
    }
    // MEDULLA-IDENTITY-R610.4 / R610.5: every served graph read is scoped to the
    // verified tenant and audited.
    if matches!(
        id,
        "Uql" | "UnifiedQuery" | "Sql" | "NlQuery" | "NlQueryExplain"
    ) {
        codes.extend(["AUDIT_UNAVAILABLE", "QUERY_UNSCOPABLE"]);
    }
    // MEDULLA-IDENTITY-R023.10: typed principal metadata admission runs in the
    // mutation compiler, so every node-writing route can refuse an invalid record.
    if matches!(
        id,
        "AddNode"
            | "CreateNodeIfAbsent"
            | "TxnAddNode"
            | "ApplyMutation"
            | "Sql"
            | "CypherQuery"
            | "AddTriples"
    ) {
        codes.extend([
            "METADATA_UNTYPED",
            "METADATA_PRINCIPAL_UNKNOWN",
            "METADATA_SCHEMA_INVALID",
        ]);
    }
    // MEDULLA-FEDERATED-QUERY-R058.4.1: `max_steps` outside 1 to 4.
    if id == "NlQuery" {
        codes.push("NL_STEPS_INVALID");
    }
    // MEDULLA-IDENTITY-R023.4: a point read of a unit outside the caller's scope.
    // MEDULLA-DURABLE-KERNEL-R086.3: a durable-tier fault on the miss path.
    if id == "GetNodeProperties" {
        codes.extend(["UNIT_OUT_OF_SCOPE", "DURABLE_READ_UNAVAILABLE"]);
    }
    if d.policy.is_durable() {
        codes.insert("CORRUPT_MUTATION_LEDGER");
    }
    // The harness vocabulary's typed write admission (MEDULLA-ORCH-VOCAB-R801/R802)
    // runs inside every node-write method: the direct AddNode gateway path and
    // the ChangeEnvelope carriers (single and batch, draft and confirmed).
    if matches!(
        d.id.as_str(),
        "AddNode"
            | "ApplyChangeEnvelope"
            | "ApplyChangeEnvelopeDraft"
            | "ApplyChangeEnvelopes"
            | "ApplyChangeEnvelopeDrafts"
    ) {
        codes.extend(typed_codes(
            HarnessVocabularyErrorCode::ALL,
            HarnessVocabularyErrorCode::as_str,
        ));
        codes.extend(HARNESS_RECORD_ADMISSION_ERRORS.iter().copied());
    }
    if d.domain == "security" {
        codes.insert("POLICY_NATIVE_AUTHORITY_REQUIRED");
    }
    if d.id.as_str().starts_with("Blob") {
        codes.insert("BODY_DIGEST_MISMATCH");
    }
    for (method, extras) in EXACT_METHOD_ERRORS {
        if d.id.as_str() == *method {
            codes.extend(extras.iter().copied());
        }
    }
}

fn add_typed_handler_errors(d: &MethodDescriptor, codes: &mut BTreeSet<&'static str>) {
    let id = d.id.as_str();
    if matches!(
        id,
        "Uql" | "NlQuery" | "NlQueryExplain" | "NlQueryPlan" | "KnowledgeStream"
    ) {
        codes.extend(typed_codes(UqlCode::ALL, UqlCode::as_str));
    }
    if id.starts_with("Decision") || id.starts_with("Decide") {
        codes.extend(typed_codes(
            DecisionErrorCode::ALL,
            DecisionErrorCode::as_str,
        ));
        codes.extend(typed_codes(
            StatisticalErrorCode::ALL,
            StatisticalErrorCode::as_str,
        ));
        codes.extend(typed_codes(
            HarnessContrastErrorCode::ALL,
            HarnessContrastErrorCode::as_str,
        ));
    }
    if id.starts_with("GraphSchema") {
        codes.extend(typed_codes(
            GraphSchemaErrorCode::ALL,
            GraphSchemaErrorCode::as_str,
        ));
    }
    if id.starts_with("ConnectorPack") {
        codes.extend(typed_codes(
            PackWriteErrorCode::ALL,
            PackWriteErrorCode::as_str,
        ));
    }
    if id == "Solve" {
        codes.extend(typed_codes(SolveErrorCode::ALL, SolveErrorCode::as_str));
    }
}

/// The served methods that move an explicit state machine (MEDULLA-ORCH-VOCAB-R811.8,
/// organization constitution Article XXXI). Each may answer the typed record
/// `{code: TRANSITION_REFUSED, machine, from, event, reason}`.
pub const MACHINE_METHODS: &[&str] = &[
    "SubmitWorkItem",
    "ClaimWorkItem",
    "CancelWorkItem",
    "ReleaseWorkItemResources",
    "ReleaseCapacity",
    "ResourceReservationStatus",
    "AnalyticsJob",
    "BrokerConsume",
    "BrokerAck",
    "BrokerReject",
    "BrokerAckTag",
    "BrokerNackTag",
];

/// The wire code of a refused machine transition.
pub const TRANSITION_REFUSED: &str = "TRANSITION_REFUSED";

/// Include shared boundary refusals and the typed codes that a method's
/// handler family can return. A sorted set keeps the generated API stable.
pub fn method_error_set(d: &MethodDescriptor) -> Vec<&'static str> {
    let mut codes: BTreeSet<_> = d.error_set.iter().copied().collect();
    add_engine_errors(d, &mut codes);
    add_server_errors(d, &mut codes);
    add_owner_errors(d, &mut codes);
    add_typed_handler_errors(d, &mut codes);
    if MACHINE_METHODS.contains(&d.id.as_str()) {
        codes.insert(TRANSITION_REFUSED);
    }
    codes.into_iter().collect()
}

/// Whether the canonical method contract permits this served code.
/// An absent method or code fails closed. The table is derived once from the
/// same descriptors and function used by gen_contract.
pub fn method_allows_error(method: &str, code: &str) -> bool {
    static ALLOWED: OnceLock<BTreeMap<&'static str, BTreeSet<&'static str>>> = OnceLock::new();
    ALLOWED
        .get_or_init(|| {
            crate::method_descriptors()
                .map(|descriptor| {
                    (
                        descriptor.id.as_str(),
                        method_error_set(&descriptor).into_iter().collect(),
                    )
                })
                .collect()
        })
        .get(method)
        .is_some_and(|codes| codes.contains(code))
}

#[cfg(test)]
mod tests {
    use super::method_allows_error;

    // spec: MEDULLA-ORCH-VOCAB-R811.8

    #[test]
    fn every_machine_method_lists_transition_refused() {
        let served: std::collections::BTreeSet<&str> = crate::method_descriptors()
            .map(|descriptor| descriptor.id.as_str())
            .collect();
        for method in super::MACHINE_METHODS {
            assert!(served.contains(method), "{method} is not a served method");
            assert!(
                method_allows_error(method, super::TRANSITION_REFUSED),
                "{method} moves a machine but its contract lacks TRANSITION_REFUSED"
            );
        }
        for method in served {
            if !super::MACHINE_METHODS.contains(&method) {
                assert!(
                    !method_allows_error(method, super::TRANSITION_REFUSED),
                    "{method} is not a machine method but lists TRANSITION_REFUSED"
                );
            }
        }
        assert_eq!(
            super::TRANSITION_REFUSED,
            medulla_types::transition_refused::TRANSITION_REFUSED
        );
    }

    #[test]
    fn connector_and_foreign_refusals_are_routed_to_their_served_methods() {
        assert!(method_allows_error("ConnectorPack", "SEED_TURTLE_INVALID"));
        assert!(method_allows_error(
            "UnifiedQuery",
            "SOURCE_APPROVAL_NOT_ADMITTED"
        ));
        assert!(method_allows_error(
            "UnifiedQuery",
            "SOURCE_BINDING_MISMATCH"
        ));
        assert!(method_allows_error("Uql", "SOURCE_APPROVAL_NOT_ADMITTED"));
        assert!(method_allows_error(
            "ForeignReadOpen",
            "SOURCE_BINDING_MISMATCH"
        ));
        assert!(!method_allows_error(
            "CreateGraph",
            "SOURCE_BINDING_MISMATCH"
        ));
        assert!(method_allows_error("AnalyticsJob", "INVALID_CONFIGURATION"));
        assert!(method_allows_error(
            "MutationOutbox",
            "INVALID_CONFIGURATION"
        ));
        assert!(!method_allows_error("CreateGraph", "INVALID_CONFIGURATION"));
    }

    // spec: MEDULLA-DECISION-ENGINE-R073.5
    #[test]
    fn withdrawn_component_refusal_is_declared_for_component_admission() {
        assert!(method_allows_error("AgentComponent", "COMPONENT_WITHDRAWN"));
        for other in ["CreateGraph", "GetNodes", "GuardrailEntailment"] {
            assert!(!method_allows_error(other, "COMPONENT_WITHDRAWN"));
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R041
    #[test]
    fn reserved_component_id_refusal_is_declared_for_component_writes() {
        assert!(method_allows_error(
            "AgentComponent",
            "RESERVED_COMPONENT_ID"
        ));
        for other in ["CreateGraph", "GetNodes", "GuardrailEntailment"] {
            assert!(!method_allows_error(other, "RESERVED_COMPONENT_ID"));
        }
    }

    // spec: MEDULLA-CONTRACT-R014
    #[test]
    fn governed_refusals_are_only_routed_to_governed_changes() {
        for code in medulla_types::contract::ServerErrorCode::ALL
            .iter()
            .map(|code| code.as_str())
            .filter(|code| code.starts_with("GOVERNED_"))
        {
            assert!(method_allows_error("GovernedChange", code));
            for other in ["RbacElevation", "Identity", "QueryForeignColumns"] {
                assert!(!method_allows_error(other, code));
            }
        }
        assert!(!method_allows_error(
            "GovernedChange",
            "GOVERNED_NOT_REGISTERED"
        ));
    }

    #[test]
    fn runtime_error_routing_is_method_specific_and_closed() {
        assert!(method_allows_error("CreateGraph", "ACCESS_DENIED"));
        assert!(method_allows_error("CreateGraph", "OPERATION_REDIRECTED"));
        assert!(method_allows_error("CypherQuery", "METHOD_NOT_YET_SERVED"));
        assert!(method_allows_error("Sql", "METHOD_NOT_YET_SERVED"));
        assert!(!method_allows_error("CreateGraph", "UNSUPPORTED_COALITION"));
        assert!(!method_allows_error("NotADeclaredMethod", "ACCESS_DENIED"));
        assert!(method_allows_error("GetNodes", "RESULT_TOO_LARGE"));
        assert!(method_allows_error("GetEdges", "RESULT_TOO_LARGE"));
        assert!(method_allows_error("Uql", "UQL_UNBOUND_PARAMETER"));
        assert!(!method_allows_error("CreateGraph", "UQL_UNBOUND_PARAMETER"));
        assert!(!method_allows_error("GetNodesByLabel", "RESULT_TOO_LARGE"));
        for method in [
            "CreateGraph",
            "AddNode",
            "CypherQuery",
            "GraphQl",
            "SubmitWorkItem",
            "ClaimWorkItem",
            "PublishRunCheckpoint",
            "TsListSeries",
        ] {
            assert!(
                method_allows_error(method, "REPLAY_NONCE_CONSUMED"),
                "{method}"
            );
        }
        assert!(!method_allows_error(
            "TsListSeries",
            "UNSUPPORTED_COALITION"
        ));
        assert!(method_allows_error(
            "KgDelegate",
            "CLUSTER_CONFIGURATION_INVALID"
        ));
        assert!(method_allows_error("KgDelegate", "REPLAY_NONCE_CONSUMED"));
        assert!(method_allows_error("CreateGraph", "RESERVED_GRAPH_NAME"));
        assert!(!method_allows_error("AddNode", "RESERVED_GRAPH_NAME"));
        assert!(!method_allows_error(
            "SubmitWorkItem",
            "CLUSTER_CONFIGURATION_INVALID"
        ));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R028.1.1
    #[test]
    fn checkpoint_refusals_are_declared_without_widening_other_reads() {
        assert!(method_allows_error("GetRunCheckpoint", "CONFLICT"));
        assert!(!method_allows_error(
            "GetRunCheckpoint",
            "REPLAY_NONCE_CONSUMED"
        ));
        assert!(!method_allows_error("GetNodes", "CONFLICT"));
        assert!(!method_allows_error(
            "PublishRunCheckpoint",
            "UNSUPPORTED_COALITION"
        ));
    }

    #[test]
    fn the_older_identity_writers_declare_exactly_the_store_fences_they_meet() {
        for (method, code, declared) in [
            ("RegisterIdentity", "IDENTITY_STORE_MANAGED", true),
            ("RegisterIdentity", "IDENTITY_STORE_NAMESPACE", true),
            ("RbacAdmin", "IDENTITY_STORE_NAMESPACE", true),
            ("RbacAdmin", "IDENTITY_STORE_MANAGED", false),
            ("GetIdentity", "IDENTITY_STORE_NAMESPACE", false),
        ] {
            assert_eq!(
                method_allows_error(method, code),
                declared,
                "{method} {code}"
            );
        }
    }
}
