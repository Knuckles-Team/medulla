//! `Method::RunTraceExport` (MEDULLA-ORCH-VOCAB-R009.100): one page of the records correlated
//! to a root `:RunTrace`.
//!
//! The read runs over the caller's row-level-security projection of the request graph and
//! writes nothing. It starts at the root node, follows exactly the declared child edges of the
//! run-trace ontology (`hasChildRun`, `hasSpan`, `hasToolCall`; no other edge) and orders the
//! records depth first, children by node id. The snapshot digest is SHA-256 over the ordered
//! record refs of the pinned graph revision, so the same trace pages to the same digest. A
//! span the trace declared (`declaredSpans`) and the walk did not reach makes the page
//! incomplete with `EXPORT_CHILD_MISSING`; it is never a silent gap.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use medulla_protocol_types::result_contract::coordination::RunTraceExport;
use medulla_types::contract::Digest256;
use medulla_types::run_trace_export::{
    RedactionProfile, RunTraceExportPageV1, RunTraceExportV1, RunTraceRecord, RunTraceRecordKind,
    CHILD_EDGES, EDGE_HAS_CHILD_RUN, EDGE_HAS_SPAN, EDGE_HAS_TOOL_CALL, EXPORT_CHILD_MISSING,
    EXPORT_CURSOR_STALE, EXPORT_SPAN_DUPLICATE, RUN_TRACE_EXPORT_SCHEMA_VERSION,
    RUN_TRACE_NOT_FOUND,
};

type Props = serde_json::Map<String, serde_json::Value>;

/// The deepest chain of child runs the export follows.
const MAX_DEPTH: usize = 8;
/// The most records one export reads.
const MAX_RECORDS: usize = 10_000;
/// The properties `strip_content` removes: message, argument and result bodies.
const CONTENT_KEYS: [&str; 8] = [
    "content",
    "input",
    "output",
    "prompt",
    "arguments",
    "args",
    "result",
    "messages",
];

fn invalid(what: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: run trace export: {what}")
}

fn node_props(core: &GraphCore, id: &str) -> Option<Props> {
    let blob = core.get_node_properties(id)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

fn label_of(props: &Props) -> Option<&str> {
    ["node_type", "type", "label"]
        .iter()
        .find_map(|key| props.get(*key).and_then(serde_json::Value::as_str))
}

fn digest_of(props: &Props) -> String {
    // The map is key ordered, so its text is canonical.
    let text = serde_json::to_string(props).unwrap_or_default();
    format!("sha256:{}", Digest256::sha256(text.as_bytes()).to_hex())
}

fn started_at_ms(props: &Props) -> Option<u64> {
    ["started_at_ms", "start_ms", "start_time_ms"]
        .iter()
        .find_map(|key| props.get(*key).and_then(serde_json::Value::as_u64))
}

/// The span ids a run declared: a JSON array of strings, stored as an array or as its text.
fn declared_spans(props: &Props) -> Vec<String> {
    let value = match props.get("declaredSpans") {
        Some(serde_json::Value::String(text)) => {
            serde_json::from_str(text).unwrap_or(serde_json::Value::Null)
        }
        Some(other) => other.clone(),
        None => return Vec::new(),
    };
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One record found by the walk, before the redaction and the window apply.
struct Found {
    record_ref: String,
    kind: RunTraceRecordKind,
    parent_ref: Option<String>,
    props: Props,
}

/// The children of `parent` along the declared child edges: `(edge, child id)`, by child id.
fn children(core: &GraphCore, parent: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for child in core.get_successors(parent).unwrap_or_default() {
        for (relation, _) in core.typed_edges_between(parent, &child) {
            if let Some(edge) = CHILD_EDGES.iter().find(|edge| **edge == relation) {
                out.push((*edge, child.clone()));
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(b.0)));
    out.dedup();
    out
}

/// Walk the trace depth first and return its records in export order, with the span ids the
/// walked runs declared.
fn walk(
    core: &GraphCore,
    root: &str,
) -> Result<(Vec<Found>, BTreeSet<String>), String> {
    let not_found = || format!("{RUN_TRACE_NOT_FOUND}: {root}");
    let root_props = node_props(core, root).ok_or_else(not_found)?;
    // The caller's projection already narrowed the graph to its tenant: a node of another
    // tenant is absent, so it reads exactly as an unknown run.
    if label_of(&root_props) != Some("RunTrace") {
        return Err(not_found());
    }
    let mut declared: BTreeSet<String> = declared_spans(&root_props).into_iter().collect();
    let mut found = vec![Found {
        record_ref: root.to_string(),
        kind: RunTraceRecordKind::ParentRun,
        parent_ref: None,
        props: root_props,
    }];
    let mut seen: BTreeSet<String> = BTreeSet::from([root.to_string()]);
    // Depth first, in node id order: a stack whose last entry is the next to visit.
    let mut stack: Vec<(String, usize)> = vec![(root.to_string(), 0)];
    while let Some((parent, depth)) = stack.pop() {
        let mut next = Vec::new();
        for (edge, child) in children(core, &parent) {
            let Some(props) = node_props(core, &child) else {
                continue;
            };
            let kind = match (edge, label_of(&props)) {
                (EDGE_HAS_CHILD_RUN, Some("RunTrace")) => RunTraceRecordKind::ChildAgent,
                (EDGE_HAS_SPAN, Some("Span")) => RunTraceRecordKind::Span,
                (EDGE_HAS_TOOL_CALL, Some("ToolCall")) => RunTraceRecordKind::ToolCall,
                // A node the edge cannot name is not a child of this trace.
                _ => continue,
            };
            if !seen.insert(child.clone()) {
                return Err(format!("{EXPORT_SPAN_DUPLICATE}: {child}"));
            }
            if found.len() >= MAX_RECORDS {
                return Err(invalid(format!("the trace holds more than {MAX_RECORDS} records")));
            }
            if kind == RunTraceRecordKind::ChildAgent {
                declared.extend(declared_spans(&props));
                if depth + 1 < MAX_DEPTH {
                    next.push((child.clone(), depth + 1));
                }
            }
            found.push(Found {
                record_ref: child,
                kind,
                parent_ref: Some(parent.clone()),
                props,
            });
        }
        // The first child is visited first, so push the stack in reverse.
        stack.extend(next.into_iter().rev());
    }
    // The stack walk visits children of the last pushed run first; re-sort so the order is the
    // recorded depth-first preorder: parents before children, siblings by id.
    let order = preorder(&found);
    found.sort_by_key(|record| order[record.record_ref.as_str()]);
    Ok((found, declared))
}

/// The preorder position of every record: a parent, then its children by node id.
fn preorder(found: &[Found]) -> BTreeMap<&str, usize> {
    let mut by_parent: BTreeMap<Option<&str>, Vec<&str>> = BTreeMap::new();
    for record in found {
        by_parent
            .entry(record.parent_ref.as_deref())
            .or_default()
            .push(record.record_ref.as_str());
    }
    for siblings in by_parent.values_mut() {
        siblings.sort_unstable();
    }
    let mut order = BTreeMap::new();
    let mut stack: Vec<&str> = by_parent.get(&None).cloned().unwrap_or_default();
    stack.reverse();
    while let Some(current) = stack.pop() {
        let position = order.len();
        order.insert(current, position);
        if let Some(children) = by_parent.get(&Some(current)) {
            stack.extend(children.iter().rev());
        }
    }
    order
}

/// The properties a profile lets through, or `None` when it carries only the digest.
fn visible_properties(profile: RedactionProfile, props: &Props) -> Option<Props> {
    match profile {
        RedactionProfile::DigestsOnly => None,
        RedactionProfile::StripContent => Some(
            props
                .iter()
                .filter(|(key, _)| !CONTENT_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        RedactionProfile::None => Some(props.clone()),
    }
}

/// `sha256:<hex>` over the ordered record refs.
fn snapshot_digest(refs: &[&str]) -> String {
    let mut bytes = Vec::new();
    for record_ref in refs {
        bytes.extend_from_slice(record_ref.as_bytes());
        bytes.push(b'\n');
    }
    format!("sha256:{}", Digest256::sha256(&bytes).to_hex())
}

/// One page of the export; the pure core of [`handle_run_trace_export`].
pub(crate) fn export_page(
    core: &GraphCore,
    caller: &str,
    request: &RunTraceExportV1,
    page_size_max: u64,
) -> Result<RunTraceExportPageV1, String> {
    if request.schema_version != RUN_TRACE_EXPORT_SCHEMA_VERSION {
        return Err(invalid(format!(
            "schema_version {} is not {RUN_TRACE_EXPORT_SCHEMA_VERSION}",
            request.schema_version
        )));
    }
    if request.root_run_id.is_empty() {
        return Err(invalid("root_run_id is required"));
    }
    if let Some(window) = request.time_range {
        if window.from_ms > window.to_ms {
            return Err(invalid("time_range.from_ms is after time_range.to_ms"));
        }
    }
    let size = match request.page_size {
        None => page_size_max,
        Some(0) => return Err(invalid("page_size must be at least 1")),
        Some(size) => u64::from(size).min(page_size_max),
    } as usize;
    // The revision pins the read: a caller that pinned another one reads a different graph.
    let revision = core.version();
    if request.source_revision.is_some_and(|pinned| pinned != revision) {
        return Err(format!(
            "{EXPORT_CURSOR_STALE}: the export is pinned to revision {} and the graph is at {revision}",
            request.source_revision.unwrap_or_default()
        ));
    }
    let (found, declared) = walk(core, &request.root_run_id)?;
    // `none` hands out every property, so only the trace's owner may ask for it.
    if request.redaction_profile == RedactionProfile::None {
        let owner = found[0]
            .props
            .get("_owner_id")
            .and_then(serde_json::Value::as_str);
        if owner != Some(caller) {
            return Err(
                "ACCESS_DENIED: the none redaction profile needs the trace-owner principal"
                    .to_string(),
            );
        }
    }
    let in_window = |record: &Found| {
        request.time_range.is_none_or(|window| {
            started_at_ms(&record.props)
                .is_some_and(|at| (window.from_ms..=window.to_ms).contains(&at))
        })
    };
    let selected: Vec<&Found> = found.iter().filter(|record| in_window(record)).collect();
    let refs: Vec<&str> = selected.iter().map(|record| record.record_ref.as_str()).collect();
    let digest = snapshot_digest(&refs);

    let offset = match request.page_token.as_deref() {
        None => 0,
        Some(token) => {
            let (token_digest, at) = token
                .rsplit_once(':')
                .ok_or_else(|| format!("{EXPORT_CURSOR_STALE}: the page token is malformed"))?;
            if token_digest != digest {
                return Err(format!(
                    "{EXPORT_CURSOR_STALE}: the trace changed since the page token was issued"
                ));
            }
            at.parse::<usize>()
                .ok()
                .filter(|at| *at <= selected.len())
                .ok_or_else(|| format!("{EXPORT_CURSOR_STALE}: the page token is out of range"))?
        }
    };
    let end = (offset + size).min(selected.len());
    let records: Vec<RunTraceRecord> = selected[offset..end]
        .iter()
        .map(|record| RunTraceRecord {
            record_ref: record.record_ref.clone(),
            kind: record.kind,
            parent_ref: record.parent_ref.clone(),
            started_at_ms: started_at_ms(&record.props),
            content_digest: digest_of(&record.props),
            properties: visible_properties(request.redaction_profile, &record.props),
        })
        .collect();

    let present: BTreeSet<&str> = found
        .iter()
        .filter(|record| record.kind == RunTraceRecordKind::Span)
        .map(|record| record.record_ref.as_str())
        .collect();
    let missing_children: Vec<String> = declared
        .iter()
        .filter(|id| !present.contains(id.as_str()))
        .cloned()
        .collect();
    let complete = missing_children.is_empty();
    Ok(RunTraceExportPageV1 {
        schema_version: RUN_TRACE_EXPORT_SCHEMA_VERSION,
        root_run_id: request.root_run_id.clone(),
        source_revision: revision,
        redaction_profile: request.redaction_profile,
        records,
        complete,
        refusal: (!complete).then(|| EXPORT_CHILD_MISSING.to_string()),
        missing_children,
        snapshot_digest: digest.clone(),
        next_page_token: (end < selected.len()).then(|| format!("{digest}:{end}")),
    })
}

/// Serve one `RunTraceExport` request over the caller's projection of the request graph.
pub(crate) fn handle_run_trace_export(
    ctx: &QueryHandlerCtx<'_>,
    request: RunTraceExportV1,
) -> Response {
    let Some(authority) = ctx
        .read_authority
        .filter(|authority| authority.carrier().is_some())
    else {
        return Response::err(
            ctx.req_id,
            "ACCESS_DENIED: a run trace export needs a verified caller",
        );
    };
    let projected = authority.project_core(ctx.core);
    let page_size_max = medulla_server_api::knob_config::knobs().orchestration_limits_page_size_max;
    match export_page(
        &projected,
        ctx.caller,
        &request,
        page_size_max,
    ) {
        Ok(page) => Response::ok(ctx.req_id, ResultPayload::of::<RunTraceExport>(page)),
        Err(message) => Response::err(ctx.req_id, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::run_trace_export::TimeRange;

    fn put(core: &GraphCore, id: &str, label: &str, extra: serde_json::Value) {
        let mut props = serde_json::json!({"node_type": label});
        props
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
    }

    fn link(core: &GraphCore, from: &str, to: &str, relation: &str) {
        core.add_typed_edge(
            from.to_string(),
            to.to_string(),
            relation.to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({"relationship": relation})).unwrap(),
        )
        .unwrap();
    }

    /// One parent run with a child agent run, two spans and one tool call.
    fn trace() -> GraphCore {
        let core = GraphCore::new();
        put(
            &core,
            "run:parent",
            "RunTrace",
            serde_json::json!({
                "_owner_id": "owner",
                "started_at_ms": 100,
                "declaredSpans": ["span:a", "span:b"],
            }),
        );
        put(&core, "run:child", "RunTrace", serde_json::json!({"started_at_ms": 150}));
        put(&core, "span:a", "Span", serde_json::json!({"started_at_ms": 110, "output": "secret"}));
        put(&core, "span:b", "Span", serde_json::json!({"started_at_ms": 120}));
        put(&core, "call:1", "ToolCall", serde_json::json!({"started_at_ms": 130, "tool_name": "t"}));
        link(&core, "run:parent", "run:child", EDGE_HAS_CHILD_RUN);
        link(&core, "run:parent", "span:a", EDGE_HAS_SPAN);
        link(&core, "run:parent", "span:b", EDGE_HAS_SPAN);
        link(&core, "run:child", "call:1", EDGE_HAS_TOOL_CALL);
        core
    }

    fn ask() -> RunTraceExportV1 {
        serde_json::from_str(r#"{"root_run_id":"run:parent"}"#).unwrap()
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.100
    /// Verifies MEDULLA-ORCH-VOCAB-R009.100: a parent with children exports complete, in
    /// depth-first order, and the snapshot digest is the same on every read.
    #[test]
    fn parent_with_children_exports_complete_and_stable() {
        let core = trace();
        let first = export_page(&core, "owner", &ask(), 100).unwrap();
        let refs: Vec<&str> = first.records.iter().map(|r| r.record_ref.as_str()).collect();
        // Parent first, then each child by node id with its own children right after it.
        assert_eq!(refs, ["run:parent", "run:child", "call:1", "span:a", "span:b"]);
        assert!(first.complete && first.refusal.is_none() && first.missing_children.is_empty());
        assert_eq!(first.records[0].kind, RunTraceRecordKind::ParentRun);
        assert!(first.records.iter().all(|r| r.properties.is_none()), "digests only");
        let again = export_page(&core, "owner", &ask(), 100).unwrap();
        assert_eq!(first.snapshot_digest, again.snapshot_digest);
        assert!(first.snapshot_digest.starts_with("sha256:"));
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.100
    /// Verifies MEDULLA-ORCH-VOCAB-R009.100: a declared span the walk did not reach makes the
    /// page incomplete with `EXPORT_CHILD_MISSING`, never a silent gap.
    #[test]
    fn a_declared_span_that_is_missing_makes_the_page_incomplete() {
        let core = trace();
        core.remove_node("span:b".to_string());
        let page = export_page(&core, "owner", &ask(), 100).unwrap();
        assert!(!page.complete);
        assert_eq!(page.refusal.as_deref(), Some(EXPORT_CHILD_MISSING));
        assert_eq!(page.missing_children, ["span:b"]);
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.100
    /// Verifies MEDULLA-ORCH-VOCAB-R009.100: pages are bounded, a token carries the snapshot
    /// digest, and a trace that changed refuses `EXPORT_CURSOR_STALE`.
    #[test]
    fn pages_are_bounded_and_a_stale_token_refuses() {
        let core = trace();
        let mut request = ask();
        request.page_size = Some(2);
        let first = export_page(&core, "owner", &request, 100).unwrap();
        assert_eq!(first.records.len(), 2);
        let token = first.next_page_token.clone().expect("more pages");
        request.page_token = Some(token.clone());
        let second = export_page(&core, "owner", &request, 100).unwrap();
        assert_eq!(second.records.len(), 2);
        assert_eq!(first.snapshot_digest, second.snapshot_digest);
        // A larger page size is clamped to the registry ceiling.
        request.page_token = None;
        request.page_size = Some(900);
        assert_eq!(export_page(&core, "owner", &request, 3).unwrap().records.len(), 3);
        // The trace changed after the token was issued.
        put(&core, "span:c", "Span", serde_json::json!({}));
        link(&core, "run:parent", "span:c", EDGE_HAS_SPAN);
        request.page_size = Some(2);
        request.page_token = Some(token);
        let stale = export_page(&core, "owner", &request, 100).unwrap_err();
        assert!(stale.starts_with(EXPORT_CURSOR_STALE), "{stale}");
        // A pinned revision that is no longer current refuses the same way.
        let mut pinned = ask();
        pinned.source_revision = Some(core.version() + 1);
        let stale = export_page(&core, "owner", &pinned, 100).unwrap_err();
        assert!(stale.starts_with(EXPORT_CURSOR_STALE), "{stale}");
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.100
    /// Verifies MEDULLA-ORCH-VOCAB-R009.100: a span reached from two parents refuses
    /// `EXPORT_SPAN_DUPLICATE`, and only the declared child edges are followed.
    #[test]
    fn a_duplicate_span_refuses_and_other_edges_are_not_followed() {
        let core = trace();
        link(&core, "run:child", "span:a", EDGE_HAS_SPAN);
        let refusal = export_page(&core, "owner", &ask(), 100).unwrap_err();
        assert!(refusal.starts_with(EXPORT_SPAN_DUPLICATE), "{refusal}");

        let core = trace();
        put(&core, "span:x", "Span", serde_json::json!({}));
        link(&core, "run:parent", "span:x", "relatedTo");
        let page = export_page(&core, "owner", &ask(), 100).unwrap();
        assert!(page.records.iter().all(|r| r.record_ref != "span:x"));
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.100
    /// Verifies MEDULLA-ORCH-VOCAB-R009.100: `strip_content` removes bodies, `none` needs the
    /// trace owner, and the window keeps only records that started inside it.
    #[test]
    fn redaction_profiles_and_window() {
        let core = trace();
        let mut request = ask();
        request.redaction_profile = RedactionProfile::StripContent;
        let page = export_page(&core, "someone", &request, 100).unwrap();
        let span = page.records.iter().find(|r| r.record_ref == "span:a").unwrap();
        let props = span.properties.as_ref().expect("structure kept");
        assert!(props.contains_key("started_at_ms") && !props.contains_key("output"));

        request.redaction_profile = RedactionProfile::None;
        let refusal = export_page(&core, "someone", &request, 100).unwrap_err();
        assert!(refusal.starts_with("ACCESS_DENIED"), "{refusal}");
        let owned = export_page(&core, "owner", &request, 100).unwrap();
        let span = owned.records.iter().find(|r| r.record_ref == "span:a").unwrap();
        assert_eq!(span.properties.as_ref().unwrap()["output"], "secret");

        request.redaction_profile = RedactionProfile::DigestsOnly;
        request.time_range = Some(TimeRange { from_ms: 105, to_ms: 125 });
        let windowed = export_page(&core, "owner", &request, 100).unwrap();
        let refs: Vec<&str> = windowed.records.iter().map(|r| r.record_ref.as_str()).collect();
        assert_eq!(refs, ["span:a", "span:b"]);

        request.time_range = Some(TimeRange { from_ms: 9, to_ms: 1 });
        assert!(export_page(&core, "owner", &request, 100).unwrap_err().starts_with("INVALID_ARGUMENT"));
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.100
    /// Verifies MEDULLA-ORCH-VOCAB-R009.100: an unknown or non-trace root reads as absent.
    #[test]
    fn an_unknown_root_reads_as_absent() {
        let core = trace();
        let mut request = ask();
        request.root_run_id = "span:a".to_string();
        let refusal = export_page(&core, "owner", &request, 100).unwrap_err();
        assert!(refusal.starts_with(RUN_TRACE_NOT_FOUND), "{refusal}");
        request.root_run_id = "run:nobody".to_string();
        let refusal = export_page(&core, "owner", &request, 100).unwrap_err();
        assert!(refusal.starts_with(RUN_TRACE_NOT_FOUND), "{refusal}");
    }
}
