use medulla_h_ingest::repository_index::repository_envelope;
// EH-280 — the lowered write-set is deterministic and tombstones remove.

use std::collections::HashMap;

use medulla_protocol_types::change_envelope::MaterialClass;
use medulla_types::ingestion_wire::{ExtractedEdge, ExtractedNode, IndexResult};

use medulla_h_ingest::repository_index::{lower, BATCH_TOO_LARGE, REDACTED_HOST_IDENTITY};
use medulla_protocol_types::protocol::Method;

fn node(id: &str, kind: &str, properties: &[(&str, &str)]) -> ExtractedNode {
    ExtractedNode {
        node_id: id.to_string(),
        node_type: kind.to_string(),
        properties: properties
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    }
}

fn edge(source: &str, target: &str, kind: &str) -> ExtractedEdge {
    ExtractedEdge {
        source: source.to_string(),
        target: target.to_string(),
        edge_type: kind.to_string(),
        properties: HashMap::new(),
    }
}

fn result() -> IndexResult {
    IndexResult {
        nodes: vec![
            node(
                "branch:b",
                "Branch",
                &[("ref_name", "main"), ("status", "live")],
            ),
            node(
                "fileversion:f",
                "FileVersion",
                &[("path", "a.py"), ("content_digest", "d")],
            ),
        ],
        edges: vec![
            edge("branch:b", "fileversion:f", "hasFileVersion"),
            edge("branch:b", "fileversion:old", "removesFileVersion"),
            edge("symbol:a", "symbol:b", "calls"),
            edge("symbol:a", "symbol:b", "similar_to"),
        ],
        ..Default::default()
    }
}

#[test]
fn write_set_is_deterministic_across_property_order() {
    let first = lower(&result()).expect("lowered");
    for _ in 0..8 {
        // A fresh HashMap may iterate in another order; the id must not move.
        assert_eq!(
            lower(&result()).expect("lowered").envelope_id,
            first.envelope_id
        );
    }
    assert!(first.envelope_id.starts_with("repository-index:"));
}

#[test]
fn tombstones_lower_to_membership_removal() {
    let lowered = lower(&result()).expect("lowered");
    assert!(lowered.methods.iter().any(|method| matches!(
        method,
        Method::RemoveEdge { source_id, target_id }
            if source_id == "branch:b" && target_id == "fileversion:old"
    )));
}

// Supplementary source evidence for MEDULLA-REPO-INGEST-R009. The served hydration
// acceptance still needs the source-to-specification readback; this regression
// protects its existing typed relation projection from endpoint-pair loss.
#[test]
fn one_edge_per_relation_triple_keeps_sibling_relationships_and_deduplicates_replays() {
    let mut input = result();
    input.edges.push(edge("symbol:a", "symbol:b", "calls"));
    let lowered = lower(&input).expect("lowered");
    let symbol_edges: Vec<&Method> = lowered
        .methods
        .iter()
        .filter(
            |method| matches!(method, Method::AddEdge { source_id, .. } if source_id == "symbol:a"),
        )
        .collect();
    assert_eq!(symbol_edges.len(), 2);
    let relationships: std::collections::BTreeSet<String> = symbol_edges
        .iter()
        .map(|method| {
            let Method::AddEdge {
                properties_msgpack, ..
            } = method
            else {
                unreachable!("filtered to AddEdge");
            };
            let properties: std::collections::BTreeMap<String, String> =
                rmp_serde::from_slice(properties_msgpack).expect("edge properties");
            properties["relationship"].clone()
        })
        .collect();
    assert_eq!(
        relationships,
        std::collections::BTreeSet::from(["calls".to_string(), "similar_to".to_string()])
    );
}

/// A repository shaped like real code: a decorated class in `app/home/views.py`.
fn code_result(extra: &[(&str, &str)]) -> IndexResult {
    let mut symbol = vec![("name", "Settings"), ("decorators", "@dataclass")];
    symbol.extend_from_slice(extra);
    IndexResult {
        nodes: vec![
            node(
                "fileversion:v",
                "FileVersion",
                &[("path", "app/home/views.py")],
            ),
            node("symbol:s", "SYMBOL", &symbol),
        ],
        edges: vec![edge("fileversion:v", "symbol:s", "IMPLEMENTS")],
        ..Default::default()
    }
}

#[test]
fn repository_code_passes_the_repository_content_rule() {
    let envelope = repository_envelope(&code_result(&[])).expect("real code is repository content");
    assert_eq!(envelope.material_class, MaterialClass::RepositorySnapshot);
}

/// Attested material admits `@` and a relative `home/` segment as ordinary
/// text (MEDULLA-DURABLE-KERNEL-R043.7): the same code is not refused for privacy
/// under the attested rule either.
#[test]
fn the_same_code_is_not_a_privacy_refusal_under_the_attested_rule() {
    let mut envelope = repository_envelope(&code_result(&[])).expect("repository content");
    envelope.material_class = MaterialClass::Attested;
    if let Err(error) = envelope.validate() {
        assert!(!error.contains("persistence privacy policy"), "{error}");
    }
}

/// Host identity quoted inside repository content (a `pyproject.toml` author,
/// a docstring path) never reaches the store, and does not refuse the batch:
/// the value is redacted at lowering, and the envelope's repository rule
/// (unchanged) still screens every committed value.
#[test]
fn host_identity_inside_repository_content_is_redacted_not_stored() {
    for leak in ["/home/alice/project/app.py", "maintainer dev@example.com"] {
        let envelope = repository_envelope(&code_result(&[("doc", leak)]))
            .expect("the batch commits without the host identity");
        let stored = rmp_serde::to_vec_named(&envelope.mutation.operations).unwrap();
        let rendered = String::from_utf8_lossy(&stored);
        assert!(!rendered.contains(leak), "{leak} must not be stored");
        assert!(rendered.contains(REDACTED_HOST_IDENTITY));
    }
}

/// A repository-sized result: `symbols` SYMBOL nodes, each implemented by one
/// file version (two operations per symbol).
fn large_result(symbols: usize) -> IndexResult {
    let nodes = (0..symbols)
        .map(|index| node(&format!("symbol:{index}"), "SYMBOL", &[("name", "f")]))
        .collect();
    let edges = (0..symbols)
        .map(|index| edge("fileversion:v", &format!("symbol:{index}"), "IMPLEMENTS"))
        .collect();
    IndexResult {
        nodes,
        edges,
        ..Default::default()
    }
}

/// A real repository batch (24 000 operations) commits as ONE atomic envelope:
/// its projection notice is a bounded summary, not one event per operation,
/// so the envelope's inline-material bound (unchanged) admits it.
#[test]
fn a_repository_sized_batch_is_one_envelope_with_a_bounded_notice() {
    let envelope = repository_envelope(&large_result(12_000)).expect("one atomic envelope");
    assert_eq!(envelope.mutation.operations.len(), 24_000);
    let notice = envelope
        .mutation
        .outbox
        .iter()
        .find(|intent| intent.topic == "engine.projection.rebuild")
        .expect("projection notice");
    assert!(
        notice.payload.len() < 4 * 1024,
        "notice is {} bytes",
        notice.payload.len()
    );
}

/// Past the mutation batch's operation budget the batch is refused whole,
/// with a code the source transport acts on, never committed in parts.
// spec: MEDULLA-REPO-INGEST-R007
#[test]
fn a_batch_over_the_commit_budget_is_refused_whole() {
    let limit = medulla_protocol_types::mutation_batch::MAX_MUTATION_OPERATIONS;
    let error = lower(&large_result(limit / 2 + 1))
        .err()
        .expect("over-budget batch is refused");
    assert!(error.starts_with(BATCH_TOO_LARGE), "{error}");
}

/// `count` SYMBOL nodes (no edges), each carrying one property sized
/// `value_len` bytes -- few operations, many bytes per operation, so the
/// encoded write-set's SIZE, not its operation count, is what a caller can
/// push past the budget.
fn byte_heavy_result(count: usize, value: &str) -> IndexResult {
    let nodes = (0..count)
        .map(|index| node(&format!("symbol:{index}"), "SYMBOL", &[("doc", value)]))
        .collect();
    IndexResult {
        nodes,
        ..Default::default()
    }
}

// Supplementary lowering-only evidence for MEDULLA-REPO-INGEST-R008.
// The roughly 60 MB fixture crosses MAX_WRITE_SET_BYTES (48 MiB), not the
// requirement's 64 MiB ceiling. lower() checks refusal and admission of the
// halved write set; it does not submit, commit, or retry through a served client.
// Full integration acceptance remains unverified by this test.
#[test]
fn a_batch_over_the_byte_ceiling_is_refused_and_a_halved_resend_succeeds() {
    let big_value = "x".repeat(300_000);
    // 200 operations (far under MAX_MUTATION_OPERATIONS) but ~60 MB encoded,
    // past MAX_WRITE_SET_BYTES (48 MiB): only the byte ceiling can refuse this.
    let oversized = byte_heavy_result(200, &big_value);
    let error = lower(&oversized)
        .err()
        .expect("a batch past the write-byte ceiling is refused");
    assert!(error.starts_with(BATCH_TOO_LARGE), "{error}");

    // The halved resend: half the operations, half the bytes, well under the
    // ceiling -- the retry a stable code (not a parsed message) enables.
    let halved = byte_heavy_result(100, &big_value);
    lower(&halved).expect("the halved resend commits under the write-byte ceiling");
}

// spec: MEDULLA-REPO-INGEST-R006.5
#[test]
fn one_parse_per_unique_blob_across_branches() {
    use medulla_types::contract::BoundedVec;
    use medulla_types::ingestion_wire::IndexRepositoryScope;
    use medulla_types::test_support::repository_index::{file_version, live_ref};
    use sha2::{Digest, Sha256};

    let digest = |body: &str| format!("sha256:{}", hex::encode(Sha256::digest(body.as_bytes())));
    let util = "def shared():\n    return 1\n";
    let app_main = "from pkg.util import shared\n\ndef run():\n    return shared()\n";
    let app_feature = "from pkg.util import shared\n\ndef run():\n    return shared() + 1\n";
    // Three branches: `util` is one blob on all three, `app_main` is shared by
    // two, `app_feature` belongs to one.
    let scope = IndexRepositoryScope {
        repository_id: "forge/team/project".to_string(),
        refs: BoundedVec::new(vec![
            live_ref("main", 'a'),
            live_ref("feature", 'b'),
            live_ref("release", 'c'),
        ])
        .expect("refs"),
        file_versions: BoundedVec::new(vec![
            file_version("main", "pkg/util.py", digest(util)),
            file_version("main", "pkg/app.py", digest(app_main)),
            file_version("feature", "pkg/util.py", digest(util)),
            file_version("feature", "pkg/app.py", digest(app_feature)),
            file_version("release", "pkg/util.py", digest(util)),
            file_version("release", "pkg/app.py", digest(app_main)),
        ])
        .expect("versions"),
        tombstones: BoundedVec::default(),
        commits: BoundedVec::default(),
    };
    // The caller ships each unique blob once.
    let files = vec![
        ("pkg/util.py".to_string(), util.as_bytes().to_vec()),
        ("pkg/app.py".to_string(), app_main.as_bytes().to_vec()),
        ("pkg/app.py".to_string(), app_feature.as_bytes().to_vec()),
    ];
    let result = medulla_compute::parser::branch_index::index_branches(files, &scope)
        .expect("branch-aware batch");

    let count = |kind: &str| result.nodes.iter().filter(|n| n.node_type == kind).count();
    assert_eq!(result.files_parsed, 3, "one parse per unique blob");
    assert_eq!(count("Blob"), 3);
    assert_eq!(count("FileVersion"), 6, "a path/blob pair per branch");
    assert_eq!(count("Branch"), 3);
    let named = |name: &str| {
        result
            .nodes
            .iter()
            .filter(|n| n.node_type == "SYMBOL")
            .filter(|n| n.properties.get("name").map(String::as_str) == Some(name))
            .count()
    };
    assert_eq!(
        named("shared"),
        1,
        "a blob on three branches yields one symbol"
    );
    assert_eq!(named("run"), 2, "two distinct app blobs, not three");
}
