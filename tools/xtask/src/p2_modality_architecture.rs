//! P2 modality / KnowledgeBatch architecture gate.
//!
//! Only properties the compiler and the Rust suites cannot see: bans on retired or forbidden shapes
//! (a legacy handler module, no-op runtimes, TCK exemptions, no-op codec features, the retired
//! compatibility projection, a source-bearing receipt digest, a source-bearing Raft field); the
//! graph-dispatch ordering contract (graph ACL and placement resolve before KnowledgeStream can be
//! routed), expressed over the call graph; and the cross-language Python client / server
//! ingest-stream item bound. Everything is matched on identifiers over comment/string-masked Rust,
//! so a rename or reformat of unrelated code never needs this file edited.

use std::path::Path;

use regex::Regex;

use crate::analysis::callgraph::{reachable_source, top_level_fns};
use crate::analysis::mask::{balanced_span_from, code_mask, comments_mask};
use crate::analysis::{module_tree, read};

pub const NAME: &str = "p2-modality-architecture";
pub const SUMMARY: &str = "P2 modality/KnowledgeBatch architecture is not bypassed";

const MODALITIES: [&str; 4] = ["document", "image", "audio", "video"];
const RETIRED_MODALITY_FEATURES: [&str; 2] = ["codec", "extract"];

const GRAPH_DISPATCH: &str = "crates/medulla-server-graph/src/dispatch.rs";
const MUTATION: &str = "crates/medulla-server-mutation/src/mutation.rs";
const RAFT: &str = "crates/medulla-raft/src/raft/mod.rs";
const WIRE: &str = "crates/medulla-types/src/knowledge_stream.rs";
const INGEST: &str = "crates/medulla-server-ops/src/api/ingest.rs";
const PYTHON_CLIENT: &str = "medulla/client.py";

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    check(root).map_err(|message| format!("P2 architecture gate failed: {message}"))?;
    println!("P2 modality architecture gate passed");
    Ok(())
}

/// The first violated contract, if any.
pub fn check(root: &Path) -> Result<(), String> {
    require_single_knowledge_stream_handler(root)?;
    require_modality_crates(root)?;
    require_no_compatibility_projection(&compiler_source(root, WIRE)?)?;
    require_graph_dispatch_ordering(&module_tree::read(root, GRAPH_DISPATCH, false)?)?;
    require_source_free_receipt(&module_tree::read(root, MUTATION, false)?)?;
    require_modality_raft_replication(root)?;
    require_ingest_stream_item_bound(root, &read(root, PYTHON_CLIENT)?)
}

/// A Rust module's compiler-reachable source tree, test-inclusive.
fn compiler_source(root: &Path, relative: &str) -> Result<String, String> {
    module_tree::read(root, relative, true)
}

/// Whether an identifier regex matches whole words of Rust *code*.
fn has_identifier(source: &str, pattern: &str) -> Result<bool, String> {
    let code = code_mask(source)?;
    let re = Regex::new(&format!(r"(?:^|\W)(?:{pattern})(?:\W|$)")).map_err(|e| e.to_string())?;
    Ok(re.is_match(&code))
}

fn require_single_knowledge_stream_handler(root: &Path) -> Result<(), String> {
    if root
        .join("crates/medulla-server-graph/src/handlers/knowledge_stream.rs")
        .exists()
    {
        return Err("KnowledgeStream retains an ambiguous legacy handler module".into());
    }
    Ok(())
}

fn require_native_runtime_contract(modality: &str, runtime: &str) -> Result<(), String> {
    if has_identifier(runtime, r"\w*Noop\w*")? {
        return Err(format!("{modality} runtime still contains a no-op"));
    }
    Ok(())
}

fn require_no_tck_exemption(modality: &str, contract: &str) -> Result<(), String> {
    if has_identifier(contract, "tck_not_applicable")? {
        return Err(format!(
            "{modality} still exempts a production TCK dimension"
        ));
    }
    Ok(())
}

fn require_no_noop_features(modality: &str, cargo_toml: &str) -> Result<(), String> {
    let doc: toml_edit::DocumentMut = cargo_toml
        .parse()
        .map_err(|e| format!("{modality} Cargo.toml: {e}"))?;
    let retired = doc
        .get("features")
        .and_then(|f| f.as_table_like())
        .map(|features| {
            RETIRED_MODALITY_FEATURES
                .iter()
                .any(|name| features.contains_key(name))
        })
        .unwrap_or(false);
    if retired {
        return Err(format!(
            "{modality} retains a no-op codec/extractor feature"
        ));
    }
    Ok(())
}

fn require_modality_crates(root: &Path) -> Result<(), String> {
    for modality in MODALITIES {
        let krate = format!("crates/medulla-{modality}");
        require_native_runtime_contract(
            modality,
            &compiler_source(root, &format!("{krate}/src/runtime.rs"))?,
        )?;
        require_no_tck_exemption(
            modality,
            &compiler_source(root, &format!("{krate}/src/contract.rs"))?,
        )?;
        require_no_noop_features(modality, &read(root, &format!("{krate}/Cargo.toml"))?)?;
    }
    Ok(())
}

fn require_no_compatibility_projection(wire: &str) -> Result<(), String> {
    if has_identifier(wire, "CompatibilityMsgpackV1")? {
        return Err("retired KnowledgeStream compatibility projection is still present".into());
    }
    Ok(())
}

/// Offset of a call to `name` in `body` (word-boundary matched).
fn call_offset(body: &str, name: &str) -> Option<usize> {
    Regex::new(&format!(r"\b{name}\s*\("))
        .ok()?
        .find(body)
        .map(|m| m.start())
}

/// Graph ACL, then placement, then routing, as an order of CALLS.
///
/// `dispatch_graph_op_inner` must call the ACL capture, then placement resolution, then the
/// post-lock method router; nothing it calls before the router may reach KnowledgeStream handling,
/// and the ACL capture must still reach the graph ACL check. Resolved over the call graph, so
/// decomposing a helper does not matter.
fn require_graph_dispatch_ordering(dispatch: &str) -> Result<(), String> {
    let code = code_mask(dispatch)?;
    let fns = top_level_fns(&code);
    let inner = fns
        .get("dispatch_graph_op_inner")
        .cloned()
        .unwrap_or_default();
    if inner.is_empty() {
        return Err("dispatch_graph_op_inner is absent from dispatch.rs".into());
    }
    let acl = call_offset(&inner, "capture_graph_dispatch");
    let placement = call_offset(&inner, "resolve_routed_raft");
    let routing = call_offset(&inner, "route_graph_op_method");
    let ordered =
        matches!((acl, placement, routing), (Some(a), Some(p), Some(r)) if p > a && r > p);
    if !ordered {
        return Err("KnowledgeStream is routed before graph ACL/placement semantics".into());
    }
    if call_offset(
        &reachable_source(&code, "capture_graph_dispatch"),
        "check_graph_access",
    )
    .is_none()
    {
        return Err("the graph ACL gate no longer performs the graph ACL check".into());
    }
    let knowledge_stream = Regex::new(r"\bKnowledgeStream\b").expect("static regex");
    let routing = routing.expect("checked above");
    let body_start = inner.find('{').unwrap_or(0);
    let pre_route = &inner[body_start..routing];
    let call = Regex::new(r"(\w+)\s*\(").expect("static regex");
    let reaches_stream = call
        .captures_iter(pre_route)
        .map(|c| c[1].to_string())
        .filter(|name| fns.contains_key(name))
        .any(|name| knowledge_stream.is_match(&reachable_source(&code, &name)));
    if knowledge_stream.is_match(pre_route)
        || reaches_stream
        || !knowledge_stream.is_match(&reachable_source(&code, "route_graph_op_method"))
    {
        return Err("KnowledgeStream is routed outside the post-lock router, so it no longer sits behind graph ACL and placement resolution".into());
    }
    Ok(())
}

/// The durable receipt must not digest the source-bearing wire body.
fn require_source_free_receipt(mutation: &str) -> Result<(), String> {
    let receipt = reachable_source(&code_mask(mutation)?, "durable_receipt_method");
    if receipt.is_empty() {
        return Err("durable_receipt_method is absent".into());
    }
    if call_offset(&receipt, "canonical_body_bytes").is_some() {
        return Err(
            "modality receipt retains a direct digest of the source-bearing wire body".into(),
        );
    }
    Ok(())
}

/// (attributes, body) of the sole `SanitizedModalityRaftCommand` struct.
fn sanitized_command(raft: &str) -> Result<(String, String), String> {
    let code = code_mask(raft)?;
    let text = comments_mask(raft)?;
    let re = Regex::new(
        r"((?:#\[(?:[^\[\]]|\[[^\]]*\])*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?struct\s+SanitizedModalityRaftCommand\b",
    )
    .expect("static regex");
    let absent = || "the current sanitized modality Raft command is absent".to_string();
    let matches: Vec<_> = re.captures_iter(&code).collect();
    if matches.len() != 1 {
        return Err(absent());
    }
    let whole = matches[0].get(0).expect("whole match");
    let attrs = matches[0].get(1).expect("attribute group");
    let opener = code[whole.end()..]
        .find('{')
        .map(|n| whole.end() + n)
        .ok_or_else(absent)?;
    let closer = balanced_span_from(&code, opener, b'{', b'}')?;
    Ok((
        text[attrs.start()..attrs.end()].to_string(),
        code[opener + 1..closer].to_string(),
    ))
}

/// The replicated modality command is closed and carries no source field.
fn require_modality_raft_replication(root: &Path) -> Result<(), String> {
    let raft = module_tree::compiler_family(root, RAFT)?.production;
    let (attrs, body) = sanitized_command(&raft)?;
    let field = Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(\w+)\s*:").expect("static regex");
    let source_named = Regex::new(r"(?:^|_)source(?:_|$)").expect("static regex");
    let source_field = field
        .captures_iter(&body)
        .any(|c| source_named.is_match(&c[1]));
    if !attrs.contains("deny_unknown_fields") || source_field {
        return Err("Raft modality command is not closed and source-free".into());
    }
    Ok(())
}

/// The client's `ingest_stream` item ceiling equals the server's. Cross-language, so no compiler
/// sees it: the server bound moved 61 -> 49 once while the client kept validating against 64.
fn require_ingest_stream_item_bound(root: &Path, python_client: &str) -> Result<(), String> {
    let client = Regex::new(r"\blen\(\s*items\s*\)\s*<=\s*(\d+)").expect("static regex");
    let mut bounds: Vec<String> = client
        .captures_iter(python_client)
        .map(|c| c[1].to_string())
        .collect();
    bounds.sort();
    bounds.dedup();
    if bounds.len() != 1 {
        return Err("Python client ingest_stream item bound is absent or inconsistent".into());
    }
    let server = comments_mask(&compiler_source(root, INGEST)?)?;
    let server_re =
        Regex::new(r"\bMAX_INGEST_STREAM_ITEMS\s*:\s*usize\s*=\s*(\d+)").expect("static regex");
    let server_bound = server_re
        .captures(&server)
        .ok_or("server MAX_INGEST_STREAM_ITEMS not found")?;
    if bounds[0] != server_bound[1] {
        return Err(format!(
            "Python client ingest_stream bound ({}) has drifted from the server's MAX_INGEST_STREAM_ITEMS ({})",
            bounds[0], &server_bound[1]
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;
    use crate::workspace_root;

    fn replace_once(source: &str, old: &str, new: &str) -> String {
        assert_eq!(
            source.matches(old).count(),
            1,
            "fixture target is not unique: {old}"
        );
        source.replacen(old, new, 1)
    }

    fn graph_dispatch() -> String {
        module_tree::read(&workspace_root(), GRAPH_DISPATCH, false).unwrap()
    }

    #[test]
    fn dispatch_ordering_follows_graph_pipeline_children() {
        let dispatch = graph_dispatch();
        require_graph_dispatch_ordering(&dispatch).unwrap();
        let broken = dispatch.replacen(
            "capture_graph_dispatch(state, ctx",
            "capture_without_acl(state, ctx",
            1,
        );
        assert!(require_graph_dispatch_ordering(&broken)
            .unwrap_err()
            .contains("routed before graph ACL"));
    }

    #[test]
    fn dispatch_ordering_rejects_knowledge_stream_before_the_router() {
        let broken = replace_once(
            &graph_dispatch(),
            "    stamp_resource_and_capacity_timestamps(&mut method);\n",
            "    stamp_resource_and_capacity_timestamps(&mut method);\n    if matches!(&method, Method::KnowledgeStream { .. }) {}\n",
        );
        assert!(require_graph_dispatch_ordering(&broken)
            .unwrap_err()
            .contains("outside the post-lock router"));
    }

    const VALID_RAFT_CHILD: &str = "#[cfg(feature = \"modality-serving\")]\n#[derive(Serialize, Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct SanitizedModalityRaftCommand {\n    pub(crate) node_id: String,\n    #[serde(with = \"serde_bytes\")]\n    pub(crate) sealed_runtime_state: Vec<u8>,\n}\n";

    fn raft_fixture(child: &str, declare: bool) -> Tree {
        let module = format!(
            "{}pub struct RaftRequest;\n",
            if declare { "mod modality;\n" } else { "" }
        );
        Tree::new(&[
            ("crates/medulla-raft/src/raft/mod.rs", &module),
            ("crates/medulla-raft/src/raft/modality.rs", child),
        ])
    }

    #[test]
    fn raft_replication_reads_declared_child_and_rejects_orphans() {
        let t = raft_fixture(VALID_RAFT_CHILD, true);
        require_modality_raft_replication(t.path()).unwrap();
        std::fs::write(t.path().join(RAFT), "pub struct RaftRequest;\n").unwrap();
        assert!(require_modality_raft_replication(t.path())
            .unwrap_err()
            .contains("orphan Rust module files"));
    }

    #[test]
    fn raft_replication_rejects_missing_declared_child() {
        let t = raft_fixture(VALID_RAFT_CHILD, true);
        std::fs::remove_file(t.path().join("crates/medulla-raft/src/raft/modality.rs")).unwrap();
        assert!(require_modality_raft_replication(t.path())
            .unwrap_err()
            .contains("exactly one file"));
    }

    #[test]
    fn raft_replication_rejects_comment_only_modality_contract() {
        let t = raft_fixture(&format!("/*\n{VALID_RAFT_CHILD}*/\n"), true);
        assert!(require_modality_raft_replication(t.path())
            .unwrap_err()
            .contains("current sanitized modality Raft command"));
    }

    #[test]
    fn raft_replication_rejects_an_open_or_source_bearing_command() {
        for (old, new) in [
            (
                "    pub(crate) node_id: String,\n",
                "    pub(crate) node_id: String,\n    source_bytes: Vec<u8>,\n",
            ),
            ("#[serde(deny_unknown_fields)]\n", ""),
        ] {
            let t = raft_fixture(&replace_once(VALID_RAFT_CHILD, old, new), true);
            assert!(require_modality_raft_replication(t.path())
                .unwrap_err()
                .contains("not closed and source-free"));
        }
    }

    #[test]
    fn raft_replication_passes_on_the_current_tree() {
        require_modality_raft_replication(&workspace_root()).unwrap();
    }

    #[test]
    fn modality_guard_reads_runtime_children_and_rejects_a_noop() {
        let t = Tree::new(&[
            ("crates/medulla-video/src/runtime.rs", "mod probe;\npub struct NativeVideoRuntime;\n"),
            (
                "crates/medulla-video/src/runtime/probe.rs",
                "// a NoopRuntime used to live here\nconst NOTE: &str = \"NoopRuntime\";\npub(super) fn production_probe() {}\n",
            ),
        ]);
        let runtime = "crates/medulla-video/src/runtime.rs";
        require_native_runtime_contract("video", &compiler_source(t.path(), runtime).unwrap())
            .unwrap();
        std::fs::write(
            t.path().join("crates/medulla-video/src/runtime/probe.rs"),
            "pub(super) struct NoopRuntime;\n",
        )
        .unwrap();
        let source = compiler_source(t.path(), runtime).unwrap();
        assert!(require_native_runtime_contract("video", &source)
            .unwrap_err()
            .contains("video runtime still contains a no-op"));
    }

    #[test]
    fn modality_guard_rejects_tck_exemptions_and_noop_features() {
        require_modality_crates(&workspace_root()).unwrap();
        assert!(
            require_no_tck_exemption("audio", "fn tck_not_applicable() {}")
                .unwrap_err()
                .contains("exempts a production TCK")
        );
        assert!(
            require_no_noop_features("audio", "[features]\ncodec = []\n")
                .unwrap_err()
                .contains("no-op codec/extractor")
        );
        require_no_noop_features("audio", "[features]\nother = []\n").unwrap();
    }

    #[test]
    fn knowledge_stream_wire_rejects_the_retired_projection() {
        let wire = compiler_source(&workspace_root(), WIRE).unwrap();
        require_no_compatibility_projection(&wire).unwrap();
        let spoiled = format!("{wire}\nenum P {{ CompatibilityMsgpackV1 }}\n");
        assert!(require_no_compatibility_projection(&spoiled)
            .unwrap_err()
            .contains("compatibility projection"));
    }

    #[test]
    fn receipt_rejects_a_source_body_digest() {
        let mutation = module_tree::read(&workspace_root(), MUTATION, false).unwrap();
        require_source_free_receipt(&mutation).unwrap();
        let broken = format!("{mutation}\nfn durable_receipt_method(method: &Method) -> Method {{\n    canonical_body_bytes(method)\n}}\n");
        assert!(require_source_free_receipt(&broken)
            .unwrap_err()
            .contains("source-bearing wire body"));
    }

    #[test]
    fn ingest_stream_client_bound_matches_and_rejects_drift() {
        let root = workspace_root();
        let client = read(&root, PYTHON_CLIENT).unwrap();
        require_ingest_stream_item_bound(&root, &client).unwrap();
        let drifted = replace_once(
            &client,
            "not 2 <= len(items) <= 49",
            "not 2 <= len(items) <= 61",
        );
        assert!(require_ingest_stream_item_bound(&root, &drifted)
            .unwrap_err()
            .contains("has drifted from the server's"));
        let missing = drifted.replacen("len(items) <= 61", "len(items) < limit", 1);
        assert!(require_ingest_stream_item_bound(&root, &missing)
            .unwrap_err()
            .contains("absent or inconsistent"));
    }

    #[test]
    fn single_handler_and_whole_gate_pass_on_the_current_tree() {
        check(&workspace_root()).unwrap();
    }
}
