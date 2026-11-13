//! Pure-data result bodies of the `ingestion` contract domain: the source-code parse
//! and repository-index graphs, the screen-observation graph, and discovery hits.
//!
//! They live here, at the bottom of the crate DAG, so the result contract
//! (`result_contract::ingestion`) can name the exact body a handler encodes; the
//! parsers and the screen enrichment (`medulla-compute`) re-export them.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::contract::BoundedVec;

pub const MAX_INDEX_DIAGNOSTICS_PER_FILE: usize = 8;

/// Per-file disposition produced by [`IndexResult`].  It is deliberately
/// separate from `ParseResult`: an unsupported parser capability and a source
/// file containing no declarations are not the same outcome.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IndexFileStatus {
    Success,
    Unsupported,
    Error,
}

/// One bounded, machine-readable diagnostic for a repository-index input.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexDiagnostic {
    pub code: String,
    pub message: String,
}

/// The exact outcome of one input file in an `IndexRepository` batch.
///
/// Outcomes are returned one-for-one and in the same order as the submitted
/// files. Both digests use the canonical `sha256:<lowercase-hex>` spelling.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexFileOutcome {
    pub file_path: String,
    pub status: IndexFileStatus,
    pub content_digest: String,
    pub parser_capability_digest: String,
    /// Bounded by the engine. The current parser emits at most one diagnostic
    /// for an input, while the vector leaves room for richer parsers without a
    /// wire-shape change.
    pub diagnostics: BoundedVec<IndexDiagnostic, MAX_INDEX_DIAGNOSTICS_PER_FILE>,
}

/// Engine-only evidence for the native ingestion rungs of one content+grammar
/// unit. This never crosses the wire: callers cannot claim that a rung ran or
/// abstained to gain access to a more expensive stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRungEvidence {
    pub content_digest: String,
    pub parser_capability_digest: String,
    pub status: IndexFileStatus,
    pub extracted_facts: usize,
    pub inferred_facts: usize,
    pub derived_facts: usize,
    pub symbol_resolution_completed: bool,
    pub statistical_completed: bool,
}

/// An extracted graph node: the shape the AST and screen enrichments share, so the
/// caller's persist path is one.
#[derive(Serialize, Deserialize, Debug)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone)]
// Clone is derived so a delta can rebind reused artifacts without reparsing.
pub struct ExtractedNode {
    pub node_id: String,
    pub node_type: String,
    pub properties: HashMap<String, String>,
}

/// An extracted graph edge.
#[derive(Serialize, Deserialize, Debug)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone)]
// Clone is derived so a delta can rebind reused artifacts without reparsing.
pub struct ExtractedEdge {
    pub source: String,
    pub target: String,
    pub edge_type: String,
    pub properties: HashMap<String, String>,
}

/// Result of `Method::ParseFile`, and one entry of `Method::ParseFiles`: the raw,
/// unresolved symbol graph of one source file.
#[derive(Serialize, Deserialize, Debug)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ParseResult {
    pub nodes: Vec<ExtractedNode>,
    pub edges: Vec<ExtractedEdge>,
    pub symbols_extracted: usize,
}

/// Resolved, cross-file symbol graph for a batch of files — the response shape
/// of the `IndexRepository` RPC. Unlike `ParseFiles` (one raw `ParseResult` per
/// file), this is a SINGLE merged graph whose `calls`/`inherits`/`realizes`/
/// `depends_on` edges point at real node ids.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexResult {
    /// Every SYMBOL node across all files — ONE row per declaration site, never
    /// deduplicated. The doc used to claim "deduplicated by node id" and
    /// `collect_result_nodes` never did it; the DOC was the wrong half. Under the
    /// old content-addressed id, deduplicating would have been a data-loss bug:
    /// byte-identical declarations shared an id but not their facts (`file_path`,
    /// `line`, byte range), so collapsing them would have thrown away every
    /// occurrence but the first. Ids are now per-occurrence
    /// (CONCEPT:MEDULLA-KG.compute.symbol-occurrence-id), so uniqueness holds by
    /// construction and there is nothing left to dedupe — provided the batch
    /// carries each file path once, which is the caller's contract (a path
    /// repeated in `files` is parsed twice and would repeat its ids). The
    /// internal `call_sites` resolution-input property is stripped before return.
    pub nodes: Vec<ExtractedNode>,
    /// `IMPLEMENTS` (file→symbol) + resolved `calls` (symbol→symbol) + `inherits`/
    /// `realizes` (class→class) + resolved `depends_on` (file→file). Raw unresolved
    /// `calls_raw`/`depends_on_raw` edges are dropped — they're superseded here.
    pub edges: Vec<ExtractedEdge>,
    pub symbols_extracted: usize,
    /// Successfully parsed inputs. This is not the submitted batch size: see
    /// `file_outcomes` for unsupported and failed inputs.
    pub files_parsed: usize,
    /// Exactly one outcome per submitted file, in input order. An unsupported
    /// extension is explicit and never represented as an empty success.
    pub file_outcomes: Vec<IndexFileOutcome>,
    #[serde(skip)]
    #[cfg_attr(feature = "contract-schema", schemars(skip))]
    pub native_rung_evidence: Vec<NativeRungEvidence>,
    /// Call sites bound to a definition (numerator of call-resolution coverage).
    pub calls_resolved: usize,
    /// Call sites seen but not bound (external/stdlib/ambiguous) — the remainder.
    pub calls_unresolved: usize,
    /// Of `calls_resolved`, those bound by receiver/class scope (CONCEPT:MEDULLA-KG.compute.type-scope-resolved-call).
    pub calls_scope_resolved: usize,
    /// Of `calls_resolved`, those disambiguated by argument-count match.
    pub calls_type_resolved: usize,
    /// Class→base `inherits` edges emitted.
    pub inherits_edges: usize,
    /// Class→interface `realizes` edges emitted.
    pub realizes_edges: usize,
    /// Model-free `similar_to` edges emitted (CONCEPT:MEDULLA-KG.compute.model-free-similar-code).
    pub similar_edges: usize,
    /// Import statements bound to an in-batch file.
    pub imports_resolved: usize,
    /// Import statements seen but not bound (external packages, unknown layout).
    pub imports_unresolved: usize,
}

/// Upper bound on refs one branch-aware `IndexRepository` batch may declare.
pub const MAX_INDEX_SCOPE_REFS: usize = 4_096;
/// Upper bound on ref memberships (or tombstones) in one branch-aware batch.
pub const MAX_INDEX_SCOPE_FILE_VERSIONS: usize = 262_144;

/// Upper bound on commits one branch-aware `IndexRepository` batch may carry.
pub const MAX_INDEX_SCOPE_COMMITS: usize = 100_000;
/// Upper bound on the file changes of one commit in an index scope.
pub const MAX_INDEX_COMMIT_CHANGES: usize = 1_024;

/// How a commit changed one file.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IndexChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// One file a commit changed, with its line counts.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexCommitChange {
    pub path: String,
    pub kind: IndexChangeKind,
    pub added_lines: u32,
    pub removed_lines: u32,
}

/// One commit of the repository history (MEDULLA-REPO-INGEST-R601.3). The
/// digest is a lowercase 40- or 64-hex commit id.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexCommit {
    pub digest: String,
    pub author: String,
    pub timestamp_ms: u64,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub changes: BoundedVec<IndexCommitChange, MAX_INDEX_COMMIT_CHANGES>,
}

/// Whether a declared ref still exists at the indexed revision (EH-280).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IndexRefStatus {
    /// The ref resolves to `revision_id`; its memberships ride in `file_versions`.
    Live,
    /// The ref no longer exists; `revision_id` is the last admitted revision and
    /// every prior membership arrives as a tombstone.
    Deleted,
}

/// One branch/tag ref pinned to an immutable commit.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexRef {
    pub ref_name: String,
    /// Lowercase 40- or 64-hex Git commit id, never a mutable ref name.
    pub revision_id: String,
    pub status: IndexRefStatus,
}

/// `ref_name` contains `path` bound to the blob whose content digest is
/// `blob_digest` (`sha256:<hex>`). The blob may have been submitted in this
/// batch or in an earlier one: a `:FileVersion` needs only the digest.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexFileVersion {
    pub ref_name: String,
    pub path: String,
    pub blob_digest: String,
}

/// Upper bound on the delta entries of one bounded incremental index job.
pub const MAX_INDEX_DELTA_PATHS: usize = 262_144;

/// One added or modified path of a bounded incremental index delta
/// (MEDULLA-REPO-INGEST-R006.4): its parse artifacts arrive fresh with the
/// delta and bind to `path`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexDeltaChange {
    pub path: String,
    pub blob_digest: String,
}

/// One renamed path: the artifact under `from` rebinds to the target
/// generation under `to` and reuses its parse artifact.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexDeltaRename {
    pub from: String,
    pub to: String,
}

/// The bounded incremental index delta of one `IndexRepository` job
/// (MEDULLA-REPO-INGEST-R006.4). `base_revision` names the base the target
/// generation chains from; a mismatch refuses with `INDEX_BASE_MISMATCH`
/// before any write. Paths in `added`, `modified`, `deleted` and `renamed`
/// are disjoint; every other path reuses its parse artifacts unchanged.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexDelta {
    pub base_revision: String,
    pub target_revision: String,
    #[serde(default)]
    pub added: BoundedVec<IndexDeltaChange, MAX_INDEX_DELTA_PATHS>,
    #[serde(default)]
    pub modified: BoundedVec<IndexDeltaChange, MAX_INDEX_DELTA_PATHS>,
    #[serde(default)]
    pub deleted: BoundedVec<String, MAX_INDEX_DELTA_PATHS>,
    #[serde(default)]
    pub renamed: BoundedVec<IndexDeltaRename, MAX_INDEX_DELTA_PATHS>,
}

/// `path` (bound to `prior_blob_digest`) left `ref_name` since the preceding
/// admitted revision; `successor_path` names the rename target when known.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexTombstone {
    pub ref_name: String,
    pub path: String,
    pub prior_blob_digest: String,
    #[serde(default)]
    pub successor_path: Option<String>,
}

/// Branch-aware, blob-deduplicated scope of one `IndexRepository` batch
/// (EH-280). With a scope, each submitted file is ONE unique blob (named by
/// one of the paths it occurs at): the engine parses it exactly once, attaches
/// its symbols to `:Blob`, and projects `:FileVersion` (path + blob) and
/// `:Branch -> :FileVersion` membership for every ref in the batch.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexRepositoryScope {
    pub repository_id: String,
    pub refs: BoundedVec<IndexRef, MAX_INDEX_SCOPE_REFS>,
    #[serde(default)]
    pub file_versions: BoundedVec<IndexFileVersion, MAX_INDEX_SCOPE_FILE_VERSIONS>,
    #[serde(default)]
    pub tombstones: BoundedVec<IndexTombstone, MAX_INDEX_SCOPE_FILE_VERSIONS>,
    /// Commit history to enrich. Absent means no history.
    #[serde(default)]
    pub commits: BoundedVec<IndexCommit, MAX_INDEX_SCOPE_COMMITS>,
}

/// Result of `Method::ObserveScreen`: one captured frame as session/frame/UI-element
/// graph entities.
#[derive(Serialize, Deserialize, Debug)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScreenObservationResult {
    /// The session node + the frame node + one node per UI element.
    pub nodes: Vec<ExtractedNode>,
    /// session-`hasObservation`->frame, frame-`hasElement`->element, and
    /// prevframe-`succeededBy`->frame (only when the frame actually changed).
    pub edges: Vec<ExtractedEdge>,
    pub frame_id: String,
    pub width: u32,
    pub height: u32,
    /// FNV-1a hash of the PNG bytes — the caller passes it back as `prev_hash`.
    pub hash: u64,
    /// False when the frame is byte-identical to the previous one (no visual change).
    pub changed: bool,
    pub element_count: usize,
}

/// One ranked node of `Method::Discover`, hydrated with its human-readable text.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DiscoverHit {
    pub id: String,
    /// The node's `name`, or its id when it has none.
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub node_type: String,
    /// Combined keyword + semantic score.
    pub score: f32,
    /// Seed-to-hit node ids when the hit was reached by neighbour expansion
    /// (MEDULLA-FEDERATED-QUERY-R090.8); empty for a fused seed hit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    /// Source URL when the hit is a web Document (MEDULLA-FEDERATED-QUERY-R653.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Unix seconds at which the web Document was retrieved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at: Option<i64>,
    /// The backend that produced the web Document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// SHA-256 of the query that found the web Document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_digest: Option<String>,
}
