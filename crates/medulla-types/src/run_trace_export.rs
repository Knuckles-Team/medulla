//! The correlated run-trace export read (MEDULLA-ORCH-VOCAB-R009.100).
//!
//! One `RunTraceExport` request names a root `:RunTrace` and answers one page of the records
//! correlated to it: the parent run, its child agent runs, their spans and their tool calls.
//! The read follows exactly the declared child edges of the run-trace ontology and no other
//! edge. The page is a read of the graph at one pinned `source_revision`; it writes nothing and
//! names no store of its own.

use serde::{Deserialize, Serialize};

use crate::refusal_code::RefusalCode;

/// Current record version of the export request and page.
pub const RUN_TRACE_EXPORT_SCHEMA_VERSION: u16 = 1;

/// The `:RunTrace` -> child agent run edge of the run-trace ontology.
pub const EDGE_HAS_CHILD_RUN: &str = "hasChildRun";
/// The `:RunTrace` -> span edge of the run-trace ontology.
pub const EDGE_HAS_SPAN: &str = "hasSpan";
/// The `:RunTrace` -> tool call edge of the run-trace ontology.
pub const EDGE_HAS_TOOL_CALL: &str = "hasToolCall";
/// The declared child edges, in the order the export visits them. No other edge is followed.
pub const CHILD_EDGES: [&str; 3] = [EDGE_HAS_CHILD_RUN, EDGE_HAS_SPAN, EDGE_HAS_TOOL_CALL];

/// A span the root (or a child run) declared and the export did not find.
pub const EXPORT_CHILD_MISSING: &str = "EXPORT_CHILD_MISSING";
/// One span id reached twice, so the digest would count it twice.
pub const EXPORT_SPAN_DUPLICATE: &str = "EXPORT_SPAN_DUPLICATE";
/// A page token or pinned revision that no longer names the snapshot it was cut from.
pub const EXPORT_CURSOR_STALE: &str = "EXPORT_CURSOR_STALE";
/// The root run is not visible to the caller's tenant (a foreign tenant reads as absent).
pub const RUN_TRACE_NOT_FOUND: &str = "RUN_TRACE_NOT_FOUND";

/// The refusal codes of the served run-trace export. The decision plane collects this table
/// with every other family table and refuses a code declared twice.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(EXPORT_CHILD_MISSING),
    RefusalCode::new(EXPORT_SPAN_DUPLICATE),
    RefusalCode::new(EXPORT_CURSOR_STALE),
    RefusalCode::new(RUN_TRACE_NOT_FOUND),
];

fn schema_version() -> u16 {
    RUN_TRACE_EXPORT_SCHEMA_VERSION
}

/// What the page carries of each record's content. The set is closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RedactionProfile {
    /// Only the structure and the content digest of each record.
    #[default]
    DigestsOnly,
    /// The structure and the non-content properties; message, argument and result bodies are
    /// removed.
    StripContent,
    /// Every property. Only the owner of the trace may ask for it.
    None,
}

/// The inclusive millisecond window the records must start in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TimeRange {
    pub from_ms: u64,
    pub to_ms: u64,
}

/// One export request (`Method::RunTraceExport`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunTraceExportV1 {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    /// The node id of the root `:RunTrace`.
    pub root_run_id: String,
    /// Only records that started inside the window are returned.
    #[serde(default)]
    pub time_range: Option<TimeRange>,
    /// The graph revision the export is pinned to. Absent pins the current one; a value that
    /// is no longer current refuses `EXPORT_CURSOR_STALE`.
    #[serde(default)]
    pub source_revision: Option<u64>,
    #[serde(default)]
    pub redaction_profile: RedactionProfile,
    /// Records per page; absent is `orchestration_limits.page_size_max`, and a larger value is
    /// clamped to it.
    #[serde(default)]
    pub page_size: Option<u32>,
    /// The `next_page_token` of the previous page.
    #[serde(default)]
    pub page_token: Option<String>,
}

/// The kind of one exported record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunTraceRecordKind {
    ParentRun,
    ChildAgent,
    Span,
    ToolCall,
}

/// One correlated record of the export.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunTraceRecord {
    /// The node id of the record.
    pub record_ref: String,
    pub kind: RunTraceRecordKind,
    /// The record it hangs from; absent for the parent run.
    #[serde(default)]
    pub parent_ref: Option<String>,
    /// When the record started, in unix milliseconds, when the node says.
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    /// `sha256:<hex>` of the record's canonical properties.
    pub content_digest: String,
    /// The properties the redaction profile lets through; absent under `digests_only`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Map<String, serde_json::Value>>,
}

/// One page of the export (`RunTraceExportPageV1`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RunTraceExportPageV1 {
    #[serde(default = "schema_version")]
    pub schema_version: u16,
    pub root_run_id: String,
    /// The graph revision the page was read at.
    pub source_revision: u64,
    pub redaction_profile: RedactionProfile,
    pub records: Vec<RunTraceRecord>,
    /// True only when every span the trace declared is present in the export.
    pub complete: bool,
    /// Set to `EXPORT_CHILD_MISSING` when `complete` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// The declared span ids the export did not find, sorted.
    #[serde(default)]
    pub missing_children: Vec<String>,
    /// `sha256:<hex>` over the ordered record refs of the pinned revision.
    pub snapshot_digest: String,
    /// The token of the next page; absent on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> RunTraceExportPageV1 {
        RunTraceExportPageV1 {
            schema_version: RUN_TRACE_EXPORT_SCHEMA_VERSION,
            root_run_id: "run:parent".to_string(),
            source_revision: 7,
            redaction_profile: RedactionProfile::DigestsOnly,
            records: vec![
                RunTraceRecord {
                    record_ref: "run:parent".to_string(),
                    kind: RunTraceRecordKind::ParentRun,
                    parent_ref: None,
                    started_at_ms: Some(1_000),
                    content_digest: format!("sha256:{}", "a".repeat(64)),
                    properties: None,
                },
                RunTraceRecord {
                    record_ref: "span:1".to_string(),
                    kind: RunTraceRecordKind::Span,
                    parent_ref: Some("run:parent".to_string()),
                    started_at_ms: None,
                    content_digest: format!("sha256:{}", "b".repeat(64)),
                    properties: None,
                },
            ],
            complete: false,
            refusal: Some(EXPORT_CHILD_MISSING.to_string()),
            missing_children: vec!["span:2".to_string()],
            snapshot_digest: format!("sha256:{}", "c".repeat(64)),
            next_page_token: Some(format!("sha256:{}:2", "c".repeat(64))),
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.100
    /// Verifies MEDULLA-ORCH-VOCAB-R009.100: the request and the page round-trip through JSON
    /// against fixed vectors, and a field no record names is refused.
    #[test]
    fn page_round_trips_against_vectors() {
        let request = r#"{"root_run_id":"run:parent","time_range":{"from_ms":1,"to_ms":9},
            "source_revision":7,"redaction_profile":"strip_content","page_size":50,
            "page_token":"sha256:00:0"}"#;
        let decoded: RunTraceExportV1 = serde_json::from_str(request).unwrap();
        assert_eq!(decoded.schema_version, RUN_TRACE_EXPORT_SCHEMA_VERSION);
        assert_eq!(decoded.redaction_profile, RedactionProfile::StripContent);
        assert_eq!(decoded.time_range, Some(TimeRange { from_ms: 1, to_ms: 9 }));
        let again: RunTraceExportV1 =
            serde_json::from_str(&serde_json::to_string(&decoded).unwrap()).unwrap();
        assert_eq!(again, decoded);

        let minimal: RunTraceExportV1 = serde_json::from_str(r#"{"root_run_id":"r"}"#).unwrap();
        assert_eq!(minimal.redaction_profile, RedactionProfile::DigestsOnly);
        assert!(minimal.page_size.is_none() && minimal.page_token.is_none());
        assert!(serde_json::from_str::<RunTraceExportV1>(r#"{"root_run_id":"r","extra":1}"#).is_err());
        assert!(serde_json::from_str::<RedactionProfile>(r#""everything""#).is_err());

        let page = page();
        let text = serde_json::to_string(&page).unwrap();
        assert!(text.contains(r#""kind":"parent_run""#), "{text}");
        assert!(text.contains(r#""refusal":"EXPORT_CHILD_MISSING""#), "{text}");
        assert!(!text.contains("properties"), "digests_only carries no content: {text}");
        let back: RunTraceExportPageV1 = serde_json::from_str(&text).unwrap();
        assert_eq!(back, page);
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.100
    /// Verifies MEDULLA-ORCH-VOCAB-R009.100: the child edges are the three declared names and
    /// the refusal table carries each code once.
    #[test]
    fn child_edges_and_refusals_are_declared_once() {
        assert_eq!(CHILD_EDGES, ["hasChildRun", "hasSpan", "hasToolCall"]);
        let mut codes: Vec<&str> = REFUSALS.iter().map(|code| code.as_str()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), REFUSALS.len());
    }
}
