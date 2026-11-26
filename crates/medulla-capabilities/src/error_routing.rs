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
    // Any durable commit can lose the object-store root's `HEAD` fence (MEDULLA-OBJECT-STORAGE-BACKEND-R003).
    "OBJECT_PUBLISH_STALE",
    // So can a write over a graph with no catalog row to publish under.
    "OBJECT_GRAPH_IDENTITY_MISSING",
    // A restore from the object root refuses a graph sealed under a key this store lacks.
    "OBJECT_SEAL_KEY_MISSING",
];

/// A method-specific family wins over the wider domain family.
const METHOD_PREFIXES: &[(&str, &[&str])] = &[
    ("ArgumentMap", &["AIF_"]),
    ("LiveArtifact", &["LIVE_ARTIFACT_"]),
    // MEDULLA-FEDERATED-QUERY-R608.4.
    ("CausalVerify", &["CAUSAL_"]),
    // MEDULLA-CLUSTER-R001.3: k-means refuses with the clustering kernel's typed codes.
    ("DsKMeans", &["CLUSTERING_"]),
    // MEDULLA-FEDERATED-QUERY-R708.4.
    ("StoredQuery", &["STORED_QUERY_"]),
    // MEDULLA-SRC-AUTH-R012.7: a sync passes through the refusal of the foreign read it consumes.
    ("SourceSync", &["FOREIGN_READ_"]),
    // MEDULLA-UNIFIED-DATA-PLANE-R030.3.
    ("ApproveEntityLink", &["LINK_"]),
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
            // MEDULLA-TYPED-PACKS-R097.2.7.3: an external schema the import refuses.
            "EXTERNAL_",
            "SOURCE_APPROVAL_",
            "SOURCE_CONTRACT_",
            "SOURCE_REGISTRATION_",
            // MEDULLA-FEDERATED-QUERY-R722.4: an access contract the import refuses.
            "CONTRACT_APPROVAL",
            "CONTRACT_SECRET",
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
    ("Claims", &["ARTIFACT_", "CLAIM_", "EVIDENCE_", "PROOF_"]),
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

/// The refusals of one state-space step (`medulla_tsdb::derive::state_space`).
const STATE_SPACE_ERRORS: &[&str] = &[
    "STATE_SCHEMA_UNAVAILABLE",
    "STATE_CHECKPOINT_STALE",
    "STATE_DIMENSION_MISMATCH",
    "STATE_DOMAIN_VIOLATION",
    "STATE_FACTOR_ORDER",
    "STATE_FUTURE_LEAK",
    "STATE_JACOBIAN_INVALID",
    "STATE_MODEL_UNREGISTERED",
    "STATE_NOISE_CORRELATED",
    "STATE_RETUNE_BUDGET",
    "STATE_TRANSFORM_INVALID",
    "STATE_UNIT_MISMATCH",
    "STATE_WARMUP",
    "STATE_WINDOW_INVALID",
    "KERNEL_INPUT_NONFINITE",
    "KERNEL_PARAM_INVALID",
    "KERNEL_SINGULAR",
    "KERNEL_STATE_BUDGET",
    "KERNEL_WORK_BUDGET",
];

const EXACT_METHOD_ERRORS: &[(&str, &[&str])] = &[
    // MEDULLA-DURABLE-KERNEL-R871: a Batch over the operation cap is refused whole.
    ("Batch", &["BATCH_TOO_LARGE", "INVALID_ARGUMENT"]),
    // MEDULLA-FINANCE-PRIMITIVES-R002.9/R002.10: a state-space definition, and the
    // filter step a source append runs, refuse with the series and kernel codes of
    // `SeriesRefusal::render`.
    ("TsDefineStateSpace", STATE_SPACE_ERRORS),
    ("TsAppend", STATE_SPACE_ERRORS),
    // MEDULLA-ORCH-VOCAB-R750.752: an undefined transition and an unregistered
    // definition are refused with their own codes.
    (
        "Statechart",
        &["INSTANCE_DEFINITION_UNREGISTERED", "STATECHART_NO_TRANSITION"],
    ),
    // MEDULLA-ORCH-VOCAB-R006.5: a graph template whose topology facts break a publish
    // shape rule is refused with the rule's detail.
    ("AgentGraph", &["TOPOLOGY_SHAPE_INVALID"]),
    // MEDULLA-CONTRACT-R054: the served contract read's pagination refusals.
    (
        "DescribeContract",
        &["DESCRIBE_CURSOR_INVALID", "DESCRIBE_LIMIT_INVALID"],
    ),
    // MEDULLA-OBSERVE-R006.1: the runtime introspection read's refusals.
    (
        "Introspect",
        &[
            "INTROSPECT_LIMIT_INVALID",
            "INTROSPECT_CURSOR_INVALID",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R709: a GeoSPARQL operand that is not WKT or
    // names an unsupported coordinate reference.
    // MEDULLA-CQ-CONN-R084.7: a competency question names a pack the tenant has not imported.
    (
        "Sparql",
        &[
            "GEO_CRS_UNSUPPORTED",
            "GEO_LITERAL_INVALID",
            "PACK_NOT_IMPORTED",
            // MEDULLA-FEDERATED-QUERY-R731: an interactive query past its node budget.
            "QUERY_BUDGET_EXCEEDED",
            // MEDULLA-FEDERATED-QUERY-R740.2: a `graph_media_type` outside the six.
            "RDF_MEDIA_TYPE_UNSUPPORTED",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R740.1: the graph write of one RDF body refuses by media type,
    // by parse, by a quad of another graph and by size.
    (
        "ApplyMutation",
        &[
            "RDF_MEDIA_TYPE_UNSUPPORTED",
            "RDF_PARSE_FAILED",
            "RDF_DATASET_GRAPH_MISMATCH",
            "RDF_BODY_TOO_LARGE",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R705.4: feedback on an unreadable document, and the
    // per-principal hourly event limit.
    (
        "RetrievalFeedbackRecord",
        &["FEEDBACK_TARGET_HIDDEN", "FEEDBACK_RATE_LIMIT"],
    ),
    // MEDULLA-TYPED-PACKS-R807: the conformance read or revoke finds no current evidence.
    ("ConnectorPack", &["CONFORMANCE_EVIDENCE_ABSENT"]),
    // MEDULLA-DECISION-ENGINE-R160.4: skill package publication refusals.
    (
        "PublishSkillPackage",
        &[
            "PACKAGE_DIGEST_MISMATCH",
            "PACKAGE_RUN_SET_INVALID",
            "PACKAGE_RUN_HISTORY_MISSING",
            "PACKAGE_VALIDATION_FAILED",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R038.3: a source that cannot give one snapshot.
    // MEDULLA-UNIFIED-DATA-PLANE-R038.5.3 / MEDULLA-FEDERATED-QUERY-R073.3: a remote
    // branch whose source cannot be reached.
    (
        "UnifiedQuery",
        &[
            // MEDULLA-FEDERATED-QUERY-R706.4: a policy names a tag with no registered marking.
            "POLICY_TAG_UNKNOWN",
            // MEDULLA-FEDERATED-QUERY-R706.10: a shape policy that cannot be read or evaluated,
            // and a result node a `refuse` shape policy rejects.
            "POLICY_SHAPE_INVALID",
            "POLICY_VIOLATION",
            "SNAPSHOT_UNAVAILABLE",
            "CODE_MAP_STALE",
            "CODE_MAP_INVALID",
            "SOURCE_UNREACHABLE",
            // MEDULLA-UNIFIED-DATA-PLANE-R038.5.4: a search argument outside
            // what the remote source's tool declares.
            "UNSUPPORTED_SEARCH_ARGUMENT",
            "EMBEDDING_SPACE_INVALID",
            // MEDULLA-FEDERATED-QUERY-R040.1.7: the served retrieval request refusals.
            "EMBEDDING_SPACE_INCOMPATIBLE",
            "QUERY_ENCODER_UNAVAILABLE",
            "CONSTRAINT_INVALID",
            "GRAPH_REVISION_STALE",
            "CONSTRAINT_UNEVALUABLE",
            "TOPIC_WEIGHT_INVALID",
            "TOPIC_SPACE_MISMATCH",
            "RETRIEVAL_K_INVALID",
            "TOPIC_CORPUS_STALE",
            "TOPIC_BACKEND_UNAVAILABLE",
            "TOPIC_ASSIGNMENT_MISSING",
            "TOPIC_ASSIGNMENT_DUPLICATE",
            "TOPIC_ASSIGNMENT_STALE",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R038.6.3/.4: a view registration or a view read refuses with
    // the view codes before any source statement.
    (
        "RegisterGraphView",
        &[
            "GRAPH_VIEW_DIGEST_MISSING",
            "GRAPH_VIEW_DIGEST_MISMATCH",
            "UNMAPPED_LABEL",
            "UNMAPPED_RELATIONSHIP",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R038.5.1: an out-of-range search capability.
    ("RegisterForeignSource", &["SEARCH_CAPABILITY_INVALID"]),
    // MEDULLA-DURABLE-KERNEL-R876.4: the bounded neighborhood read's bound, cursor and visibility refusals.
    (
        "GetNeighborhood",
        &[
            "NEIGHBORHOOD_DEPTH_EXCEEDED",
            "NEIGHBORHOOD_LIMIT_REQUIRED",
            "NEIGHBORHOOD_LIMIT_EXCEEDED",
            "NEIGHBORHOOD_EXCLUDE_TOO_LARGE",
            "NEIGHBORHOOD_CURSOR_STALE",
            "NODE_NOT_FOUND",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R765: an invisible source and a non-owner share.
    ("GetForeignSource", &["SOURCE_NOT_FOUND"]),
    ("ProbeForeignSource", &["SOURCE_NOT_FOUND"]),
    ("ShareForeignSource", &["SOURCE_NOT_FOUND", "SOURCE_OWNER_REQUIRED"]),
    ("UnshareForeignSource", &["SOURCE_NOT_FOUND", "SOURCE_OWNER_REQUIRED"]),
    // MEDULLA-UNIFIED-DATA-PLANE-R020.2.2: a DuckDB file outside its allowlist is refused at
    // registration; a warehouse read refuses a kind the engine does not serve, an unreadable
    // source, a drifted schema and a denied file with these typed codes.
    ("RegisterForeignSource", &["DUCKDB_FILE_DENIED"]),
    (
        "QueryForeignColumns",
        &[
            "CAPABILITY_UNSUPPORTED",
            "SOURCE_UNAVAILABLE",
            "SCHEMA_DRIFT",
            "DUCKDB_FILE_DENIED",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R077.6.4: a pinned read of `information_schema.vector_advice`.
    (
        "Sql",
        &[
            "COLUMN_TYPE_UNSUPPORTED",
            "SCHEMA_REVISION_STALE",
            // MEDULLA-FEDERATED-QUERY-R077.5: a `NEAREST` read over a column vector binding.
            "COLUMN_NOT_BOUND",
            "SEARCH_PROBE_MODEL_MISMATCH",
            "QUERY_ENCODER_UNAVAILABLE",
        ],
    ),
    // MEDULLA-TYPED-PACKS-R809: an unparsable or oversized Turtle payload.
    ("OntologyCanonicalHash", &["TURTLE_INVALID"]),
    // MEDULLA-DURABLE-KERNEL-R078: the served edge chokepoint refuses an
    // inverted validity interval; invalidation and supersession keep their
    // closed-edge and missing-edge refusals.
    // MEDULLA-UNIFIED-DATA-PLANE-R030.4: a generic edge writer refuses an `owl:sameAs` edge.
    ("AddEdge", &["LINK_NOT_APPROVABLE", "VALIDITY_INVERTED"]),
    ("BatchUpdate", &["LINK_NOT_APPROVABLE"]),
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
        &[
            "SCOPE_EXCEEDS_POLICY",
            "SCOPE_POLICY_UNREADABLE",
            // MEDULLA-ORCH-VOCAB-R007.1: a `:precedes` cycle between steps.
            "STEP_ORDER_CYCLE",
            // MEDULLA-ORCH-VOCAB-R005.100: a stale step input schema.
            "SHIELD_SCHEMA_STALE",
        ],
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
            // A job id the caller does not own reads as absent.
            "JOB_NOT_FOUND",
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
    ("FeatureLedger", &["PARAMETER_INVALID", "REVISION_CONFLICT"]),
    // MEDULLA-DECISION-ENGINE-R702.3: an absent or unreadable graph, and a malformed correction.
    (
        "CorrectionRecord",
        &["CANDIDATE_PLAN_REFUSED", "PARAMETER_INVALID"],
    ),
    // MEDULLA-DECISION-ENGINE-R701.3: an absent or unreadable graph, and a topic with no gap.
    (
        "FailureIngest",
        &["CANDIDATE_PLAN_REFUSED", "PARAMETER_INVALID"],
    ),
    // MEDULLA-DECISION-ENGINE-R708.3: a malformed op, an unreadable graph and the flag refusals.
    (
        "KnowledgeFlag",
        &["CANDIDATE_PLAN_REFUSED", "PARAMETER_INVALID"],
    ),
    // MEDULLA-DECISION-ENGINE-R709.4: a malformed op, an unreadable graph and the curation refusals.
    (
        "KnowledgeCuration",
        &["CANDIDATE_PLAN_REFUSED", "PARAMETER_INVALID"],
    ),
    // MEDULLA-DECISION-ENGINE-R710.4: a malformed op, an unreadable graph and the friction refusals.
    (
        "FrictionReport",
        &["CANDIDATE_PLAN_REFUSED", "PARAMETER_INVALID"],
    ),
    // MEDULLA-DURABLE-KERNEL-R877.3: the bundle refusals, all before the target changes.
    ("BundleExport", &["BUNDLE_TOO_LARGE"]),
    (
        "BundleImport",
        &[
            "BUNDLE_DIGEST_MISMATCH",
            "BUNDLE_EMBEDDING_MODEL_UNKNOWN",
            "BUNDLE_GRAPH_EXISTS",
            "BUNDLE_TOO_LARGE",
            "BUNDLE_VERSION_UNSUPPORTED",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R713.4: a malformed op and an unreadable graph.
    (
        "SkillTelemetry",
        &["CANDIDATE_PLAN_REFUSED", "PARAMETER_INVALID"],
    ),
    ("GetRunWorkItem", &["RUN_ADMISSION_UNAVAILABLE"]),
    // MEDULLA-UNIFIED-DATA-PLANE-R040.4: a blocked embedding-model promotion.
    // MEDULLA-FEDERATED-QUERY-R039.2.2/R039.2.4: the chunk-encoding recipe the
    // worker runs refuses an unsupported position mode, an invalid overlap and
    // an empty or zero-norm aggregate.
    (
        "SemanticIndex",
        &[
            "PROMOTION_BLOCKED",
            "ENCODE_POSITION_MODE_UNSUPPORTED",
            "ENCODE_OVERLAP_INVALID",
            "NO_EMBEDDING",
            "ENCODE_DIM_MISMATCH",
            "ENCODE_NON_FINITE",
            "ENCODE_BUDGET_EXHAUSTED",
            "ENCODE_POLICY_STALE",
            "EMBED_BUDGET_EXHAUSTED",
            "EMBED_VECTOR_INVALID",
            "EMBED_PROVIDER_FAILED",
            // MEDULLA-FEDERATED-QUERY-R077.2: a SQL column source that is absent or cannot be embedded.
            "COLUMN_UNKNOWN",
            "COLUMN_TYPE_UNSUPPORTED",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R039.2.5/R039.2.6: the served chunk encoding and
    // the encoded-document search refuse with the chunk-encoding codes.
    (
        "EncodeDocument",
        &[
            "ENCODE_POSITION_MODE_UNSUPPORTED",
            "ENCODE_OVERLAP_INVALID",
            "NO_EMBEDDING",
            "ENCODE_DIM_MISMATCH",
            "ENCODE_NON_FINITE",
            "ENCODE_BUDGET_EXHAUSTED",
            "ENCODE_POLICY_STALE",
        ],
    ),
    ("EncodedDocumentSearch", &["ENCODE_POLICY_STALE"]),
    // MEDULLA-DECISION-ENGINE-R615.6: the write-back queue ops of DecisionLog.
    // MEDULLA-DECISION-ENGINE-R174.6: a generation cycle without its capacity lease.
    (
        "DecisionLog",
        &[
            "CAPACITY_UNAVAILABLE",
            "INVALID_WRITEBACK_IDENTIFIER",
            "INVALID_WRITEBACK_AS_OF",
            "WRITEBACK_DUPLICATE_EXTERNAL_ID",
            "WRITEBACK_TENANT_MISMATCH",
            "WRITEBACK_SOURCE_CONFLICT",
            "PROPOSAL_UNAPPROVED",
            "WRITEBACK_QUEUE_TRANSITION_REFUSED",
            "WRITEBACK_QUEUE_UNKNOWN",
            "WRITEBACK_QUEUE_EXISTS",
            "WRITEBACK_PROPOSAL_UNKNOWN",
            "WRITEBACK_BELOW_QUEUE_TIER",
            "WRITEBACK_QUEUE_TOO_LARGE",
            "WRITEBACK_SELF_APPROVAL",
            // MEDULLA-SDLC-R013: a scenario run record without a phase, scenario or commit.
            "SCENARIO_RUN_INCOMPLETE",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R763.2: column lineage derivation and impact refusals.
    (
        "ColumnLineage",
        &[
            "LINEAGE_SQL_TOO_LARGE",
            "LINEAGE_SQL_UNPARSEABLE",
            "LINEAGE_SQL_NOT_A_QUERY",
            "LINEAGE_DEPTH_INVALID",
            "LINEAGE_CATALOG_AMBIGUOUS",
            "LINEAGE_CATALOG_MISSING",
            "LINEAGE_IMPACT_TOO_LARGE",
            "SCHEMA_DRIFT",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R029.2: the table context of an attached source refuses with
    // the schema-context codes and the refusals of the probe and lineage reads it consumes.
    (
        "SchemaContext",
        &[
            "SCHEMA_CONTEXT_SOURCE_UNKNOWN",
            "SCHEMA_CONTEXT_UNKNOWN_TABLE",
            "SCHEMA_CONTEXT_UNMAPPED",
            "SCHEMA_CONTEXT_CLASS_AMBIGUOUS",
            "SCHEMA_DRIFT",
            "SOURCE_UNAVAILABLE",
            "LINEAGE_IMPACT_TOO_LARGE",
        ],
    ),
    // Local-only authority: a clustered server refuses it with the typed
    // `LOCAL_ONLY_CLUSTER_REFUSAL` (server-mutation) instead of proposing it.
    ("SqlSourceBatch", &["CLUSTER_MUTATION_UNAVAILABLE"]),
    // MEDULLA-TYPED-PACKS-R041: a caller-minted pack-reserved component id.
    (
        "AgentComponent",
        &[
            "BLUEPRINT_IDENTITY_CHANGED",
            "BLUEPRINT_MALFORMED",
            "BODY_DIGEST_MISMATCH",
            "COMPONENT_REVISION_UNKNOWN",
            "COMPONENT_WITHDRAWN",
            "LIFECYCLE_RETIRED_TERMINAL",
            "LIFECYCLE_STATE_MALFORMED",
            "LIFECYCLE_TRANSITION_INVALID",
            "MALFORMED_A2A_AGENT_CARD",
            "MALFORMED_BODY",
            "MALFORMED_GUARDRAIL_RULE",
            "RECEIPT_DIGEST_INVALID",
            "RECEIPT_DIGEST_REUSED",
            "RECEIPT_REVISION_UNKNOWN",
            "RESERVED_COMPONENT_ID",
            "SKILL_BREAKING_NO_MIGRATION",
            "SKILL_CONSUMER_CONTRACT_CHANGED",
            "SKILL_MIGRATION_RECEIPT_INVALID",
            // MEDULLA-TYPED-PACKS-R100.100: the lossless skill edit refusals.
            "SKILL_PATCH_ANCHOR_INVALID",
            "SKILL_PATCH_OVERLAP",
            "SKILL_SOURCE_STALE",
            "SKILL_SPAN_UNKNOWN",
            "WITHDRAWN_NOT_ALLOWED",
        ],
    ),
    (
        "GuardrailEntailment",
        &["GUARDRAIL_SCOPE_TOO_LARGE", "MALFORMED_GUARDRAIL_RULE"],
    ),
    // MEDULLA-DURABLE-KERNEL-R074.2.
    (
        "WorkerRegister",
        &[
            "WORKER_ALREADY_REGISTERED",
            "WORKER_REGISTRATION_INVALID",
            "WORKER_TTL_INVALID",
            "WORKER_CLOCK_REGRESSION",
        ],
    ),
    (
        "WorkerHeartbeat",
        &[
            "WORKER_UNKNOWN",
            "WORKER_EXPIRED",
            "WORKER_TTL_INVALID",
            "WORKER_CLOCK_REGRESSION",
        ],
    ),
    // `ChangeEnvelope::validate` refuses carried engine-internal/native operations.
    (
        "ApplyChangeEnvelope",
        &[
            "CARRIER_INNER_METHOD_REFUSED",
            "CHANGE_ENVELOPE_CONTEXT_MISMATCH",
            // MEDULLA-REPO-INGEST-R002.653.
            "ENVELOPE_SOURCE_MISSING",
            "SUPERSESSION_UNKNOWN_FACT",
            // MEDULLA-REPO-INGEST-R002.651.2.
            "SUPERSESSION_UNPAIRED",
        ],
    ),
    (
        "ApplyChangeEnvelopeDraft",
        &[
            "CARRIER_INNER_METHOD_REFUSED",
            "ENVELOPE_SOURCE_MISSING",
            "SUPERSESSION_UNKNOWN_FACT",
            // MEDULLA-REPO-INGEST-R002.651.2.
            "SUPERSESSION_UNPAIRED",
        ],
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
        &["DATA_PREP_DECLARATION_INVALID", "INGEST_TENANT_MISMATCH"],
    ),
    // MEDULLA-CQ-XSRC-R008.4.
    (
        "CrossSourceQuestion",
        &["CQ_XSRC_ITEM_WITHOUT_SOURCE", "CQ_XSRC_UNKNOWN_QUESTION"],
    ),
    // MEDULLA-CQ-XSRC-R011.
    (
        "SupplyChainImpact",
        &["CQ_CLASS_UNKNOWN", "CQ_XSRC_ITEM_WITHOUT_SOURCE"],
    ),
    // MEDULLA-REPO-INGEST-R002.100.
    (
        "VerifyIngestReadback",
        &[
            "HYDRATION_BINDING_MISMATCH",
            "READBACK_DENIED",
            "READBACK_MISMATCH",
            "READBACK_PARTIAL",
            "READBACK_STALE",
        ],
    ),
    // MEDULLA-REPO-INGEST-R002.651.4.
    ("ProcessConformance", &["PROCESS_MODEL_UNKNOWN"]),
    // MEDULLA-SUPPLY-VOCAB-R009.3: ecosystem package risk propagation.
    (
        "MineRiskPropagation",
        &["ECOSYSTEM_DEPTH_OUT_OF_RANGE", "ECOSYSTEM_PACKAGE_UNKNOWN"],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R651.7.4 / MEDULLA-DECISION-ENGINE-R802.4.
    (
        "EnrichDocument",
        &[
            "ALIGNMENT_AMBIGUOUS",
            "ALIGNMENT_NO_MAPPING",
            "CHUNK_CONFIG_INVALID",
            "CONNECTOR_SCHEMA_MAPPING_AMBIGUOUS",
            "CONNECTOR_SCHEMA_MAPPING_INVALID",
            "CONNECTOR_SCHEMA_MAPPING_UNAVAILABLE",
            "DOCUMENT_NOT_FOUND",
            "DOCUMENT_PROPERTIES_INVALID",
            "DOCUMENT_TEXT_MISSING",
            "DOCUMENT_TOO_LARGE",
            "EMBEDDING_DIMENSION_MISMATCH",
            "EMBEDDING_UNAVAILABLE",
            "ENRICHMENT_UNAVAILABLE",
            "RUNG_OUTRANKED",
            "SECTION_TREE_INVALID",
            "STALE_MAPPING_REFERENCE",
            "UNKNOWN_MAPPING_REFERENCE",
            "WEB_BACKEND_INVALID",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R614.3 / R614.4: the scheduled enrichment surface.
    (
        "SemanticEnrich",
        &[
            "CARD_FACT_UNKNOWN",
            "CARD_UNGROUNDED",
            "DOCUMENT_NOT_FOUND",
            "EMBEDDING_UNAVAILABLE",
            "ENRICHMENT_NOT_SCHEDULED",
            "ENRICHMENT_UNAVAILABLE",
            "INVALID_CONFIDENCE",
            "INVALID_IDENTIFIER",
            "KNOB_OUT_OF_BOUNDS",
            "PROJECTION_RULE_UNKNOWN_TAXONOMY",
            "TAXONOMY_EMPTY",
            "VECTOR_DIMENSION_MISMATCH",
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
    // MEDULLA-DECISION-ENGINE-R651.3.
    ("MemoryRemember", &["ACCESS_DENIED", "PARAMETER_INVALID"]),
    // MEDULLA-DECISION-ENGINE-R656.3: a named document set narrows recall.
    (
        "MemoryRecall",
        &[
            "ACCESS_DENIED",
            "PARAMETER_INVALID",
            "DOCUMENT_SET_NOT_FOUND",
            "INVALID_ARGUMENT",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R656.3: a named document set narrows discovery.
    (
        "Discover",
        &[
            "DOCUMENT_SET_NOT_FOUND",
            "INVALID_ARGUMENT",
            // MEDULLA-FEDERATED-QUERY-R704.4: a scope that names no agent node or a foreign set.
            "SCOPE_AGENT_UNKNOWN",
            "SCOPE_SET_UNKNOWN",
            "RESOURCE_ACCESS_DENIED",
        ],
    ),
    ("MemoryExport", &["ACCESS_DENIED"]),
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
    // MEDULLA-INGEST-WORKER-R913.2: a graph with no admitted ontology refuses the slice.
    ("OntologySlice", &["ONTOLOGY_HIERARCHY_CYCLE", "ONTOLOGY_NOT_ADMITTED"]),
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
            "CHANGESET_NODE_CONFLICT",
            "INVALID_COMMIT_DIGEST",
            "LAYER_CYCLE",
            "REPOSITORY_BATCH_TOO_LARGE",
        ],
    ),
    // MEDULLA-DURABLE-KERNEL-R700.4: a retention policy the engine refuses to run.
    (
        "RetentionCleanupRun",
        &["RETENTION_HOLD_CHECK_REQUIRED", "RETENTION_POLICY_INVALID"],
    ),
    // MEDULLA-SRC-AUTH-R801: a policy source bind the review no longer matches.
    (
        "PolicySourceBind",
        &[
            "POLICY_SOURCE_DIGEST_MISMATCH",
            "POLICY_SOURCE_STALE",
            "POLICY_SOURCE_UNREVIEWED",
        ],
    ),
    // `analytics_pool_config` refuses an invalid scheduler policy
    // (MEDULLA-DURABLE-KERNEL-R082.9).
    ("AnalyticsJob", &["SCHEDULER_CONFIG_INVALID"]),
    // MEDULLA-FEDERATED-QUERY-R091.4: the plan-evolution refusals a downtime job's result carries.
    ("AnalyticsJob", &["PLAN_RESULT_MISMATCH", "PLAN_AUTHORITY_EXPIRED"]),
    // MEDULLA-DECISION-ENGINE-R144.4: the refusals of an indexless retrieval. A denied
    // document answers as an absent one; budget exhaustion reports the counts.
    (
        "IndexlessRetrieve",
        &[
            "COMPONENT_PIN_MISMATCH",
            "DOCUMENT_NOT_FOUND",
            "INDEXLESS_BUDGET_EXHAUSTED",
            "INDEXLESS_OPS_EMPTY",
            "INDEXLESS_SPAN_INVALID",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R100.100.4: the refusals of a context search. The kernel
    // codes pass through unchanged; a cursor is invalid when forged, edited or issued
    // for another tenant or query, and stale when the snapshot moved.
    (
        "RetrievalContextSearch",
        &[
            "COMPONENT_PIN_MISMATCH",
            "CONTEXT_CURSOR_INVALID",
            "CONTEXT_CURSOR_STALE",
            "CONTEXT_MAP_INVALID",
            "CONTEXT_QUERY_EMPTY",
            "CONTEXT_SPAN_TOO_LARGE",
            "POLICY_BODY_UNAVAILABLE",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R607.4: the refusals of a topology analytic. A graph over
    // `topology.max_nodes` / `topology.max_edges` is too large, a bad `k`, weight or
    // candidate set is an invalid parameter, and fewer than 3 nodes is trivial.
    (
        "MineTopology",
        &[
            "TOPOLOGY_GRAPH_TOO_LARGE",
            "TOPOLOGY_PARAM_INVALID",
            "TOPOLOGY_TRIVIAL_GRAPH",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R702.3: the refusals of a search-task synthesis. An unknown
    // or unreadable answer node is `SEARCH_TASK_INVALID`; a risky task is a typed rejection, not an error: it names its
    // detector under `SEARCH_TASK_SHORTCUT`.
    (
        "SynthesizeSearchTask",
        &[
            "SEARCH_TASK_INVALID",
            "SEARCH_TASK_SHORTCUT",
            "SEARCH_TASK_TOO_LARGE",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R650.4: the refusals of a context bundle compile; the
    // package codes pass through unchanged from the selection kernel.
    (
        "ContextCompile",
        &[
            "ACCESS_CHECK_UNAVAILABLE",
            "CONTEXT_BUDGET_MISSING",
            "CONTEXT_ITEM_COST_INVALID",
            "CONTEXT_PACKAGE_CERTIFICATE_UNACCEPTABLE",
            "CONTEXT_PACKAGE_INVALID_MEASUREMENT",
            "CONTEXT_PACKAGE_MISSING_FACT",
            "CONTEXT_PACKAGE_WORK_EXHAUSTED",
            // MEDULLA-FEDERATED-QUERY-R704.4: a scope that names no agent node or a foreign set.
            "SCOPE_AGENT_UNKNOWN",
            "SCOPE_SET_UNKNOWN",
            "RESOURCE_ACCESS_DENIED",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R081.11: a round refuses a malformed part and the
    // sufficiency step refuses a missing round limit with its typed code. The selection
    // step (R081.7) refuses a missing, stale or non-finite score pair, an invalid
    // normalization and an unapproved activation; the hierarchy step (R081.9, R081.9.7.4)
    // refuses a malformed tree and an exhausted limit; the arbitration step (R083.4) abstains
    // with both candidates and refuses a candidate that names no grounding.
    (
        "RetrievalRound",
        &[
            "ARBITRATION_ABSTAIN",
            "CANDIDATE_GROUNDING_REQUIRED",
            "RETRIEVAL_BUDGET_EXCEEDED",
            "RETRIEVAL_CONTRACT_INVALID",
            "RETRIEVAL_EMBEDDING_REVISION_MISMATCH",
            "RETRIEVAL_FEEDBACK_INVALID",
            "RETRIEVAL_HIERARCHY_INVALID",
            "RETRIEVAL_NORMALIZATION_INVALID",
            "RETRIEVAL_POLICY_UNVERIFIED",
            "RETRIEVAL_ROUND_LIMIT_REQUIRED",
            "RETRIEVAL_SCORE_PAIR_REQUIRED",
            "RETRIEVAL_SOLVER_NOT_CONVERGED",
            "RETRIEVAL_STATE_INVALID",
            "RETRIEVAL_STATE_STALE",
            "RETRIEVAL_STRATEGY_UNVERIFIED",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R081.103.3: the typed refusals of a viewport read.
    (
        "ViewportQuery",
        &[
            "ACCESS_DENIED",
            "BYTE_CEILING_TOO_SMALL",
            "INDEX_UNAVAILABLE",
            "STALE_SOURCE_REVISION",
            "VIEWPORT_CAP_EXCEEDED",
            "VIEWPORT_INVALID",
        ],
    ),
    (
        "ResolveViewportSelection",
        &[
            "ACCESS_DENIED",
            "INDEX_UNAVAILABLE",
            "STALE_SOURCE_REVISION",
            "VIEWPORT_INVALID",
        ],
    ),
    // MEDULLA-SRC-AUTH-R004.6.1: a request that names no class, too many, or a bad budget.
    (
        "SourceCoverage",
        &[
            "SOURCE_COVERAGE_HOP_BUDGET_INVALID",
            "SOURCE_COVERAGE_NO_CLASSES",
            "SOURCE_COVERAGE_TOO_MANY_CLASSES",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R653.2: a grep pattern past the regex size limits.
    // MEDULLA-FEDERATED-QUERY-R654.4: a walk past the depth cap, or an anchor or
    // symptom the index does not hold.
    (
        "CodeContext",
        &["CODE_DEPTH_EXCEEDED", "CODE_ANCHOR_UNKNOWN"],
    ),
    // MEDULLA-MEDIA-VOCAB-R001.102.4: the bookmark search and relations refusals.
    (
        "BookmarkSemanticSearch",
        &[
            "BOOKMARK_POLICY_INVALID",
            "BOOKMARK_LIMIT_EXCEEDED",
            "BOOKMARK_INDEX_STALE",
            "BOOKMARK_REVISION_MISMATCH",
            "BOOKMARK_RESPONSE_TOO_LARGE",
        ],
    ),
    (
        "BookmarkRelations",
        &[
            "BOOKMARK_POLICY_INVALID",
            "BOOKMARK_LIMIT_EXCEEDED",
            "BOOKMARK_INDEX_STALE",
            "BOOKMARK_REVISION_MISMATCH",
            "BOOKMARK_RESPONSE_TOO_LARGE",
        ],
    ),
    // MEDULLA-REPO-INGEST-R002.651.4: a conformance request for a model the graph does not hold.
    ("ProcessConformance", &["PROCESS_MODEL_UNKNOWN"]),
    // MEDULLA-REPO-INGEST-R700.3: a step budget outside its range.
    ("CodeHealthSweep", &["INVALID_ARGUMENT"]),
    ("Diagnose", &["CODE_DEPTH_EXCEEDED", "CODE_ANCHOR_UNKNOWN"]),
    (
        "CorpusRetrieve",
        &[
            "CORPUS_REGEX_TOO_COMPLEX",
            "CORPUS_LINEAGE_TOO_DEEP",
            "CORPUS_SCOPE_DENIED",
        ],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R018.3: mismatched factor record lengths.
    ("FinanceFactorRank", &["FACTOR_RECORD_INVALID"]),
    // MEDULLA-FINANCE-PRIMITIVES-R018.5: an empty provenance list or an unresolved key.
    ("FinanceFactorRecord", &["FACTOR_RECORD_INVALID"]),
    // MEDULLA-FINANCE-PRIMITIVES-R015.1.1 and R010.2: staging refusals.
    (
        "FinanceOrderStage",
        &[
            "PROPOSAL_HEAD_STALE",
            "PROPOSAL_REVISION_CONFLICT",
            "PROPOSAL_HASH_MISMATCH",
            "PROPOSAL_EVIDENCE_MISSING",
            "ORDER_LIMIT_BREACH",
            "TRADING_POLICY_MISSING",
        ],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R015.1.2 and R015.1.3: promotion refusals.
    (
        "FinanceOrderPromote",
        &[
            "TRADING_POLICY_MISSING",
            "ORDER_LIMIT_BREACH",
            "PROMOTE_GATE_UNAVAILABLE",
            "PROMOTE_GATE_FAILED",
            "PROMOTE_HEAD_STALE",
            "PROMOTE_EVIDENCE_MISMATCH",
            "PROMOTE_RECEIPT_INCOMPLETE",
            "ORDER_PROMOTION_VETOED",
            "PROMOTE_WRITE_FAILED",
        ],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R010.2: the limit verdict is a result, not a refusal.
    (
        "FinanceOrderLimitCheck",
        &["PROPOSAL_HASH_MISMATCH", "TRADING_POLICY_MISSING"],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R750.752: stable codes of the research kernels.
    (
        "FinancePatternClassify",
        &["PATTERN_WINDOW_SHORT", "PATTERN_INPUT_INVALID"],
    ),
    (
        "FinanceProfitAttribution",
        &["ATTRIBUTION_WEIGHTS_INVALID", "ATTRIBUTION_INPUT_INVALID"],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R013.2.6: a figure without provenance, an unsupported basis policy.
    (
        "FinanceAccountProject",
        &["FIGURE_PROVENANCE_MISSING", "BASIS_POLICY_UNSUPPORTED"],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R005.3.4: the served performance method.
    (
        "FinancePerformance",
        &[
            "FIGURE_PROVENANCE_MISSING",
            "RANGE_UNSUPPORTED",
            "BENCHMARK_UNKNOWN",
            "ACCOUNT_BINDING_UNVERIFIED",
        ],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R002.11: regime forecast, calibration gate and sizing input.
    (
        "RegimeForecast",
        &[
            "FORECAST_MODEL_STALE",
            "FORECAST_HORIZON_INVALID",
            "FORECAST_MATRIX_INVALID",
            "FORECAST_INPUT_SUPPLIED",
            "CALIBRATION_ROW_DUPLICATE",
            "CALIBRATION_ROW_MISSING",
            "CALIBRATION_ROW_MISMATCH",
            "CALIBRATION_FUTURE_LEAK",
            "CALIBRATION_SAMPLES_LOW",
            "CALIBRATION_POLICY_INVALID",
            "REGIME_MODEL_NOT_FOUND",
        ],
    ),
    (
        "RegimeSizingInput",
        &[
            "FORECAST_MODEL_STALE",
            "FORECAST_HORIZON_INVALID",
            "FORECAST_MATRIX_INVALID",
            "FORECAST_INPUT_SUPPLIED",
            "CALIBRATION_ROW_DUPLICATE",
            "CALIBRATION_ROW_MISSING",
            "CALIBRATION_ROW_MISMATCH",
            "CALIBRATION_FUTURE_LEAK",
            "CALIBRATION_SAMPLES_LOW",
            "CALIBRATION_POLICY_INVALID",
            "REGIME_MODEL_NOT_FOUND",
            "SIZING_ABSTAIN",
        ],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R902: a probabilistic forecast sample named as a kernel input.
    ("FinancePositionSize", &["PROBABILISTIC_INPUT_REFUSED"]),
    ("FinanceMarket", &["PROBABILISTIC_INPUT_REFUSED"]),
    // A batch commits through the change-envelope route, whose graph-version
    // fence refuses with WRITE_CONFLICT when another write moved the graph.
    // MEDULLA-INGEST-WORKER-R003.15, R003.16: the transcript refusals, all before any graph change.
    (
        "SourceIngest",
        &[
            // MEDULLA-DECISION-ENGINE-R869.2: the action record refusals.
            "ACTION_APPROVAL_REQUIRED",
            "ACTION_FACT_REVISION_UNKNOWN",
            "ACTION_IDEMPOTENCY_CONFLICT",
            "ACTION_RECORD_INVALID",
            "FORECAST_SAMPLE_INVALID",
            // MEDULLA-INGEST-WORKER-R002.9: the instance profile refusals.
            "PROFILE_DANGLING_CLASS",
            "PROFILE_TOO_LARGE",
            // MEDULLA-INGEST-WORKER-R002.7.
            "PROPOSAL_NOT_ADMITTED",
            // MEDULLA-INGEST-WORKER-R700.4: the reader refusals, each the rule id of a
            // quarantine with reason `NormalizationFailed`.
            "READER_CORRUPT",
            "READER_ENCRYPTED",
            "READER_TOO_LARGE",
            "READER_UNSUPPORTED_MEDIA_TYPE",
            // MEDULLA-TYPED-PACKS-R715: a delivery that breaks an approved contract under
            // the quarantine policy parks and is refused.
            "SCHEMA_DRIFT_QUARANTINED",
            "TRANSCRIPT_DUPLICATE_MESSAGE_CONFLICT",
            "TRANSCRIPT_FOREIGN_CONCEPT",
            "TRANSCRIPT_INVALID",
            "TRANSCRIPT_MISSING_EVENT_TIME",
            "TRANSCRIPT_UNKNOWN_FORMAT",
            "WRITE_CONFLICT",
        ],
    ),
    // MEDULLA-FINANCE-PRIMITIVES-R904: no current record for the binding in the caller's tenant.
    ("WalletKeyTrustGet", &["WALLET_KEY_TRUST_NOT_FOUND"]),
    // MEDULLA-FEDERATED-QUERY-R724: a put with bad bounds, an unregistered scope or
    // an unknown mapping.
    (
        "SchemaProjections",
        &["SCHEMA_BOUNDARY_INVALID", "BOUNDARY_MAPPING_UNKNOWN"],
    ),
    // MEDULLA-TYPED-PACKS-R097.700: the domain pack loader and run refusals.
    (
        "DomainPack",
        &[
            "DOMAIN_PACK_INTEGRITY_MISMATCH",
            "DOMAIN_PACK_INVALID",
            "DOMAIN_PACK_CASE_FAILED",
            "DOMAIN_PACK_MAPPING_INVALID",
            "DOMAIN_PACK_CLASS_UNKNOWN",
        ],
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
    // MEDULLA-BRANCH-MERGE-R006: the time-travel read refusals.
    (
        "BranchDiff",
        &[
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "BRANCH_VERSION_NOT_ON_CHAIN",
        ],
    ),
    (
        "BranchReadAt",
        &[
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "BRANCH_VERSION_NOT_ON_CHAIN",
        ],
    ),
    // MEDULLA-BRANCH-MERGE-R003: the merge refusals, all before any head moves.
    (
        "BranchMerge",
        &[
            "BRANCH_DELETED",
            "BRANCH_NO_COMMON_ANCESTOR",
            "BRANCH_PAYLOAD_UNREPLAYABLE",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "MERGE_ACTOR_UNKNOWN",
            "MERGE_ADOPTION_INVALID",
            "MERGE_TARGET_STALE",
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
    // MEDULLA-SCM-R013.4: the symbol-edit refusals and those of SubmitChangeset.
    (
        "SubmitSymbolEdit",
        &[
            "AGENT_RUN_TRACE_MISSING",
            "BLOB_DIGEST_MISMATCH",
            "BLOB_NOT_FOUND",
            "BLOB_TOO_LARGE",
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "CHANGESET_EMPTY",
            "CHANGESET_INVALID",
            "CHANGESET_NODE_CONFLICT",
            "CHANGESET_PARENT_NOT_ON_BRANCH",
            "CHANGESET_REQUIREMENT_MISSING",
            "EDIT_BASE_STALE",
            "EDIT_BLOB_UNREADABLE",
            "EDIT_NO_OPS",
            "EDIT_OPS_OVERLAP",
            "EDIT_ROUNDTRIP_MISMATCH",
            "EDIT_SYMBOL_UNKNOWN",
            "EDIT_UNPARSEABLE",
            "LIFECYCLE_ENDPOINT_UNKNOWN",
            "MERGE_BLOB_UNREADABLE",
            "PATH_READ_DENIED",
            "PATH_WRITE_DENIED",
            "RUN_TRACE_UNKNOWN",
            "SOURCE_CONFLICT",
        ],
    ),
    // MEDULLA-SCM-R002: the changeset refusals, all before the chain changes.
    (
        "SubmitChangeset",
        &[
            "AGENT_RUN_TRACE_MISSING",
            "BLOB_DIGEST_MISMATCH",
            "BLOB_NOT_FOUND",
            "BLOB_TOO_LARGE",
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "CHANGESET_EMPTY",
            "CHANGESET_INVALID",
            "CHANGESET_NODE_CONFLICT",
            "CHANGESET_PARENT_NOT_ON_BRANCH",
            "CHANGESET_REQUIREMENT_MISSING",
            "LIFECYCLE_ENDPOINT_UNKNOWN",
            "MERGE_BLOB_UNREADABLE",
            "PATH_WRITE_DENIED",
            "REQUIREMENT_EDIT_CONFLICT",
            // MEDULLA-SDLC-R017.4: a loop-authored spec edit without an approved proposal.
            "PROPOSAL_DIRECT_WRITE_REFUSED",
            "RUN_TRACE_UNKNOWN",
            "SOURCE_CONFLICT",
        ],
    ),
    // MEDULLA-SCM-R010: the git import refusals, each before the refused commit is written.
    (
        "ImportGitHistory",
        &[
            "BLOB_NOT_FOUND",
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            // MEDULLA-SDLC-R017.4: a loop-authored import that edits the spec tree.
            "PROPOSAL_DIRECT_WRITE_REFUSED",
            "CHANGESET_INVALID",
            "GIT_GRAPH_MALFORMED",
            "GIT_MODE_UNSUPPORTED",
            "PATH_WRITE_DENIED",
        ],
    ),
    // MEDULLA-SCM-R014: the git export refusals.
    (
        "GitExportPack",
        &[
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "GIT_EXPORT_TREE_READ_FAILED",
            "GIT_GRAPH_MALFORMED",
            "PATH_READ_DENIED",
            "REVISION_NOT_ON_CHAIN",
        ],
    ),
    // MEDULLA-SCM-R007: a path grant that is out of bounds.
    ("SetPathGrant", &["PATH_GRANT_INVALID"]),
    // MEDULLA-SCM-R004: the tree read refusals.
    (
        "ReadTree",
        &[
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "CHANGESET_INVALID",
            "PATH_READ_DENIED",
            "REVISION_NOT_ON_CHAIN",
        ],
    ),
    // MEDULLA-SCM-R005.1: the path-history read refusals.
    (
        "ReadPathHistory",
        &[
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "CHANGESET_INVALID",
            "HISTORY_INDEX_STALE",
            "PATH_READ_DENIED",
            "REVISION_NOT_ON_CHAIN",
        ],
    ),
    // MEDULLA-SCM-R009: the bisect refusals.
    (
        "BisectStep",
        &[
            "BISECT_VERDICT_COVERAGE",
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "INVALID_ARGUMENT",
            "PATH_READ_DENIED",
            "REVISION_NOT_ON_CHAIN",
        ],
    ),
    // MEDULLA-SCM-R005.2: the blame refusals and those of the path-history read.
    (
        "ReadBlame",
        &[
            "BLAME_BINARY",
            "BLAME_FILE_TOO_LARGE",
            "BLAME_PATH_ABSENT",
            "BRANCH_DELETED",
            "BRANCH_TENANT_DENIED",
            "BRANCH_UNKNOWN",
            "CHANGESET_INVALID",
            "HISTORY_INDEX_STALE",
            "PATH_READ_DENIED",
            "REVISION_NOT_ON_CHAIN",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R701.3: an unknown standard id, or a standard or asset node
    // that does not decode.
    (
        "StandardsDrift",
        &["STANDARD_NOT_FOUND", "STANDARDS_NODE_INVALID"],
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
    // MEDULLA-CQ-ORCH-R705.6/R705.8: a document the exchange reader refuses.
    (
        "ArchitectureModelImport",
        &[
            "ARCHIMATE_DOC_MALFORMED",
            "ARCHIMATE_DOC_ENTITY_REFUSED",
            "ARCHIMATE_DOC_TOO_LARGE",
            "ARCHIMATE_NAMESPACE_UNKNOWN",
            "ARCHIMATE_RELATIONSHIP_INVALID",
            "ARCHIMATE_VERSION_MISMATCH",
        ],
    ),
    // MEDULLA-CQ-ORCH-R700.3/R700.4: a process the compile refuses, or a stale expected version.
    (
        "ProcessCompile",
        &[
            "PROCESS_UNKNOWN",
            "PROCESS_TASK_UNBOUND",
            "PROCESS_CYCLE_WITHOUT_GATE",
            "PROCESS_TOO_LARGE",
            "PROCESS_IMPORT_UNMAPPED_ELEMENT",
            "PROCESS_IMPORT_NOT_CANONICAL",
            "PROCESS_IMPORT_DIGEST_MISMATCH",
            "PROCESS_IMPORT_UNKNOWN_SOURCE",
            "PROCESS_IMPORT_UNKNOWN_MODEL",
            "PROCESS_IMPORT_TOO_LARGE",
            "WORKFLOW_VERSION_CONFLICT",
            "PARAMETER_INVALID",
        ],
    ),
    // MEDULLA-CQ-ORCH-R701.3/R701.4: a canonical document the import refuses, or a stale version.
    (
        "ProcessModelImport",
        &[
            "PROCESS_IMPORT_UNMAPPED_ELEMENT",
            "PROCESS_IMPORT_NOT_CANONICAL",
            "PROCESS_IMPORT_DIGEST_MISMATCH",
            "PROCESS_IMPORT_UNKNOWN_SOURCE",
            "PROCESS_IMPORT_TOO_LARGE",
            "WORKFLOW_VERSION_CONFLICT",
            "PARAMETER_INVALID",
        ],
    ),
    // MEDULLA-CQ-ORCH-R702.9/R702.12: a stale model version and the reader's refusals.
    (
        "ProcessDocumentImport",
        &[
            "WORKFLOW_VERSION_CONFLICT",
            "PARAMETER_INVALID",
            "PROCESS_DOC_MALFORMED",
            "PROCESS_DOC_ENTITY_REFUSED",
            "PROCESS_DOC_TOO_LARGE",
            "PROCESS_DOC_NAMESPACE_UNKNOWN",
            "PROCESS_DOC_RULE_ENTRIES_MISMATCH",
        ],
    ),
    // MEDULLA-ORCH-VOCAB-R009.101.1: a manifest the engine does not read, content that differs from
    // the index or from the run trace, a run trace the tenant does not hold.
    (
        "IndexHarnessSessionBatch",
        &[
            "SESSION_MANIFEST_UNSUPPORTED",
            "SESSION_CONTENT_CHANGED",
            "SESSION_TRACE_UNKNOWN",
        ],
    ),
    // MEDULLA-ORCH-VOCAB-R009.100: a declared span the walk did not reach, a span reached twice,
    // a token or pinned revision cut from another snapshot, an invisible root run.
    (
        "RunTraceExport",
        &[
            "EXPORT_CHILD_MISSING",
            "EXPORT_SPAN_DUPLICATE",
            "EXPORT_CURSOR_STALE",
            "RUN_TRACE_NOT_FOUND",
        ],
    ),
    // MEDULLA-CQ-ORCH-R702.10: an invisible model or workflow, a source the writer cannot write.
    (
        "ProcessDocumentExport",
        &[
            "PROCESS_IMPORT_UNKNOWN_MODEL",
            "PROCESS_EXPORT_NOT_DEPLOYABLE",
            "PARAMETER_INVALID",
        ],
    ),
    // MEDULLA-CQ-ORCH-R703.3: an invisible model, a model the writer cannot write.
    (
        "ProcessModelDeploy",
        &[
            "PROCESS_SYNC_SOURCE_NOT_SECONDARY",
            "SYNC_DIRECTION_REFUSED",
            "PROCESS_IMPORT_UNKNOWN_MODEL",
            "PROCESS_EXPORT_NOT_DEPLOYABLE",
            "PARAMETER_INVALID",
        ],
    ),
    // MEDULLA-CQ-ORCH-R701.3/R701.4: an invisible workflow, a lossy strict export, an undeployable one.
    (
        "ProcessModelExport",
        &[
            "PROCESS_IMPORT_UNKNOWN_MODEL",
            "PROCESS_EXPORT_LOSSY",
            "PROCESS_EXPORT_NOT_DEPLOYABLE",
            "PARAMETER_INVALID",
        ],
    ),
    // MEDULLA-SRC-AUTH-R012.7: a sync the authority rows or the adapter table refuse.
    (
        "SourceSync",
        &[
            "SYNC_SOURCE_NOT_SECONDARY",
            "PROCESS_SYNC_SOURCE_NOT_SECONDARY",
            "SYNC_NO_SYSTEM_OF_RECORD",
            "SYNC_ADAPTER_MISSING",
            "SYNC_DIRECTION_REFUSED",
        ],
    ),
    // MEDULLA-CQ-ORCH-R010.3/R010.4: a view the derivation refuses.
    (
        "ArchitectureView",
        &[
            "VIEW_NO_SUPPORTING_SOURCE",
            "VIEW_UNKNOWN_REPOSITORY",
            "VIEW_STALE_REVISION",
            "VIEW_TOO_LARGE",
        ],
    ),
    // MEDULLA-INGEST-WORKER-R003.3: a blank query or an unverified tenant.
    ("AtomContext", &["ACCESS_DENIED", "PARAMETER_INVALID"]),
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
    // MEDULLA-UNIFIED-DATA-PLANE-R026.4.1: a projection read of an unapproved or stale mapping,
    // or one the verified caller may not make.
    (
        "ProjectionRead",
        &[
            "PROJECTION_MAPPING_UNAPPROVED",
            "PROJECTION_MAPPING_STALE",
            "PROJECTION_ACCESS_DENIED",
        ],
    ),
    // MEDULLA-DURABLE-KERNEL-R043.100.3: a cursor the feed cannot honour.
    (
        "ReadGraphChanges",
        &[
            "AUTHORIZATION_CHANGED",
            "CURSOR_EXPIRED",
            "GRAPH_INCARNATION_STALE",
        ],
    ),
    // MEDULLA-DURABLE-KERNEL-R038.3: a namespace declaration the engine refuses.
    (
        "KvDeclareNamespace",
        &["DURABILITY_CLASS_MISMATCH", "DURABILITY_CLASS_UNKNOWN"],
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
    // MEDULLA-DECISION-ENGINE-R817: the revision proposal list's refusal.
    ("RevisionProposalList", &["PROPOSAL_CURSOR_INVALID"]),
    // MEDULLA-DECISION-ENGINE-R173.7: the harness edit lifecycle's refusals (the machine's
    // own `TRANSITION_REFUSED` is added through `MACHINE_METHODS`).
    (
        "HarnessEdit",
        &[
            "HARNESS_EDIT_EXISTS",
            "HARNESS_EDIT_STATUS_INVALID",
            "HARNESS_EDIT_UNKNOWN",
            "PARAMETER_INVALID",
            "REVISION_CONFLICT",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R816: the observation state store's refusals.
    (
        "ObservationState",
        &[
            "PARAMETER_INVALID",
            "VISUAL_STATE_CROSS_TENANT",
            "VISUAL_STATE_STALE",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R029.2: the schema context answer's refusals.
    (
        "SchemaContext",
        &[
            "SCHEMA_CONTEXT_SOURCE_UNKNOWN",
            "SCHEMA_CONTEXT_TABLE_UNKNOWN",
            "SCHEMA_CONTEXT_UNMAPPED",
            "SCHEMA_CONTEXT_CLASS_AMBIGUOUS",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R764.8: the catalog usage answer's refusals.
    (
        "CatalogUsage",
        &["CATALOG_ASSET_NOT_FOUND", "CATALOG_LIMIT_INVALID"],
    ),
    // MEDULLA-TYPED-PACKS-R108.17: the catalog read refusals.
    (
        "CatalogRead",
        &[
            "ASSET_NOT_FOUND",
            "INVALID_ARGUMENT",
            "CATALOG_QUERY_TOO_LONG",
            "CATALOG_QUERY_EMPTY",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R617.9: the participant surface's refusals.
    (
        "Participant",
        &[
            "PARTICIPANT_KIND_INVALID",
            "MEMBERSHIP_SESSION_UNKNOWN",
            "PARTICIPANT_NOT_VISIBLE",
            "OBSERVER_NOT_PERMITTED",
            "SCOPE_NOT_VISIBLE",
            "SCOPE_SESSION_LIMIT",
            "DERIVE_ORDER_HELD",
            "DERIVE_CONCLUSION_LIMIT",
            "DERIVE_PAYLOAD_NOT_FOUND",
            "DERIVATION_COUNT_MISMATCH",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R655.5: the participant reads' refusals.
    (
        "ParticipantRead",
        &[
            "PARTICIPANT_NOT_VISIBLE",
            "OBSERVER_NOT_PERMITTED",
            "NO_GROUNDED_CLAIMS",
            "CARD_ABSENT",
            "SCOPE_NOT_FOUND",
            "SCOPE_NOT_VISIBLE",
            "SCOPE_SESSION_LIMIT",
            "DERIVE_ORDER_HELD",
            "DERIVE_PAYLOAD_NOT_FOUND",
            "INVALID_ARGUMENT",
            "ENGINE_UNAVAILABLE",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R656.2: the assistant profile store's refusals.
    (
        "AssistantProfile",
        &[
            "PROFILE_NOT_FOUND",
            "PROFILE_NOT_VISIBLE",
            "PROFILE_REVISION_CONFLICT",
            "DOCUMENT_SET_NOT_FOUND",
            "DOCUMENT_SET_QUERY_NOT_READ_ONLY",
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
    // MEDULLA-FINANCE-PRIMITIVES-R011.4, R011.5: an empty receipt idempotency key, or a new
    // receipt that does not start due at attempt 1.
    ("CreateNodeIfAbsent", &["RECEIPT_KEY_INVALID", "RECEIPT_STATE_INVALID"]),
    // MEDULLA-FINANCE-PRIMITIVES-R011.5: the receipt transition's refusals (its machine's own
    // `TRANSITION_REFUSED` is added through `MACHINE_METHODS`).
    (
        "FinanceReceiptTransition",
        &["RECEIPT_NOT_FOUND", "RECEIPT_STATE_INVALID"],
    ),

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
    (
        "CypherQuery",
        &[
            "REPLAY_NONCE_CONSUMED",
            "VIRTUAL_VIEW_READ_ONLY",
            "UNMAPPED_LABEL",
            "PATH_DEPTH_EXCEEDED",
            "VIEW_CLAUSE_UNSUPPORTED",
            "VIEW_JOIN_INVALID",
            "UNKNOWN_VIEW",
        ],
    ),
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
            // MEDULLA-UNIFIED-DATA-PLANE-R766: a kind outside the closed registry.
            "WORK_ITEM_KIND_UNKNOWN",
            "WORK_QUEUE_FULL",
            // MEDULLA-TYPED-PACKS-R108.6: the workflow a work item names is read from the
            // workflow registry before the row is written.
            "WORKFLOW_IDENTITY_UNKNOWN",
            "WORKFLOW_UNAUTHORIZED",
        ],
    ),
    // MEDULLA-TYPED-PACKS-R108.6: a batch names registered workflows exactly as one item does.
    (
        "SubmitWorkItems",
        &["WORKFLOW_IDENTITY_UNKNOWN", "WORKFLOW_UNAUTHORIZED"],
    ),
    ("ClaimWorkItem", &["REPLAY_NONCE_CONSUMED"]),
    // MEDULLA-DURABLE-KERNEL-R028.1.2.2: a checkpoint names only existing memory revisions.
    (
        "PublishRunCheckpoint",
        &[
            "CHECKPOINT_DIGEST_MISMATCH",
            "CHECKPOINT_MEMORY_REF_MISSING",
            "REPLAY_NONCE_CONSUMED",
        ],
    ),
    // MEDULLA-DURABLE-KERNEL-R028.1.2.3: a stale holder gets the fence refusal, never the head.
    ("GetRunCheckpoint", &["CHECKPOINT_FENCE_STALE", "CONFLICT"]),
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
    // MEDULLA-DECISION-ENGINE-R133.8: a terminal commit whose run trace receipt is invalid.
    (
        "CommitWorkItemResult",
        &[
            "RUN_TRACE_INVALID",
            "TRACE_INCOMPLETE_ANCESTRY",
            "TRACE_CYCLE",
            "TRACE_MISSING_DIGEST",
        ],
    ),
    // MEDULLA-DECISION-ENGINE-R133.7.3: the typed refusals of the authorized trace critique read.
    (
        "TraceCritique",
        &[
            "TRACE_PROJECTION_UNAUTHORIZED",
            "TRACE_STALE_CURSOR",
            "TRACE_SNAPSHOT_MISMATCH",
            "TRACE_INCOMPLETE_ANCESTRY",
            "TRACE_CYCLE",
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
    (
        "RunTemporalRules",
        &[
            "DERIVED_BOUND_MALFORMED",
            "DERIVED_ORIGIN_MISSING",
            "DERIVED_WINDOW_CONFLICT",
            "DERIVED_WRITE_UNAUTHORIZED",
            "RULE_VERSION_UNSUPPORTED",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R053.100 / R053.700 / R053.701: foreign Cypher,
    // Trino and Spark refusals on the served `Uql` FOREIGN path.
    (
        "Uql",
        &[
            // MEDULLA-FEDERATED-QUERY-R706.4: a policy names a tag with no registered marking.
            "POLICY_TAG_UNKNOWN",
            // MEDULLA-FEDERATED-QUERY-R706.10: a shape policy that cannot be read or evaluated,
            // and a result node a `refuse` shape policy rejects.
            "POLICY_SHAPE_INVALID",
            "POLICY_VIOLATION",
            "CYPHER_KEY_INVALID",
            "FENCE_FIELD_MISSING",
            "ONTOLOGY_REVISION_STALE",
            "PRESET_UNAVAILABLE",
            "SHAPE_FATAL_FINDING",
            "SHAPE_UNKNOWN",
            "SNAPSHOT_UNKNOWN",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R053.701: the job-form open refuses with the same codes.
    (
        "ForeignReadOpen",
        &[
            "FENCE_FIELD_MISSING",
            "PRESET_UNAVAILABLE",
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
    // MEDULLA-DURABLE-KERNEL-R094.7 / R094.7.1: concept ids and lineage edges.
    (
        "ConceptLineage",
        &[
            "CONCEPT_ID_MALFORMED",
            "CONCEPT_ID_PATH_COLLISION",
            "CONCEPT_LINEAGE_CYCLE",
            "CONCEPT_LINEAGE_SELF_EDGE",
            "CONCEPT_LINEAGE_UNKNOWN_ID",
        ],
    ),
    // MEDULLA-UNIFIED-DATA-PLANE-R702.6.
    ("LineageIngest", &["LINEAGE_EVENT_MALFORMED"]),
    // MEDULLA-ENGINE-PARITY-R019.2: an engine record class is never placed in an outside source.
    (
        "WriteBack",
        &["CORRUPT_WRITE_BACK", "EXTERNAL_PLACEMENT_FORBIDDEN"],
    ),
    ("TxnPlanWriteback", &["CORRUPT_WRITE_BACK"]),
    ("ActionRegister", &["ACTION_SPEC_INVALID"]),
    // MEDULLA-DURABLE-KERNEL-R072: the served task-queue refusals.
    (
        "TaskQueueEnqueue",
        &["TASK_DUPLICATE", "TASK_QUEUE_NAME_BLANK"],
    ),
    // MEDULLA-INGEST-WORKER-R001.4.1: the served ingest enqueue refusals.
    (
        "IngestEnqueue",
        &[
            "INGEST_JOB_INVALID",
            "INGEST_QUEUE_FULL",
            "INGEST_TENANT_MISMATCH",
        ],
    ),
    // MEDULLA-INGEST-WORKER-R605.2, R605.5: the served drain refusals.
    ("StartSourceDrain", &["INVALID_ARGUMENT"]),
    ("SourceDrainStatus", &["DRAIN_UNKNOWN"]),
    // MEDULLA-INGEST-WORKER-R604.3, R604.4: the reserved-worker admission of a claim.
    (
        "TaskQueueClaim",
        &[
            "TASK_LEASE_ZERO",
            "WORKER_UNKNOWN",
            "WORKER_CAPACITY_ZERO",
            "ADMIT_HELD_FOR_RESERVE",
        ],
    ),
    // MEDULLA-DURABLE-KERNEL-R072.1: a renewal refused for its lease.
    (
        "TaskQueueRenew",
        &[
            "TASK_ACK_LEASE_EXPIRED",
            "TASK_ACK_NOT_CLAIMED",
            "TASK_LEASE_ZERO",
        ],
    ),
    // MEDULLA-REPO-INGEST-R002.651.3: the dead-letter refusals.
    ("DeadLetterList", &["DEAD_LETTER_REQUEST_INVALID"]),
    (
        "DeadLetterRedrive",
        &["DEAD_LETTER_REQUEST_INVALID", "DEAD_LETTER_UNKNOWN_ITEM"],
    ),
    // MEDULLA-DURABLE-KERNEL-R076.2: a refused scheduler setting.
    ("SetSchedulerConfig", &["SCHEDULER_CONFIG_INVALID"]),
    (
        "TaskQueueAck",
        &[
            "DRAIN_CURSOR_STALE",
            "TASK_ACK_LEASE_EXPIRED",
            "TASK_ACK_NOT_CLAIMED",
        ],
    ),
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
    // MEDULLA-DECISION-ENGINE-R801.4/R801.8: the usage ledger's refusals.
    (
        "Usage",
        &[
            "USAGE_UNKNOWN_TENANT",
            "USAGE_NEGATIVE_COUNT",
            "USAGE_CACHED_EXCEEDS_INPUT",
            "USAGE_BATCH_TOO_LARGE",
            "USAGE_BAD_DIGEST",
            "USAGE_STORE_FAILED",
            "PRICE_WINDOW_OVERLAP",
            "PARAMETER_INVALID",
        ],
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
    // MEDULLA-SDLC-R002.7, R002.8: the deploy record write and its reads.
    (
        "DeployRecordPut",
        &[
            "TENANT_SCOPE_REQUIRED",
            "DEPLOY_RECORD_INVALID",
            "DEPLOY_RECORD_SEALED",
        ],
    ),
    (
        "DeployRecordGet",
        &["TENANT_SCOPE_REQUIRED", "DEPLOY_RECORD_NOT_FOUND"],
    ),
    (
        "DeployRecordList",
        &["TENANT_SCOPE_REQUIRED", "DEPLOY_RECORD_INVALID"],
    ),
    // MEDULLA-BUSINESS-OPS-R004: the approval gate question.
    (
        "ApprovalGateResolve",
        &["TENANT_SCOPE_REQUIRED", "APPROVAL_GATE_INVALID"],
    ),
    // MEDULLA-SCM-R008: the gate verdict record's refusals.
    (
        "RecordGateVerdict",
        &[
            "GATE_INPUT_DIGEST_MISMATCH",
            "GATE_VERDICT_SEALED",
            "LIFECYCLE_ENDPOINT_UNKNOWN",
        ],
    ),
    // MEDULLA-SDLC-R017.1: the served goal record's refusal.
    ("GoalPut", &["GOAL_TARGET_REQUIRED"]),
    // MEDULLA-SDLC-R017.3: the proposal record's refusals.
    (
        "ProposalPut",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INVALID_ARGUMENT",
            "PROPOSAL_EVIDENCE_REQUIRED",
            "PROPOSAL_EVIDENCE_UNRESOLVED",
            "PROPOSAL_EVIDENCE_DIGEST_MISMATCH",
            "MODE_REFERENCE_NO_PROPOSALS",
            "MODE_ORIGIN_NOT_ADMITTED",
            // MEDULLA-SDLC-R016.2: a research entry without its cited sources.
            "RESEARCH_ENTRY_UNCITED",
            // MEDULLA-SDLC-R017.4: a proposal whose target is a spec file path.
            "PROPOSAL_DIRECT_WRITE_REFUSED",
            // MEDULLA-SDLC-R016.4: a cited source read with a tool no contract names.
            "RESEARCH_TOOL_UNKNOWN",
        ],
    ),
    // MEDULLA-SDLC-R017.6: the proposal review list and decision.
    ("ProposalList", &["TENANT_SCOPE_REQUIRED"]),
    // MEDULLA-SDLC-R014.6: the competency questions' refusals.
    (
        "SpecQuestion",
        &[
            "TENANT_SCOPE_REQUIRED",
            "PARAMETER_INVALID",
            "COMPETENCY_SUBJECT_UNKNOWN",
            // MEDULLA-SDLC-R016.1: `plan_unknowns` on a reference codebase.
            "MODE_REFERENCE_NO_PROPOSALS",
        ],
    ),
    (
        "LandingPolicyPut",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INVALID_ARGUMENT",
            "LANDING_POLICY_APPROVAL_REQUIRED",
        ],
    ),
    (
        "FileDefect",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INVALID_ARGUMENT",
        ],
    ),
    (
        "TransitionDefect",
        &[
            "TENANT_SCOPE_REQUIRED",
            "LIFECYCLE_ENDPOINT_UNKNOWN",
            "DEFECT_TRANSITION_ILLEGAL",
            "DEFECT_UNVERIFIED",
        ],
    ),
    // MEDULLA-ORCH-VOCAB-R810: the observation write refusals.
    (
        "PutObservation",
        &[
            "TENANT_SCOPE_REQUIRED",
            "OBSERVATION_INVALID",
            "OBSERVATION_SCOPE_UNKNOWN",
            "OBSERVATION_CONFLICT",
        ],
    ),
    // MEDULLA-SDLC-R005.3 and R005.4: the served status refusals.
    // MEDULLA-SDLC-R019.14: the served delivery methods.
    // MEDULLA-SDLC-R004.3 and R004.4: the defect triage refusals.
    (
        "TriageDefect",
        &[
            "TENANT_SCOPE_REQUIRED",
            "DEFECT_UNKNOWN",
            "DEFECT_CLOSED",
            "PARAMETER_INVALID",
            "ACCESS_DENIED",
        ],
    ),
    (
        "DeliveryOpen",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INVALID_ARGUMENT",
            "DELIVERY_SCOPE_EMPTY",
            "MODE_REFERENCE_NO_PROPOSALS",
        ],
    ),
    (
        "DeliveryFeedback",
        &[
            "TENANT_SCOPE_REQUIRED",
            "FEEDBACK_SOURCE_UNKNOWN",
            "FEEDBACK_PROPOSAL_UNKNOWN",
            "FEEDBACK_EVENT_INVALID",
        ],
    ),
    (
        "DeliveryReport",
        &[
            "TENANT_SCOPE_REQUIRED",
            "DELIVERY_NOT_FOUND",
            "ASSIGNMENT_UNKNOWN",
            "ASSIGNMENT_ENDED",
            "REPORT_EVIDENCE_MISSING",
            "QUESTION_TOO_LONG",
        ],
    ),
    (
        "DeliveryStatus",
        &[
            "TENANT_SCOPE_REQUIRED",
            "DELIVERY_NOT_FOUND",
        ],
    ),
    (
        "DeliveryList",
        &[
            "TENANT_SCOPE_REQUIRED",
        ],
    ),
    (
        "DeliveryStop",
        &[
            "TENANT_SCOPE_REQUIRED",
            "DELIVERY_NOT_FOUND",
        ],
    ),
    // MEDULLA-TYPED-PACKS-R806: the served schema drift methods.
    (
        "SchemaDriftList",
        &["TENANT_SCOPE_REQUIRED"],
    ),
    (
        "SchemaDriftGet",
        &["TENANT_SCOPE_REQUIRED", "DRIFT_NOT_FOUND"],
    ),
    (
        "SchemaRepairPropose",
        &[
            "TENANT_SCOPE_REQUIRED",
            "DRIFT_NOT_FOUND",
            "SCHEMA_REPAIR_SHADOW_VIOLATIONS",
        ],
    ),
    (
        "SchemaRepairApprove",
        &[
            "TENANT_SCOPE_REQUIRED",
            "DRIFT_NOT_FOUND",
            "REPAIR_APPROVAL_MISSING",
            "REPAIR_SELF_APPROVED",
            "REPAIR_DIGEST_MISMATCH",
            "SCHEMA_REPAIR_SHADOW_MISSING",
            "SCHEMA_REPAIR_SHADOW_VIOLATIONS",
        ],
    ),
    (
        "DeliveryRetry",
        &[
            "TENANT_SCOPE_REQUIRED",
            "DELIVERY_NOT_FOUND",
            "INVALID_ARGUMENT",
        ],
    ),
    (
        "LifecycleTrace",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INVALID_ARGUMENT",
            "LIFECYCLE_DEPTH_INVALID",
            "LIFECYCLE_KIND_UNKNOWN",
        ],
    ),
    (
        "SpecStatus",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INVALID_ARGUMENT",
            "SPEC_STATUS_REV_UNKNOWN",
            "SPEC_TREE_ABSENT",
        ],
    ),
    (
        "GetDefect",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INVALID_ARGUMENT",
            "LIFECYCLE_ENDPOINT_UNKNOWN",
        ],
    ),
    (
        "ProposalDecide",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INVALID_ARGUMENT",
            "PROPOSAL_NOT_FOUND",
            "PROPOSAL_NOT_OPEN",
        ],
    ),
    // MEDULLA-IDENTITY-R740: the layered configuration read's refusals.
    ("ConfigLayers", &["TENANT_SCOPE_REQUIRED"]),
    // MEDULLA-SDLC-R012.9: the project mode read's refusals.
    (
        "ProjectModeGet",
        &["TENANT_SCOPE_REQUIRED", "INVALID_ARGUMENT"],
    ),
    // MEDULLA-SDLC-R012.8: the project mode record's refusals.
    (
        "ProjectModeSet",
        &[
            "TENANT_SCOPE_REQUIRED",
            "INVALID_ARGUMENT",
            "MODE_APPROVAL_REQUIRED",
        ],
    ),
    (
        "IncidentSetStatus",
        &["TENANT_SCOPE_REQUIRED", "INCIDENT_NOT_FOUND"],
    ),
    (
        "Glossary",
        &[
            "TENANT_SCOPE_REQUIRED",
            "GLOSSARY_CYCLE",
            "GLOSSARY_LABEL_TAKEN",
            "GLOSSARY_UNREVIEWED",
            "STEWARD_ROLE_MISSING",
            "PROPOSAL_INPUTS_MISSING",
        ],
    ),
    (
        "Stewardship",
        &[
            "TENANT_SCOPE_REQUIRED",
            "STEWARD_ROLE_MISSING",
            "CHANGE_REQUEST_STALE",
            "SELF_APPROVAL_DENIED",
        ],
    ),
    (
        "DashboardAsset",
        &[
            "TENANT_SCOPE_REQUIRED",
            "DASHBOARD_REVISION_CONFLICT",
            "DASHBOARD_NOT_GRANTED",
            "DASHBOARD_BODY_INVALID",
            "DASHBOARD_PURGED",
        ],
    ),
    (
        "DataProductRegister",
        &[
            "TENANT_SCOPE_REQUIRED",
            "RECORD_TENANT_MISMATCH",
            "STEWARD_ROLE_MISSING",
            "CONTRACT_SCHEMA_BREAK",
        ],
    ),
    (
        "AccessRequestOpen",
        &[
            "TENANT_SCOPE_REQUIRED",
            "RECORD_TENANT_MISMATCH",
            "ACCESS_PURPOSE_MISSING",
            "STEWARD_ROLE_MISSING",
        ],
    ),
    (
        "AccessRequestDecide",
        &[
            "TENANT_SCOPE_REQUIRED",
            "RECORD_TENANT_MISMATCH",
            "STEWARD_ROLE_MISSING",
        ],
    ),
    (
        "ClassificationClaim",
        &[
            "TENANT_SCOPE_REQUIRED",
            "CLAIM_EVIDENCE_MISSING",
            "CLAIM_STATE_ILLEGAL",
            "CLAIM_UNKNOWN_MARKING",
            "STEWARD_ROLE_MISSING",
        ],
    ),
    (
        "QualityRulePut",
        &[
            "TENANT_SCOPE_REQUIRED",
            "QUALITY_RULE_INVALID",
            "QUALITY_THRESHOLD_INVALID",
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
        &[
            "DECISIONS",
            "UNSUPPORTED_COALITION",
            "CAPACITY_UNAVAILABLE",
            "TASK_CONTEXT_PLAN_MISSING",
            "TASK_CONTEXT_ROW_UNKNOWN",
        ],
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
    // `RbacAdmin` also serves the ownership apply and rollback (MEDULLA-IDENTITY-R700.4): a
    // store-managed principal in a plan meets the managed fence, and the three ownership
    // refusals are its own.
    (
        "RbacAdmin",
        &[
            "IDENTITY_STORE_NAMESPACE",
            "IDENTITY_STORE_MANAGED",
            "OWNERSHIP_LAST_ADMIN",
            "OWNERSHIP_WIDENS_TENANT",
            "OWNERSHIP_NO_ROLLBACK",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R720.5: the DECAY and CLUSTER BY EMBEDDING stage refusals.
    (
        "Uql",
        &[
            // MEDULLA-FEDERATED-QUERY-R073.2: a pushdown outside the connection protocol's row.
            "PUSHDOWN_NOT_IN_MATRIX",
            "EVOLUTION_DIGEST_MISMATCH",
            "EVOLUTION_REVISION_UNKNOWN",
            "EVOLUTION_WINDOW_STALE",
            "UQL_E_CLUSTER_NO_VECTOR",
            "UQL_E_DECAY_NO_TIME",
            "USAGE_CURSOR_INVALID",
            "USAGE_CURSOR_STALE",
            "USAGE_QUERY_INVALID",
        ],
    ),
    // MEDULLA-FEDERATED-QUERY-R733: the GROUP stage refusals.
    ("Uql", &["UQL_GROUP_LIMIT", "UQL_UNKNOWN_FIELD"]),
    ("UnifiedQuery", &["UQL_GROUP_LIMIT", "UQL_UNKNOWN_FIELD"]),
    // MEDULLA-FEDERATED-QUERY-R731: an interactive query past its node budget.
    ("UnifiedQuery", &["QUERY_BUDGET_EXCEEDED"]),
    // MEDULLA-FEDERATED-QUERY-R734: the ORDER and SKIP stage refusals.
    ("Uql", &["UQL_SKIP_LIMIT"]),
    ("UnifiedQuery", &["UQL_SKIP_LIMIT"]),
    // MEDULLA-FEDERATED-QUERY-R078.3 / R078.4: the plan scope refusals.
    ("Uql", &["PLAN_SCOPE_INVALID", "PLAN_SCOPE_UNVERIFIED"]),
    // MEDULLA-FEDERATED-QUERY-R078.7: the company-context view refusal.
    ("Uql", &["COMPANY_MAPPING_MISSING", "SNAPSHOT_TIME_UNKNOWN"]),
    // MEDULLA-DECISION-ENGINE-R144.8: a statement past the applied result budget.
    ("Uql", &["INDEXLESS_BUDGET_EXHAUSTED"]),
    // MEDULLA-FEDERATED-QUERY-R032.100: the `PROPAGATE ... RISK HISTORY` refusals.
    ("Uql", &["HISTORY_DATASET_UNAVAILABLE", "HISTORY_MALFORMED"]),
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
        // These codes already belong to the connector-pack typed catalog. A receipt
        // whose pack head moved is refused as PACK_HEAD_CONFLICT (MEDULLA-FEDERATED-QUERY-R722.2).
        codes.extend([
            "PACK_HEAD_CONFLICT",
            "SOURCE_CONTRACT_MISMATCH",
            "SOURCE_REGISTRATION_NOT_FOUND",
        ]);
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
    // MEDULLA-TYPED-PACKS-R108.4: the catalog write guard refuses an orphan asset
    // class and a marked person field on the RDF write route and on the Agent
    // Library publish route that emits catalog nodes.
    if matches!(id, "AddTriples" | "AgentComponent") {
        codes.extend(["ASSET_CLASS_ORPHAN", "FIELD_MARKED"]);
    }
    // MEDULLA-TYPED-PACKS-R106.4, R108.13: the catalog shape codes, the batch bound and the
    // article version rule on the RDF write route.
    if id == "AddTriples" {
        codes.extend([
            "UNRESOLVED_REFERENCE",
            "RESOURCE_UNOWNED",
            "LINEAGE_EDGE_UNPAIRED",
            "CATALOG_BATCH_TOO_LARGE",
            "ARTICLE_VERSION_INVALID",
            "METRIC_TERM_UNRESOLVED",
        ]);
    }
    // MEDULLA-TYPED-PACKS-R650.4: the object-model guard refuses a value or link
    // that breaks a pack's declared value type or link cap on the RDF write route and
    // the source-ingest batch; the import refuses an unimplemented interface.
    if matches!(id, "AddTriples" | "SourceIngest") {
        codes.extend(["VALUE_CONSTRAINT_VIOLATED", "LINK_CARDINALITY_EXCEEDED"]);
    }
    if id == "ConnectorPack" {
        codes.insert("INTERFACE_UNIMPLEMENTED");
    }
    // MEDULLA-FEDERATED-QUERY-R058.4.1: `max_steps` outside 1 to 4.
    if id == "NlQuery" {
        codes.insert("NL_STEPS_INVALID");
    }
    // MEDULLA-FEDERATED-QUERY-R058.4.4: the bounded-planning refusals.
    // NlQueryExplain shares NlQuery's run and refuses exactly like it
    // (MEDULLA-FEDERATED-QUERY-R058.2.2.1).
    if matches!(id, "NlQuery" | "NlQueryExplain" | "NlQueryPlan") {
        codes.extend([
            "NL_QUERY_STEPS_EXHAUSTED",
            "NL_PLANNER_UNCONFIGURED",
            "NL_MUTATION_REFUSED",
        ]);
    }
    // MEDULLA-FEDERATED-QUERY-R058.7: a correction without the certify scope.
    if id == "NlQueryCorrect" {
        codes.insert("NL_CORRECTION_UNAUTHORIZED");
    }
    // MEDULLA-FEDERATED-QUERY-R058.6: the certified-definition loop ended in a mismatch.
    if matches!(id, "NlQuery" | "NlQueryExplain") {
        codes.insert("NL_DEFINITION_CONFLICT");
    }
    // MEDULLA-IDENTITY-R023.4: a point read of a unit outside the caller's scope.
    // MEDULLA-DURABLE-KERNEL-R086.3: a durable-tier fault on the miss path.
    if id == "GetNodeProperties" {
        codes.extend(["UNIT_OUT_OF_SCOPE", "DURABLE_READ_UNAVAILABLE"]);
    }
    // MEDULLA-IDENTITY-R702: binding a policy rule bundle refuses these two codes.
    if id == "Identity" {
        codes.extend(["POLICY_SCOPE_BOUND", "POLICY_SELF_AUTHORIZATION"]);
        // MEDULLA-ENGINE-PARITY-R007.4: granting or revoking a wire login refuses these two.
        codes.extend(["WIRE_LOGIN_EXISTS", "WIRE_LOGIN_UNKNOWN"]);
    }
    // MEDULLA-SRC-AUTH-R011.3: the trust mark is stamped in the mutation gateway.
    if id == "AddNode" {
        codes.insert("CONTENT_ORIGIN_UNKNOWN");
        // MEDULLA-ORCH-VOCAB-R808: an agent definition node names only knowledge sets the
        // caller's graph holds.
        codes.insert("SCOPE_SET_UNKNOWN");
    }
    if id == "CompareAndSetNodeFields" {
        codes.extend(["TRUST_MARK_DOWNGRADE_REFUSED", "TRUST_MARK_FIELDS_RESERVED"]);
    }
    if d.policy.is_durable() {
        codes.insert("CORRUPT_MUTATION_LEDGER");
    }
    // MEDULLA-DECISION-ENGINE-R173.8: the mutation compiler refuses a caller write of
    // `editStatus` on every node-writing route.
    if matches!(
        d.id.as_str(),
        "AddNode"
            | "CreateNodeIfAbsent"
            | "TxnAddNode"
            | "ApplyMutation"
            | "Sql"
            | "CypherQuery"
            | "AddTriples"
            | "ApplyChangeEnvelope"
            | "ApplyChangeEnvelopeDraft"
            | "ApplyChangeEnvelopes"
            | "ApplyChangeEnvelopeDrafts"
    ) {
        codes.insert("EDIT_STATUS_ENGINE_OWNED");
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
    // MEDULLA-DECISION-ENGINE-R803.8: DecisionLog's improvement-library ops (harness
    // adoption, lessons) refuse with the same closed harness vocabulary codes.
    if d.id.as_str() == "DecisionLog" {
        codes.extend(typed_codes(
            HarnessVocabularyErrorCode::ALL,
            HarnessVocabularyErrorCode::as_str,
        ));
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
    // MEDULLA-FEDERATED-QUERY-R090.4 / R651.4: the planner and expansion-port refusals.
    // NlQueryExplain shares NlQuery's planning entry and refuses exactly like it
    // (MEDULLA-FEDERATED-QUERY-R058.2.2.1).
    if matches!(d.id.as_str(), "Discover" | "NlQuery" | "NlQueryExplain") {
        use medulla_ingestion_types::ingestion_wire::{
            DISCOVER_PLANNER_UNCONFIGURED, EXPANSION_FANOUT_EXCEEDED, EXPANSION_PROVIDER_FAILED,
            EXPANSION_PROVIDER_UNCONFIGURED, QUERY_TEXT_TOO_LONG,
        };
        codes.extend([
            QUERY_TEXT_TOO_LONG,
            EXPANSION_FANOUT_EXCEEDED,
            EXPANSION_PROVIDER_UNCONFIGURED,
            EXPANSION_PROVIDER_FAILED,
        ]);
        if d.id.as_str() == "Discover" {
            codes.insert(DISCOVER_PLANNER_UNCONFIGURED);
            // MEDULLA-FEDERATED-QUERY-R741.2: the seed and hop-filter refusals.
            codes.extend([
                medulla_ingestion_types::ingestion_wire::SEED_NOT_VISIBLE,
                medulla_ingestion_types::ingestion_wire::SEED_LIMIT_EXCEEDED,
                medulla_ingestion_types::ingestion_wire::HOP_FILTER_INVALID,
            ]);
        }
    }
    let id = d.id.as_str();
    if matches!(
        id,
        "Uql" | "NlQuery" | "NlQueryExplain" | "NlQueryPlan" | "KnowledgeStream"
    ) {
        codes.extend(typed_codes(UqlCode::ALL, UqlCode::as_str));
    }
    if id.starts_with("Decision") || id.starts_with("Decide") {
        // The decision plane's family and module refusal tables.
        // The registry also holds the shared machine refusal; a decision method is
        // not a machine method (MEDULLA-ORCH-VOCAB-R811.8), so only MACHINE_METHODS
        // add TRANSITION_REFUSED.
        codes.extend(
            medulla_decision_types::decision::codes::registered()
                .map(|code| code.as_str())
                .filter(|code| *code != TRANSITION_REFUSED),
        );
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
    if id == "GraphSchema" {
        // MEDULLA-TYPED-PACKS-R103.4: the runtime ontology import, withdrawal
        // and diff refusals (`ontology_import.rs`, `ontology_diff_serve.rs`).
        codes.extend(
            [
                ServerErrorCode::ImporterMismatch,
                ServerErrorCode::OntologyCoreConflict,
                ServerErrorCode::OntologyDiffIriMismatch,
                ServerErrorCode::OntologyDigestMismatch,
                ServerErrorCode::OntologyRevisionConflict,
                ServerErrorCode::OntologyRevisionUnknown,
                ServerErrorCode::ShaclViolation,
                ServerErrorCode::TenantDenied,
            ]
            .map(|code| code.as_str()),
        );
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
    if id == "FrictionReport" {
        // MEDULLA-DECISION-ENGINE-R710.4: the friction refusal table.
        codes.extend(
            medulla_decision_types::friction::REFUSALS
                .iter()
                .map(|code| code.as_str()),
        );
    }
    if id == "SkillTelemetry" {
        // MEDULLA-DECISION-ENGINE-R713.4: the telemetry refusal table.
        codes.extend(
            medulla_decision_types::skill_telemetry::REFUSALS
                .iter()
                .map(|code| code.as_str()),
        );
    }
    if id == "KnowledgeCuration" {
        // MEDULLA-DECISION-ENGINE-R709.4: the curation refusal table.
        codes.extend(
            medulla_decision_types::knowledge_curation::REFUSALS
                .iter()
                .map(|code| code.as_str()),
        );
    }
    if id == "KnowledgeFlag" {
        // MEDULLA-DECISION-ENGINE-R708.4: the flag refusal table.
        codes.extend(
            medulla_decision_types::knowledge_flag::REFUSALS
                .iter()
                .map(|code| code.as_str()),
        );
    }
    if id == "FeatureLedger" {
        // MEDULLA-DECISION-ENGINE-R707.4: the ledger's own refusal table (a
        // stale compare-and-set, a close by an unknown change).
        codes.extend(
            medulla_decision_types::decision::feature_match::REFUSALS
                .iter()
                .map(|code| code.as_str()),
        );
    }
}

/// The served methods that move an explicit state machine (MEDULLA-ORCH-VOCAB-R811.8,
/// organization constitution Article XXXI). Each may answer the typed record
/// `{code: TRANSITION_REFUSED, machine, from, event, reason}`.
pub const MACHINE_METHODS: &[&str] = &[
    "FeatureLedger",
    // MEDULLA-DECISION-ENGINE-R701.3: `MarkAddressed` moves the gap-topic machine.
    "FailureIngest",
    // MEDULLA-DECISION-ENGINE-R708.3: `Resolve` and a repeat `Raise` move the flag machine.
    "KnowledgeFlag",
    // MEDULLA-DECISION-ENGINE-R709.5: `Status` moves each addressed flag with the adopt event.
    "KnowledgeCuration",
    // MEDULLA-DECISION-ENGINE-R710.3: `Transition` moves the friction machine.
    "FrictionReport",
    "SubmitWorkItem",
    "ClaimWorkItem",
    "CancelWorkItem",
    "ReleaseWorkItemResources",
    "ReleaseCapacity",
    "ResourceReservationStatus",
    // `Method::AnalyticsJob` exists only with the `jobs` feature.
    #[cfg(feature = "jobs")]
    "AnalyticsJob",
    "BrokerConsume",
    "BrokerAck",
    "BrokerReject",
    "BrokerAckTag",
    "BrokerNackTag",
    "Decide",
    // MEDULLA-FINANCE-PRIMITIVES-R011.5: `FinanceReceiptTransition` moves the receipt machine.
    "FinanceReceiptTransition",
    // MEDULLA-DECISION-ENGINE-R173.7: the edit lifecycle machine.
    "HarnessEdit",
    // MEDULLA-DECISION-ENGINE-R830.6: the loop statechart and `GapMachine` refusals.
    "Loops",
    // MEDULLA-ORCH-VOCAB-R815.6: the incident record status machine.
    "IncidentSetStatus",
    // MEDULLA-ORCH-VOCAB-R815.9: the workflow registration lifecycle machine.
    "WorkflowRegistry",
    // MEDULLA-ORCH-VOCAB-R815.4: the work market gap status machine.
    "GapTransition",
    // MEDULLA-TYPED-PACKS-R805.3: an access request moves `AccessRequestMachine`.
    "AccessRequestDecide",
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
    // MACHINE_METHODS is the one authority for TRANSITION_REFUSED: a family code
    // table shared by several methods (the decision plane's) must not list it
    // on a method that moves no machine.
    if MACHINE_METHODS.contains(&d.id.as_str()) {
        codes.insert(TRANSITION_REFUSED);
    } else {
        codes.remove(TRANSITION_REFUSED);
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

    /// A code a method contract allows must also be a declared wire code, or the
    /// served boundary reports it as an unclassified `INTERNAL`.
    #[test]
    fn every_method_contract_code_is_a_declared_wire_code() {
        let mut undeclared = std::collections::BTreeSet::new();
        for descriptor in crate::method_descriptors() {
            for code in super::method_error_set(&descriptor) {
                if !medulla_protocol_types::error_registry::declared_error_code(code) {
                    undeclared.insert(code);
                }
            }
        }
        assert!(
            undeclared.is_empty(),
            "undeclared wire codes: {undeclared:?}"
        );
    }

    #[test]
    fn connector_and_foreign_refusals_are_routed_to_their_served_methods() {
        assert!(method_allows_error("ConnectorPack", "SEED_TURTLE_INVALID"));
        assert!(method_allows_error(
            "ConnectorPack",
            "CONFORMANCE_EVIDENCE_ABSENT"
        ));
        assert!(method_allows_error("Sparql", "PACK_NOT_IMPORTED"));
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
        #[cfg(feature = "jobs")]
        assert!(method_allows_error(
            "AnalyticsJob",
            "SCHEDULER_CONFIG_INVALID"
        ));
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

    // spec: MEDULLA-TYPED-PACKS-R103.4
    #[test]
    fn graph_schema_answers_the_runtime_ontology_refusals() {
        for code in [
            "IMPORTER_MISMATCH",
            "ONTOLOGY_CORE_CONFLICT",
            "ONTOLOGY_DIGEST_MISMATCH",
            "ONTOLOGY_REVISION_CONFLICT",
            "SHACL_VIOLATION",
            "TENANT_DENIED",
        ] {
            assert!(method_allows_error("GraphSchema", code), "{code}");
        }
        assert!(!method_allows_error("GraphSchemaList", "IMPORTER_MISMATCH"));
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
            ("RbacAdmin", "IDENTITY_STORE_MANAGED", true),
            ("RbacAdmin", "OWNERSHIP_LAST_ADMIN", true),
            ("RbacAdmin", "OWNERSHIP_WIDENS_TENANT", true),
            ("RbacAdmin", "OWNERSHIP_NO_ROLLBACK", true),
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
