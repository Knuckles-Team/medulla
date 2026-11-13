//! MEDULLA-FEDERATED-QUERY-R081.11 and R081.3: the served retrieval round through
//! the real `dispatch` shell. A round runs one plan over the caller's snapshot and
//! then the parts the request carries; another graph's caller never sees the rows.
#![cfg(feature = "query")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_exec::algebra::{Op, Plan};
use medulla_exec::exec::{HashEmbedder, TextEmbedder};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_result_types::retrieval_round::{
    sha256_hex_of, ActivationBenchmark, BenchmarkResult, HierarchyPart, RoundRequest, RoundResult,
    SectionRequest, SelectionMode, SelectionPart, SufficiencyPart,
};
use medulla_result_types::retrieval_verdict::{
    FallbackPolicy, HierarchyParentLink, HierarchyRetrievalKind, HierarchyRetrievalPolicy,
    HierarchyScoreProvenance, HierarchyStage, RepairStep, ScoreNormalization, ScoreRegion,
    SectionGranularity, SelectionPolicy, Verdict, RETRIEVAL_BUDGET_EXCEEDED,
    RETRIEVAL_CONTRACT_INVALID, RETRIEVAL_HIERARCHY_INVALID, RETRIEVAL_POLICY_UNVERIFIED,
    RETRIEVAL_ROUND_LIMIT_REQUIRED,
};
use medulla_server_api::state::ServerState;

const SECRET: &str = "retrieval-round-served-secret";

fn state() -> Arc<RwLock<ServerState>> {
    // The server binds its reranker and text embedder from its configuration once per
    // process: the deterministic lexical-overlap reranker and the hash embedder.
    static CONFIG: std::sync::Once = std::sync::Once::new();
    CONFIG.call_once(|| {
        std::env::set_var(
            "MEDULLA_RETRIEVAL_RERANKER_MODEL",
            "builtin:lexical-overlap",
        );
        std::env::set_var("MEDULLA_UQL_TEXT_EMBEDDER", "hash");
    });
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

async fn seed(state: &Arc<RwLock<ServerState>>, docs: usize) {
    for i in 0..docs {
        let response = test_support::dispatch(
            state,
            test_support::commons_request(
                SECRET,
                i as u64 + 1,
                Method::AddNode {
                    node_id: format!("doc-{i}"),
                    properties_msgpack: test_support::json_bytes(
                        serde_json::json!({ "type": "Doc", "text": format!("entry {i}") }),
                    ),
                },
            ),
        )
        .await;
        assert!(
            response.error.is_none(),
            "seed failed: {:?}",
            response.error
        );
    }
}

fn request(sufficiency: Option<SufficiencyPart>) -> RoundRequest {
    RoundRequest {
        plan: Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]),
        corpus_revision: "corpus-1".into(),
        model_revision: "model-1".into(),
        sufficiency,
        selection: None,
        hierarchy: None,
        stop: None,
        state: None,
        feedback: None,
    }
}

fn part(round_limit: Option<u32>) -> SufficiencyPart {
    SufficiencyPart {
        round: 1,
        round_limit,
        k: 4,
        fallback_mode: "extractive".into(),
        fallback: FallbackPolicy::Lenient,
    }
}

async fn round_as(
    state: &Arc<RwLock<ServerState>>,
    graph: Option<&str>,
    request: RoundRequest,
) -> Response {
    let method = Method::RetrievalRound {
        request: Box::new(request),
    };
    let wire = match graph {
        None => test_support::commons_request(SECRET, 90, method),
        Some(graph) => test_support::request(SECRET, 90, graph, method),
    };
    test_support::dispatch(state, wire).await
}

fn decode(response: Response) -> RoundResult {
    assert!(
        response.error.is_none(),
        "round failed: {:?}",
        response.error
    );
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed result"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed result"),
        other => panic!("unexpected payload {other:?}"),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R081.11
#[tokio::test(flavor = "multi_thread")]
async fn round_with_no_parts_returns_rows_only() {
    let state = state();
    seed(&state, 3).await;
    let result = decode(round_as(&state, None, request(None)).await);
    let mut ids: Vec<_> = result.rows.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["doc-0", "doc-1", "doc-2"]);
    assert!(result.sufficiency.is_none());

    // Another tenant's graph sees none of those rows.
    let other = round_as(&state, Some("other-tenant"), request(None)).await;
    assert!(
        other.error.is_some() || decode(other).rows.is_empty(),
        "another graph must not see the rows"
    );

    // A malformed request refuses with the typed code and returns nothing.
    let mut blank = request(None);
    blank.corpus_revision = String::new();
    let refused = round_as(&state, None, blank).await;
    assert!(refused
        .error
        .unwrap_or_default()
        .starts_with("RETRIEVAL_CONTRACT_INVALID"));
}

// spec: MEDULLA-FEDERATED-QUERY-R081.3
#[tokio::test(flavor = "multi_thread")]
async fn served_round_returns_verdict_beside_rows() {
    let state = state();
    seed(&state, 2).await;
    let short = decode(round_as(&state, None, request(Some(part(Some(4))))).await);
    assert_eq!(short.rows.len(), 2);
    let verdict = short.sufficiency.expect("verdict beside rows");
    assert_eq!(verdict.verdict, Verdict::Insufficient);
    assert_eq!(verdict.repair, RepairStep::BoostK { k: 8 });

    seed(&state, 3).await;
    let enough = decode(round_as(&state, None, request(Some(part(Some(4))))).await);
    assert_eq!(
        enough.sufficiency.expect("verdict").verdict,
        Verdict::Sufficient
    );

    // Without a round limit the part refuses with its typed code.
    let refused = round_as(&state, None, request(Some(part(None)))).await;
    assert!(refused
        .error
        .unwrap_or_default()
        .starts_with(RETRIEVAL_ROUND_LIMIT_REQUIRED));

    // A request without the part returns rows only.
    assert!(decode(round_as(&state, None, request(None)).await)
        .sufficiency
        .is_none());
}

// ---- shared seeding for the selection and hierarchy parts ----

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1000);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Store one node, with an optional embedding, over the served write path.
async fn put(
    state: &Arc<RwLock<ServerState>>,
    graph: Option<&str>,
    node_id: &str,
    properties: serde_json::Value,
    embedding: Option<Vec<f32>>,
) {
    let wrap = |id: u64, method: Method| match graph {
        None => test_support::commons_request(SECRET, id, method),
        Some(graph) => test_support::request(SECRET, id, graph, method),
    };
    let add = Method::AddNode {
        node_id: node_id.to_string(),
        properties_msgpack: test_support::json_bytes(properties),
    };
    let response = test_support::dispatch(state, wrap(next_id(), add)).await;
    assert!(
        response.error.is_none(),
        "add {node_id}: {:?}",
        response.error
    );
    if let Some(embedding) = embedding {
        let method = Method::AddEmbedding {
            node_id: node_id.to_string(),
            embedding,
        };
        let response = test_support::dispatch(state, wrap(next_id(), method)).await;
        assert!(
            response.error.is_none(),
            "embed {node_id}: {:?}",
            response.error
        );
    }
}

/// Another tenant's graph stores none of the caller's nodes: the round either refuses or
/// returns nothing selected.
fn assert_unread(response: Response) {
    if response.error.is_none() {
        let result = decode(response);
        assert!(result.hierarchy_selected.is_empty() && result.rows.is_empty());
    }
}

fn error_of(response: Response) -> String {
    response.error.expect("the round refuses")
}

fn hashed(text: &str) -> Vec<f32> {
    HashEmbedder::default().embed(text).expect("hash embeds")
}

// ---- R081.7.3 / R081.7.5: dual-score selection ----

const RERANKER_REVISION: &str = "builtin:lexical-overlap@1";
const QUERY_TEXT: &str = "alpha beta gamma delta";

/// Four chunks with a known embedding score against the query vector `[1, 0]` and a known
/// lexical-overlap score against `QUERY_TEXT`:
/// `hit` high on both, `rescue` low embedding and high overlap, `dual-low` low on both,
/// `residual` high embedding and no overlap.
async fn seed_chunks(state: &Arc<RwLock<ServerState>>) {
    for (id, text, embedding) in [
        ("hit", "alpha beta gamma", vec![1.0, 0.0]),
        ("rescue", "alpha beta gamma delta", vec![0.1, 1.0]),
        ("dual-low", "unrelated words", vec![0.0, 1.0]),
        ("residual", "zzz", vec![0.95, 0.3]),
    ] {
        put(
            state,
            None,
            id,
            serde_json::json!({ "type": "Chunk", "text": text }),
            Some(embedding),
        )
        .await;
    }
}

fn chunk_plan() -> Plan {
    Plan::new(vec![
        Op::Scan {
            label: "Chunk".into(),
        },
        Op::Rank {
            query: vec![1.0, 0.0],
        },
    ])
}

fn selection_policy() -> SelectionPolicy {
    SelectionPolicy {
        policy_revision: "policy-1".into(),
        corpus_revision: "corpus-1".into(),
        embedding_model_revision: "model-1".into(),
        cross_encoder_model_revision: RERANKER_REVISION.into(),
        normalization: ScoreNormalization::MinMax {
            constant_value: 0.5,
        },
        embedding_threshold: 0.5,
        relevance_threshold: 0.5,
        fusion_weight: 0.5,
        fusion_threshold: 0.9,
        max_chunks: 10,
        activation_benchmark_digest: Some("bench-1".into()),
    }
}

fn benchmark(digest: &str, candidate: (f64, f64)) -> ActivationBenchmark {
    ActivationBenchmark {
        digest: digest.into(),
        baseline: BenchmarkResult {
            ndcg_at_10: 0.80,
            mean_selected: 4.0,
        },
        candidate: BenchmarkResult {
            ndcg_at_10: candidate.0,
            mean_selected: candidate.1,
        },
    }
}

fn selection_request(benchmark: Option<ActivationBenchmark>) -> RoundRequest {
    let mut request = request(None);
    request.plan = chunk_plan();
    request.selection = Some(SelectionPart {
        query: QUERY_TEXT.into(),
        policy: selection_policy(),
        benchmark,
        mode: SelectionMode::DualScore,
    });
    request
}

// spec: MEDULLA-FEDERATED-QUERY-R081.7.3
#[tokio::test(flavor = "multi_thread")]
async fn adaptive_selection_rescues_low_embedding_chunk() {
    let state = state();
    seed_chunks(&state).await;
    let good = benchmark("bench-1", (0.795, 3.0));
    let result = decode(round_as(&state, None, selection_request(Some(good))).await);
    let receipt = result.selection.expect("selection receipt");

    // The selector received both scores for every chunk the plan returned.
    assert_eq!(receipt.candidates.len(), 4);
    let region = |id: &str| {
        receipt
            .candidates
            .iter()
            .find(|c| c.chunk_id == id)
            .unwrap_or_else(|| panic!("candidate {id}"))
    };
    let rescue = region("rescue");
    assert!(rescue.embedding_score < 0.2, "{}", rescue.embedding_score);
    assert!((rescue.cross_encoder_score - 1.0).abs() < 1e-6);
    // The low embedding score is rescued by the high cross-encoder score; a chunk low on
    // both is refused; high embedding with no overlap misses the fused threshold.
    assert_eq!(rescue.region, ScoreRegion::Rescue);
    assert!(rescue.selected);
    assert_eq!(region("hit").region, ScoreRegion::DualHigh);
    assert_eq!(region("dual-low").region, ScoreRegion::DualLow);
    assert!(!region("dual-low").selected);
    assert_eq!(region("residual").region, ScoreRegion::Residual);
    assert!(!region("residual").selected);
    let mut selected = receipt.selected.clone();
    selected.sort();
    assert_eq!(selected, ["hit", "rescue"]);

    // The reranker revision that produced the scores appears in the receipt premise.
    let premise = result
        .premises
        .iter()
        .find(|p| p.name == "retrieval.selection")
        .expect("selection premise");
    assert_eq!(premise.value["reranker_revision"], RERANKER_REVISION);
    assert_eq!(premise.value["embedding_model_revision"], "model-1");
    assert_eq!(result.inference.reranker, 4);
    assert_eq!(result.inference.selection, 0);

    // Another tenant's graph holds no chunk, so nothing is scored or selected there.
    let other = round_as(
        &state,
        Some("other-tenant"),
        selection_request(Some(benchmark("bench-1", (0.795, 3.0)))),
    )
    .await;
    assert!(
        other.error.is_some()
            || decode(other)
                .selection
                .map_or(true, |r| r.selected.is_empty())
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R081.7.5
#[tokio::test(flavor = "multi_thread")]
async fn selection_receipt_and_benchmark_gate() {
    let state = state();
    seed_chunks(&state).await;
    let result = decode(
        round_as(
            &state,
            None,
            selection_request(Some(benchmark("bench-1", (0.795, 3.0)))),
        )
        .await,
    );
    let receipt = result.selection.expect("selection receipt");
    assert_eq!(receipt.policy_revision, "policy-1");
    assert_eq!(receipt.corpus_revision, "corpus-1");
    assert_eq!(receipt.max_chunks, 10);
    assert_eq!(
        receipt.activation_benchmark_digest.as_deref(),
        Some("bench-1")
    );
    assert!(matches!(
        receipt.normalization,
        ScoreNormalization::MinMax { .. }
    ));
    assert!(receipt
        .candidates
        .iter()
        .all(|c| c.fused_score.is_finite() && c.normalized_embedding.is_finite()));
    // Selection makes no model call of its own.
    assert_eq!(receipt.selection_inference_calls, 0);
    assert_eq!(result.inference.selection, 0);
    assert_eq!(result.inference.embedding, 0);
    let premise = result
        .premises
        .iter()
        .find(|p| p.name == "retrieval.selection")
        .expect("selection premise");
    assert_eq!(premise.step, "selection");
    assert_eq!(premise.value["selected_count"], 2);
    let mut excluded: Vec<_> = premise.value["excluded"]
        .as_array()
        .expect("excluded ids")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    excluded.sort();
    assert_eq!(excluded, ["dual-low", "residual"]);
    assert_eq!(
        premise.value["policy_digest"].as_str().map(str::len),
        Some(64)
    );

    // A benchmark digest that is not the policy's, a missing benchmark, a quality loss over
    // the floor and a selected count over the cost ratio each refuse before any answer.
    for (case, bench) in [
        ("digest mismatch", Some(benchmark("bench-2", (0.795, 3.0)))),
        ("no benchmark", None),
        ("quality loss", Some(benchmark("bench-1", (0.70, 3.0)))),
        ("cost ratio", Some(benchmark("bench-1", (0.80, 3.9)))),
    ] {
        let refused = round_as(&state, None, selection_request(bench)).await;
        let message = error_of(refused);
        assert!(
            message.starts_with(RETRIEVAL_POLICY_UNVERIFIED),
            "{case}: {message}"
        );
    }
}

// ---- R081.9.3 / R081.9.5: community descent ----

const HIERARCHY_REVISION: &str = "hierarchy-1";
const ENTITY_QUERY: &str = "invoice payment terms";

fn provenance() -> Vec<HierarchyScoreProvenance> {
    vec![HierarchyScoreProvenance {
        scorer: "stored-embedding-cosine".into(),
        scorer_revision: "1".into(),
        basis: "stored summary embedding".into(),
    }]
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// A two-level hierarchy: `c-root` over `c-fin` (entities `e1`, `e2`) and `c-ops` (entity
/// `e3`). The finance summary and entity `e1` carry the query text, so the descent keeps
/// finance and then `e1` under a beam of one.
async fn seed_hierarchy(state: &Arc<RwLock<ServerState>>, graph: Option<&str>) {
    let summaries = [
        ("s-root", "company wide overview"),
        ("s-fin", ENTITY_QUERY),
        ("s-ops", "warehouse shipping logistics"),
    ];
    for (id, text) in summaries {
        put(
            state,
            graph,
            id,
            serde_json::json!({ "type": "Summary", "text": text }),
            Some(hashed(text)),
        )
        .await;
    }
    for (id, text) in [
        ("e1", ENTITY_QUERY),
        ("e2", "refund policy"),
        ("e3", "pallet labels"),
    ] {
        put(
            state,
            graph,
            id,
            serde_json::json!({ "type": "Entity", "text": text }),
            Some(hashed(text)),
        )
        .await;
    }
    for (id, summary, entities) in [
        ("c-root", "s-root", vec![]),
        ("c-fin", "s-fin", vec!["e1", "e2"]),
        ("c-ops", "s-ops", vec!["e3"]),
    ] {
        put(
            state,
            graph,
            id,
            serde_json::json!({
                "type": "Community",
                "summary_id": summary,
                "summary_revision": HIERARCHY_REVISION,
                "entity_ids": entities,
            }),
            None,
        )
        .await;
    }
}

fn community_policy() -> HierarchyRetrievalPolicy {
    HierarchyRetrievalPolicy {
        kind: HierarchyRetrievalKind::Community,
        hierarchy_revision: HIERARCHY_REVISION.into(),
        community_ids: strings(&["c-root", "c-fin", "c-ops"]),
        summary_ids: strings(&["s-root", "s-fin", "s-ops"]),
        entity_ids: strings(&["e1", "e2", "e3"]),
        parent_links: vec![
            HierarchyParentLink {
                parent_id: "c-root".into(),
                child_id: "c-fin".into(),
            },
            HierarchyParentLink {
                parent_id: "c-root".into(),
                child_id: "c-ops".into(),
            },
        ],
        stage: HierarchyStage::Community,
        score_provenance: provenance(),
        beam_width: 1,
        max_depth: 4,
        max_visited_communities: 10,
        max_entity_candidates: 10,
        max_reranker_calls: 20,
        max_tokens: 1000,
        max_total_cost_micros: 1000,
        section_nodes: Vec::new(),
    }
}

fn hierarchy_request(part: HierarchyPart) -> RoundRequest {
    let mut request = request(Some(part_with_min_rows()));
    // The plan scans a label nothing carries: flat retrieval alone returns no row.
    request.plan = Plan::new(vec![Op::Scan {
        label: "NoSuchLabel".into(),
    }]);
    request.hierarchy = Some(part);
    request
}

fn part_with_min_rows() -> SufficiencyPart {
    part(Some(4))
}

fn community_part() -> HierarchyPart {
    HierarchyPart {
        query: ENTITY_QUERY.into(),
        policy: community_policy(),
        section: None,
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R081.9.3
#[tokio::test(flavor = "multi_thread")]
async fn community_descent_returns_entities_with_receipt() {
    let state = state();
    seed_hierarchy(&state, None).await;
    let result = decode(round_as(&state, None, hierarchy_request(community_part())).await);

    // Flat retrieval returns nothing; the descent returns the best entity of the best
    // community, and the sufficiency part counts it.
    assert!(result.rows.is_empty());
    assert_eq!(result.hierarchy_selected, ["e1"]);
    assert_eq!(result.sufficiency.expect("verdict").evidence_count, 1);
    assert_eq!(result.inference.embedding, 1);

    // All three stages ran, in order.
    let premise = result
        .premises
        .iter()
        .find(|p| p.name == "retrieval.hierarchy")
        .expect("hierarchy premise");
    let visits = premise.value["receipt"]["visits"]
        .as_array()
        .expect("visits");
    let stages: Vec<&str> = visits
        .iter()
        .map(|v| v["stage"].as_str().unwrap_or_default())
        .collect();
    let first = |stage: &str| stages.iter().position(|s| *s == stage).expect(stage);
    assert!(first("summary") < first("community") && first("community") < first("entity"));

    // The second tenant's graph stores none of the hierarchy: it refuses and returns nothing.
    assert_unread(
        round_as(
            &state,
            Some("other-tenant"),
            hierarchy_request(community_part()),
        )
        .await,
    );

    // A stale hierarchy revision refuses rather than descending a stale summary.
    let mut stale = community_part();
    stale.policy.hierarchy_revision = "hierarchy-0".into();
    let refused = round_as(&state, None, hierarchy_request(stale)).await;
    assert!(error_of(refused).starts_with(RETRIEVAL_HIERARCHY_INVALID));

    // A dangling entity refuses.
    put(
        &state,
        None,
        "c-ghost",
        serde_json::json!({
            "type": "Community",
            "summary_id": "s-root",
            "summary_revision": HIERARCHY_REVISION,
            "entity_ids": ["no-such-entity"],
        }),
        None,
    )
    .await;
    let mut dangling = community_part();
    dangling.policy.beam_width = 3;
    dangling.policy.community_ids.push("c-ghost".into());
    dangling.policy.parent_links.push(HierarchyParentLink {
        parent_id: "c-root".into(),
        child_id: "c-ghost".into(),
    });
    let refused = round_as(&state, None, hierarchy_request(dangling)).await;
    assert!(error_of(refused).starts_with(RETRIEVAL_HIERARCHY_INVALID));

    // An exhausted reranker budget refuses with the budget code and returns no entity.
    let mut tight = community_part();
    tight.policy.max_reranker_calls = 2;
    let refused = round_as(&state, None, hierarchy_request(tight)).await;
    assert!(error_of(refused).starts_with(RETRIEVAL_BUDGET_EXCEEDED));
}

// spec: MEDULLA-FEDERATED-QUERY-R081.9.5
#[tokio::test(flavor = "multi_thread")]
async fn hierarchy_premise_accounts_for_every_stage() {
    let state = state();
    seed_hierarchy(&state, None).await;
    let result = decode(round_as(&state, None, hierarchy_request(community_part())).await);
    let premise = result
        .premises
        .iter()
        .find(|p| p.name == "retrieval.hierarchy")
        .expect("hierarchy premise");
    assert_eq!(premise.step, "hierarchy");
    let receipt = &premise.value["receipt"];

    // Measured counters: one root, two child communities, two entities of the kept community.
    assert_eq!(receipt["visited_communities"], 3);
    assert_eq!(receipt["entity_candidates"], 2);
    assert_eq!(receipt["reranker_calls"], 5);
    assert_eq!(receipt["cost"], 5);
    assert_eq!(receipt["depth"], 1);
    assert_eq!(receipt["incomplete"], false);
    assert_eq!(receipt["hierarchy_revision"], HIERARCHY_REVISION);
    assert_eq!(premise.value["summary_revision"], HIERARCHY_REVISION);
    assert_eq!(premise.value["corpus_revision"], "corpus-1");
    assert_eq!(result.inference.reranker, 5);

    // Each rejected candidate appears with its measured score.
    let visits = receipt["visits"].as_array().expect("visits");
    let visit = |id: &str| {
        visits
            .iter()
            .find(|v| v["id"] == id)
            .unwrap_or_else(|| panic!("visit {id}"))
    };
    assert_eq!(visit("c-fin")["kept"], true);
    assert_eq!(visit("c-ops")["kept"], false);
    assert_eq!(visit("e1")["kept"], true);
    assert_eq!(visit("e2")["kept"], false);
    assert!(visit("e2")["score"].as_f64().is_some());
    assert_eq!(visit("e1")["path"], serde_json::json!(["c-root", "c-fin"]));

    // The flat baseline measured for the same query: every stored entity scored once.
    assert_eq!(premise.value["flat_candidates"], 3);
    assert_eq!(premise.value["flat_visits"], 3);
}

// ---- R081.9.7.3 / R081.9.7.4 / R081.9.7.5: document-section traversal ----

const DOC_REVISION: &str = "doc-rev-1";

#[allow(clippy::too_many_arguments)]
async fn put_section(
    state: &Arc<RwLock<ServerState>>,
    graph: Option<&str>,
    id: &str,
    parent: Option<&str>,
    children: &[&str],
    granularity: &str,
    span: (u64, u64),
    revision: &str,
) {
    put(
        state,
        graph,
        id,
        serde_json::json!({
            "type": "SectionNode",
            "document_id": "doc-1",
            "document_revision": revision,
            "parent_id": parent,
            "children": children,
            "granularity": granularity,
            "source_start": span.0,
            "source_end": span.1,
            "summary": format!("summary of {id}"),
            "source_digest": format!("src-{id}"),
            "summary_digest": format!("sum-{id}"),
        }),
        None,
    )
    .await;
}

/// `sec-doc` (document) over `sec-a` and `sec-b` (sections), which hold the source spans
/// `span-a1`, `span-a2` and `span-b1`.
async fn seed_sections(state: &Arc<RwLock<ServerState>>, graph: Option<&str>) {
    let nodes: [(&str, Option<&str>, &[&str], &str, (u64, u64)); 6] = [
        ("sec-doc", None, &["sec-a", "sec-b"], "document", (0, 100)),
        (
            "sec-a",
            Some("sec-doc"),
            &["span-a1", "span-a2"],
            "section",
            (0, 60),
        ),
        ("sec-b", Some("sec-doc"), &["span-b1"], "section", (60, 100)),
        ("span-a1", Some("sec-a"), &[], "source_span", (0, 30)),
        ("span-a2", Some("sec-a"), &[], "source_span", (30, 60)),
        ("span-b1", Some("sec-b"), &[], "source_span", (60, 100)),
    ];
    for (id, parent, children, granularity, span) in nodes {
        put_section(
            state,
            graph,
            id,
            parent,
            &children,
            granularity,
            span,
            DOC_REVISION,
        )
        .await;
    }
}

fn section_policy() -> HierarchyRetrievalPolicy {
    HierarchyRetrievalPolicy {
        kind: HierarchyRetrievalKind::DocumentSection,
        hierarchy_revision: "section-policy-1".into(),
        community_ids: Vec::new(),
        summary_ids: Vec::new(),
        entity_ids: Vec::new(),
        parent_links: Vec::new(),
        stage: HierarchyStage::DocumentSection,
        score_provenance: provenance(),
        beam_width: 1,
        max_depth: 8,
        max_visited_communities: 50,
        max_entity_candidates: 1,
        max_reranker_calls: 1,
        max_tokens: 1,
        max_total_cost_micros: 1,
        section_nodes: Vec::new(),
    }
}

fn section_part(root: &str, granularity: SectionGranularity) -> HierarchyPart {
    HierarchyPart {
        query: String::new(),
        policy: section_policy(),
        section: Some(SectionRequest {
            root_id: root.into(),
            document_revision: DOC_REVISION.into(),
            granularity,
        }),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R081.9.7.3
#[tokio::test(flavor = "multi_thread")]
async fn section_traversal_returns_spans_with_paths() {
    let state = state();
    seed_sections(&state, None).await;
    let result = decode(
        round_as(
            &state,
            None,
            hierarchy_request(section_part("sec-doc", SectionGranularity::SourceSpan)),
        )
        .await,
    );
    assert_eq!(result.hierarchy_selected, ["span-a1", "span-a2", "span-b1"]);
    // The selected sections join the evidence set the sufficiency part counts.
    assert_eq!(
        result.sufficiency.expect("verdict").verdict,
        Verdict::Sufficient
    );
    // No embedding, reranker or vector-index call was made.
    assert_eq!(result.inference.embedding, 0);
    assert_eq!(result.inference.reranker, 0);
    assert_eq!(result.inference.selection, 0);
    let premise = result
        .premises
        .iter()
        .find(|p| p.name == "retrieval.sections")
        .expect("sections premise");
    let receipt = &premise.value["receipt"];
    assert_eq!(
        receipt["paths"][0],
        serde_json::json!(["sec-doc", "sec-a", "span-a1"])
    );
    assert_eq!(
        receipt["selected_spans"][2],
        serde_json::json!({"start": 60, "end": 100})
    );

    // Section granularity returns the two sections, document granularity the root alone.
    let sections = decode(
        round_as(
            &state,
            None,
            hierarchy_request(section_part("sec-doc", SectionGranularity::Section)),
        )
        .await,
    );
    assert_eq!(sections.hierarchy_selected, ["sec-a", "sec-b"]);
    let document = decode(
        round_as(
            &state,
            None,
            hierarchy_request(section_part("sec-doc", SectionGranularity::Document)),
        )
        .await,
    );
    assert_eq!(document.hierarchy_selected, ["sec-doc"]);

    // A second tenant reads nothing: the root is not stored in its graph.
    assert_unread(
        round_as(
            &state,
            Some("other-tenant"),
            hierarchy_request(section_part("sec-doc", SectionGranularity::SourceSpan)),
        )
        .await,
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R081.9.7.4
#[tokio::test(flavor = "multi_thread")]
async fn malformed_trees_and_limits_refuse() {
    let state = state();
    seed_sections(&state, None).await;
    let refuse = |part: HierarchyPart| {
        let state = state.clone();
        async move { error_of(round_as(&state, None, hierarchy_request(part)).await) }
    };

    // A stale document revision.
    let mut stale = section_part("sec-doc", SectionGranularity::SourceSpan);
    if let Some(section) = stale.section.as_mut() {
        section.document_revision = "doc-rev-0".into();
    }
    assert!(refuse(stale).await.starts_with(RETRIEVAL_HIERARCHY_INVALID));

    // A root that is not stored.
    let missing = section_part("sec-none", SectionGranularity::SourceSpan);
    assert!(refuse(missing)
        .await
        .starts_with(RETRIEVAL_HIERARCHY_INVALID));

    // A granularity the tree does not hold.
    put_section(
        &state,
        None,
        "flat-doc",
        None,
        &["flat-sec"],
        "document",
        (0, 10),
        DOC_REVISION,
    )
    .await;
    put_section(
        &state,
        None,
        "flat-sec",
        Some("flat-doc"),
        &[],
        "section",
        (0, 10),
        DOC_REVISION,
    )
    .await;
    let none = section_part("flat-doc", SectionGranularity::SourceSpan);
    assert!(refuse(none).await.starts_with(RETRIEVAL_HIERARCHY_INVALID));

    // A cycle and a dangling child.
    put_section(
        &state,
        None,
        "cyc-doc",
        None,
        &["cyc-sec"],
        "document",
        (0, 10),
        DOC_REVISION,
    )
    .await;
    put_section(
        &state,
        None,
        "cyc-sec",
        Some("cyc-doc"),
        &["cyc-sec"],
        "section",
        (0, 10),
        DOC_REVISION,
    )
    .await;
    let cycle = section_part("cyc-doc", SectionGranularity::Section);
    assert!(refuse(cycle).await.starts_with(RETRIEVAL_HIERARCHY_INVALID));
    put_section(
        &state,
        None,
        "dangle-doc",
        None,
        &["ghost"],
        "document",
        (0, 10),
        DOC_REVISION,
    )
    .await;
    let dangle = section_part("dangle-doc", SectionGranularity::Section);
    assert!(refuse(dangle)
        .await
        .starts_with(RETRIEVAL_HIERARCHY_INVALID));

    // A child outside its parent's span.
    put_section(
        &state,
        None,
        "wide-doc",
        None,
        &["wide-sec"],
        "document",
        (0, 10),
        DOC_REVISION,
    )
    .await;
    put_section(
        &state,
        None,
        "wide-sec",
        Some("wide-doc"),
        &[],
        "section",
        (5, 50),
        DOC_REVISION,
    )
    .await;
    let wide = section_part("wide-doc", SectionGranularity::Section);
    assert!(refuse(wide).await.starts_with(RETRIEVAL_HIERARCHY_INVALID));

    // An exhausted depth or visited limit stops the walk and states it is incomplete.
    let mut shallow = section_part("sec-doc", SectionGranularity::SourceSpan);
    shallow.policy.max_depth = 1;
    let message = refuse(shallow).await;
    assert!(message.starts_with(RETRIEVAL_BUDGET_EXCEEDED), "{message}");
    assert!(message.contains("depth") && message.contains("incomplete: true"));
    let mut narrow = section_part("sec-doc", SectionGranularity::SourceSpan);
    narrow.policy.max_visited_communities = 3;
    let message = refuse(narrow).await;
    assert!(message.starts_with(RETRIEVAL_BUDGET_EXCEEDED), "{message}");
    assert!(message.contains("visited"));

    // A served request carries no inline section nodes, and names its section request.
    let mut inline = section_part("sec-doc", SectionGranularity::SourceSpan);
    inline.policy.section_nodes = vec![
        medulla_result_types::retrieval_verdict::HierarchySectionNode {
            id: "x".into(),
            document_id: "doc-1".into(),
            document_revision: DOC_REVISION.into(),
            parent_id: None,
            ordered_children: Vec::new(),
            granularity: SectionGranularity::Document,
            summary: "x".into(),
            source_span: medulla_result_types::retrieval_verdict::SectionSourceSpan {
                start: 0,
                end: 1,
            },
        },
    ];
    assert!(refuse(inline).await.starts_with(RETRIEVAL_CONTRACT_INVALID));
    let mut bare = section_part("sec-doc", SectionGranularity::SourceSpan);
    bare.section = None;
    assert!(refuse(bare).await.starts_with(RETRIEVAL_CONTRACT_INVALID));
}

// spec: MEDULLA-FEDERATED-QUERY-R081.9.7.5
#[tokio::test(flavor = "multi_thread")]
async fn section_receipt_reconstructs_all_paths() {
    let state = state();
    seed_sections(&state, None).await;
    let result = decode(
        round_as(
            &state,
            None,
            hierarchy_request(section_part("sec-doc", SectionGranularity::Section)),
        )
        .await,
    );
    let premise = result
        .premises
        .iter()
        .find(|p| p.name == "retrieval.sections")
        .expect("sections premise");
    assert_eq!(premise.step, "hierarchy");
    let receipt = &premise.value["receipt"];

    // Every visited path, including the rejected branches below the selected sections,
    // rebuilds from the premise alone.
    let visits = premise.value["visits"].as_array().expect("visits");
    let path_of = |id: &str| -> Vec<String> {
        serde_json::from_value(
            visits
                .iter()
                .find(|v| v["id"] == id)
                .unwrap_or_else(|| panic!("visit {id}"))["path"]
                .clone(),
        )
        .expect("path")
    };
    assert_eq!(visits.len(), 6);
    assert_eq!(path_of("sec-doc"), ["sec-doc"]);
    assert_eq!(path_of("sec-b"), ["sec-doc", "sec-b"]);
    assert_eq!(path_of("span-a2"), ["sec-doc", "sec-a", "span-a2"]);
    assert_eq!(path_of("span-b1"), ["sec-doc", "sec-b", "span-b1"]);
    let mut pruned: Vec<String> =
        serde_json::from_value(receipt["pruned_ids"].clone()).expect("pruned ids");
    pruned.sort();
    assert_eq!(pruned, ["span-a1", "span-a2", "span-b1"]);

    // The selected paths, spans and revisions are bound; no section text is carried.
    assert_eq!(
        receipt["paths"],
        serde_json::json!([["sec-doc", "sec-a"], ["sec-doc", "sec-b"]])
    );
    assert_eq!(
        receipt["selected_spans"],
        serde_json::json!([{"start": 0, "end": 60}, {"start": 60, "end": 100}])
    );
    assert_eq!(receipt["document_revision"], DOC_REVISION);
    assert_eq!(receipt["policy_revision"], "section-policy-1");
    let order = ["sec-doc", "sec-a", "sec-b", "span-a1", "span-a2", "span-b1"];
    let digests: Vec<String> = order.iter().map(|id| format!("{id}:sum-{id}")).collect();
    assert_eq!(receipt["summary_digest"], sha256_hex_of(&digests));
    assert!(!receipt.to_string().contains("summary of"));
}

// ---- R082.3 / R082.4: the stop part ----

use medulla_result_types::retrieval_round::{BudgetUsage, CallerLimits, RoundAnswer, StopRequest};
use medulla_result_types::retrieval_stop::{
    CalibrationRefusal, CalibrationTable, RoundCalibration, StopReason,
};
use medulla_types::contract::BoundedVec;

fn calibration(rounds: &[u32]) -> CalibrationTable {
    CalibrationTable {
        rows: BoundedVec::new(
            rounds
                .iter()
                .map(|&round| RoundCalibration {
                    round,
                    baseline_confidence: 0.5,
                    margin_threshold: 0.25,
                })
                .collect(),
        )
        .expect("rows fit"),
    }
}

fn answer(round: u32, digest: &str, confidence: f64) -> RoundAnswer {
    RoundAnswer {
        round,
        answer_digest: digest.into(),
        confidence,
    }
}

fn stop_part(table: CalibrationTable, history: Vec<RoundAnswer>) -> StopRequest {
    StopRequest {
        calibration: table,
        history: BoundedVec::new(history).expect("history fits"),
        prior_row_ids: BoundedVec::default(),
        prior_quality: None,
        usage: BudgetUsage::default(),
        limits: CallerLimits::default(),
    }
}

fn stop_request(round: u32, stop: StopRequest) -> RoundRequest {
    let mut request = request(Some(part(Some(4))));
    request.sufficiency.as_mut().expect("part").round = round;
    request.stop = Some(stop);
    request
}

// spec: MEDULLA-FEDERATED-QUERY-R082.3
#[tokio::test(flavor = "multi_thread")]
async fn stop_precedence_follows_the_declared_order() {
    let state = state();
    // Two documents are below the evidence floor: the sufficiency repair is a boost.
    seed(&state, 2).await;

    // No stop part: no stop verdict.
    let bare = decode(round_as(&state, None, request(Some(part(Some(4))))).await);
    assert!(bare.stop.is_none());

    // Hook 1, limits first: a continuation with no answer history still stops at the
    // round limit, and the response names the dimension. Round and token limits are
    // both spent: the round limit wins.
    let mut spent = stop_part(calibration(&[4]), vec![]);
    spent.usage.tokens_used = 5;
    spent.limits.token_limit = Some(5);
    let refused = error_of(round_as(&state, None, stop_request(4, spent)).await);
    assert!(
        refused.starts_with("RETRIEVAL_BUDGET_EXCEEDED"),
        "{refused}"
    );
    assert!(refused.contains("round_limit") && refused.contains("\"round\""));

    // Another exhausted budget: a caller lowers a limit, never raises it.
    let mut tokens = stop_part(calibration(&[1]), vec![]);
    tokens.usage.tokens_used = 7;
    tokens.limits.token_limit = Some(7);
    let refused = error_of(round_as(&state, None, stop_request(1, tokens)).await);
    assert!(
        refused.starts_with("RETRIEVAL_BUDGET_EXCEEDED"),
        "{refused}"
    );
    assert!(refused.contains("budget_exhausted") && refused.contains("tokens"));
    let mut raised = stop_part(calibration(&[1]), vec![]);
    raised.usage.tokens_used = 32_000;
    raised.limits.token_limit = Some(10_000_000);
    let refused = error_of(round_as(&state, None, stop_request(1, raised)).await);
    assert!(
        refused.contains("tokens"),
        "the knob caps the caller: {refused}"
    );

    // Hook 2, a terminal verdict leaves no continuation: no new evidence after a
    // measured round turns the boost into a stop.
    let mut stale = stop_part(calibration(&[1]), vec![]);
    stale.prior_row_ids =
        BoundedVec::new(vec!["doc-0".to_string(), "doc-1".to_string()]).expect("ids fit");
    let result = decode(round_as(&state, None, stop_request(1, stale)).await);
    let verdict = result.stop.expect("stop verdict");
    assert!(verdict.stop && verdict.reason == StopReason::NoNewEvidence);
    assert_eq!(
        result.sufficiency.expect("sufficiency").repair,
        medulla_result_types::retrieval_verdict::RepairStep::Stop
    );

    // Calibrated confidence: the same answer twice, margin above the threshold.
    let repeated = stop_part(
        calibration(&[2]),
        vec![answer(1, "d", 0.6), answer(2, "d", 0.9)],
    );
    let verdict = decode(round_as(&state, None, stop_request(2, repeated)).await)
        .stop
        .expect("stop verdict");
    assert!(verdict.stop && verdict.reason == StopReason::AnswerRepeatedMarginMet);
    assert_eq!(verdict.threshold, Some(0.25));

    // A continuing round keeps the harness's next step.
    let changed = stop_part(
        calibration(&[2]),
        vec![answer(1, "a", 0.6), answer(2, "b", 0.9)],
    );
    let result = decode(round_as(&state, None, stop_request(2, changed)).await);
    assert_eq!(
        result.stop.expect("stop verdict").reason,
        StopReason::ContinueAnswerChanged
    );
    assert!(matches!(
        result.sufficiency.expect("sufficiency").repair,
        medulla_result_types::retrieval_verdict::RepairStep::BoostK { .. }
    ));
    // A round past the limit is a malformed continuation, not a quiet stop.
    let beyond = stop_part(calibration(&[9]), vec![]);
    let refused = error_of(round_as(&state, None, stop_request(9, beyond)).await);
    assert!(
        refused.starts_with("RETRIEVAL_CONTRACT_INVALID"),
        "{refused}"
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R082.5
#[tokio::test(flavor = "multi_thread")]
async fn stop_premise_lists_margin_and_reason() {
    let state = state();
    seed(&state, 2).await;

    // A terminal verdict on a round with no calibration row: no new evidence stops
    // the loop, and the premise keeps the refusal, the reason and the progress.
    let mut stale = stop_part(calibration(&[2]), vec![]);
    stale.prior_row_ids =
        BoundedVec::new(vec!["doc-0".to_string(), "doc-1".to_string()]).expect("ids fit");
    let result = decode(round_as(&state, None, stop_request(1, stale)).await);
    let verdict = result.stop.clone().expect("stop verdict");
    assert!(verdict.stop && verdict.calibration_refusal.is_some());
    let premise = result
        .premises
        .iter()
        .find(|p| p.name == "retrieval.stop")
        .expect("stop premise");
    assert_eq!(premise.step, "stop");
    assert_eq!(premise.value["round"], 1);
    assert_eq!(premise.value["stop"], true);
    assert_eq!(premise.value["reason"], "no_new_evidence");
    assert_eq!(premise.value["calibration_refusal"], "ROUND_NOT_CALIBRATED");
    assert!(premise.value["margin"].is_null() && premise.value["threshold"].is_null());
    assert_eq!(premise.value["policy_revision"], "corpus-1:model-1");
    assert_eq!(premise.value["progress"]["no_new_evidence"], true);
    // The insufficiency verdict stays visible beside the stop premise.
    assert!(result.sufficiency.is_some());

    // A calibrated repeat lists its margin and threshold.
    let repeated = stop_part(
        calibration(&[2]),
        vec![answer(1, "d", 0.6), answer(2, "d", 0.9)],
    );
    let result = decode(round_as(&state, None, stop_request(2, repeated)).await);
    let premise = result
        .premises
        .iter()
        .find(|p| p.name == "retrieval.stop")
        .expect("stop premise");
    assert_eq!(premise.value["reason"], "answer_repeated_margin_met");
    assert_eq!(premise.value["threshold"], 0.25);
    assert!(premise.value["margin"].as_f64().expect("margin") > 0.25);
    assert!(premise.value["calibration_refusal"].is_null());

    // No stop part: no stop premise.
    let bare = decode(round_as(&state, None, request(Some(part(Some(4))))).await);
    assert!(bare.premises.iter().all(|p| p.name != "retrieval.stop"));
}

// spec: MEDULLA-FEDERATED-QUERY-R082.4
#[tokio::test(flavor = "multi_thread")]
async fn uncalibrated_rounds_keep_the_refusal_marker() {
    let state = state();
    seed(&state, 2).await;

    // Continuing: the round has no calibration row, so a confidence stop is forbidden
    // even for an answer repeated with a wide margin.
    let repeated = stop_part(
        calibration(&[7]),
        vec![answer(1, "d", 0.99), answer(2, "d", 0.99)],
    );
    let result = decode(round_as(&state, None, stop_request(2, repeated)).await);
    let verdict = result.stop.expect("stop verdict");
    assert!(!verdict.stop && verdict.reason == StopReason::ContinueUncalibrated);
    assert_eq!(
        verdict.calibration_refusal,
        Some(CalibrationRefusal::RoundNotCalibrated)
    );
    assert!(verdict.margin.is_none() && verdict.threshold.is_none());

    // Terminal: no new evidence still wins and keeps the marker.
    let mut stale = stop_part(
        calibration(&[]),
        vec![answer(1, "d", 0.9), answer(2, "d", 0.9)],
    );
    stale.prior_row_ids =
        BoundedVec::new(vec!["doc-0".to_string(), "doc-1".to_string()]).expect("ids fit");
    let verdict = decode(round_as(&state, None, stop_request(2, stale)).await)
        .stop
        .expect("stop verdict");
    assert!(verdict.stop && verdict.reason == StopReason::NoNewEvidence);
    assert_eq!(
        verdict.calibration_refusal,
        Some(CalibrationRefusal::RoundNotCalibrated)
    );

    // Exhausted limit: the budget code carries the terminal verdict, the dimension and
    // the calibration marker; no fallback calibration or budget reset appears.
    let mut spent = stop_part(calibration(&[]), vec![]);
    spent.usage.inference_calls_used = 3;
    spent.limits.inference_limit = Some(3);
    let refused = error_of(round_as(&state, None, stop_request(1, spent)).await);
    assert!(
        refused.starts_with("RETRIEVAL_BUDGET_EXCEEDED"),
        "{refused}"
    );
    assert!(refused.contains("inference_calls") && refused.contains("ROUND_NOT_CALIBRATED"));
}

// ---- R081.100: joint sparse nonnegative selection ----

/// Four documents in three dimensions: `diverse-a` and `diverse-b` span the query's plane,
/// `corr-a` and `corr-b` are highly correlated with each other and mostly off it.
async fn seed_joint(state: &Arc<RwLock<ServerState>>) {
    for (id, embedding) in [
        ("diverse-a", vec![1.0, 0.0, 0.0]),
        ("diverse-b", vec![0.0, 1.0, 0.0]),
        ("corr-a", vec![0.4, 0.4, 0.8]),
        ("corr-b", vec![0.38, 0.42, 0.81]),
    ] {
        put(
            state,
            None,
            id,
            serde_json::json!({ "type": "Chunk", "text": id }),
            Some(embedding),
        )
        .await;
    }
}

fn joint_request(
    lambda: Option<f64>,
    max_iters: Option<u32>,
    revision: &str,
    max_chunks: u32,
) -> RoundRequest {
    let mut policy = selection_policy();
    policy.max_chunks = max_chunks;
    let mut request = request(None);
    request.plan = Plan::new(vec![Op::Scan {
        label: "Chunk".into(),
    }]);
    request.selection = Some(SelectionPart {
        query: QUERY_TEXT.into(),
        policy,
        benchmark: None,
        mode: SelectionMode::JointSparse {
            embedding_revision: revision.into(),
            lambda,
            tolerance: Some(1e-12),
            max_iters,
            query_vector: Some(vec![1.0, 0.8, 0.0]),
        },
    });
    request
}

// spec: MEDULLA-FEDERATED-QUERY-R081.100
#[tokio::test(flavor = "multi_thread")]
async fn joint_selection_prefers_diverse_pair() {
    let state = state();
    seed_joint(&state).await;

    // The diverse pair reconstructs the query; the correlated pair gets no coefficient.
    // Exhaustive oracle at lambda 0.1: coefficients 0.95 and 0.75, residual hypot(0.05, 0.05).
    let result =
        decode(round_as(&state, None, joint_request(Some(0.1), None, "model-1", 10)).await);
    let joint = result.joint_selection.expect("joint selection");
    assert!(
        result.selection.is_none(),
        "the dual-score receipt is not produced"
    );
    let ids: Vec<_> = joint.selected.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["diverse-a", "diverse-b"]);
    assert!((joint.selected[0].coefficient - 0.95).abs() < 1e-6);
    assert!((joint.selected[1].coefficient - 0.75).abs() < 1e-6);
    assert!((joint.residual - 0.05_f64.hypot(0.05)).abs() < 1e-6);
    assert_eq!(joint.embedding_revision, "model-1");
    assert_eq!(joint.truncated, 0);
    assert_eq!(result.inference.reranker, 0, "no model is called");

    // The result-count bound still applies: one chunk keeps the largest coefficient.
    let capped = decode(round_as(&state, None, joint_request(Some(0.1), None, "model-1", 1)).await)
        .joint_selection
        .expect("joint selection");
    assert_eq!(capped.selected.len(), 1);
    assert_eq!(capped.selected[0].id, "diverse-a");
    assert_eq!(capped.truncated, 1);

    // Another tenant's graph holds none of these documents, so it selects nothing.
    let other = round_as(
        &state,
        Some("other-tenant"),
        joint_request(Some(0.1), None, "model-1", 10),
    )
    .await;
    if other.error.is_none() {
        let other = decode(other);
        assert!(other
            .joint_selection
            .is_none_or(|joint| joint.selected.is_empty()));
    }

    // Refusals: a stale embedding revision, an invalid lambda and a solve that cannot converge.
    let mismatch =
        error_of(round_as(&state, None, joint_request(Some(0.1), None, "model-2", 10)).await);
    assert!(
        mismatch.contains("RETRIEVAL_EMBEDDING_REVISION_MISMATCH"),
        "{mismatch}"
    );
    let invalid =
        error_of(round_as(&state, None, joint_request(Some(-1.0), None, "model-1", 10)).await);
    assert!(invalid.contains(RETRIEVAL_POLICY_UNVERIFIED), "{invalid}");
    let stuck = error_of(
        round_as(
            &state,
            None,
            joint_request(Some(0.1), Some(1), "model-1", 10),
        )
        .await,
    );
    assert!(stuck.contains("RETRIEVAL_SOLVER_NOT_CONVERGED"), "{stuck}");
}

// ---- MEDULLA-FEDERATED-QUERY-R081.101: reusable compressed document state ----

fn long_text(word: &str, filler: &str) -> String {
    let mut text = String::new();
    for _ in 0..60 {
        text.push_str(word);
        text.push(' ');
        text.push_str(filler);
        text.push(' ');
    }
    text
}

fn state_docs() -> Vec<(&'static str, String)> {
    vec![
        ("sdoc-a", long_text("apple", "pie")),
        ("sdoc-b", long_text("ocean", "wave")),
        ("sdoc-c", long_text("apple", "apple")),
    ]
}

async fn seed_state_docs(
    state: &Arc<RwLock<ServerState>>,
    docs: &[(&'static str, String)],
    build: bool,
) {
    for (i, (id, text)) in docs.iter().enumerate() {
        let response = test_support::dispatch(
            state,
            test_support::commons_request(
                SECRET,
                i as u64 + 1,
                Method::AddNode {
                    node_id: (*id).to_string(),
                    properties_msgpack: test_support::json_bytes(
                        serde_json::json!({ "type": "Doc", "text": text }),
                    ),
                },
            ),
        )
        .await;
        assert!(
            response.error.is_none(),
            "seed failed: {:?} {:?}",
            response.error,
            response.error_detail
        );
    }
    if build {
        // The build job (run once per content digest at the document commit) fills the store.
        let guard = state.read().await;
        let store = medulla_persistence::doc_state_store::shared_doc_state_store(&guard)
            .expect("state store opens under the persist dir");
        for (id, text) in docs {
            let built = medulla_plan::doc_state::build_document_state(
                &medulla_plan::doc_state::HashSegmentsEncoder,
                id,
                text,
                512,
            )
            .expect("state builds");
            store.put(&built).expect("state stores");
        }
    }
}

fn state_request(query: &str) -> RoundRequest {
    let mut round = request(None);
    round.state = Some(medulla_result_types::retrieval_round::StatePart {
        query: query.to_string(),
    });
    round
}

// spec: MEDULLA-FEDERATED-QUERY-R081.101
// spec: MEDULLA-FEDERATED-QUERY-R081.101.3
#[tokio::test(flavor = "multi_thread")]
async fn compressed_state_rerank_is_order_independent() {
    let docs = state_docs();
    let forward = state();
    seed_state_docs(&forward, &docs, true).await;
    let mut reversed_docs = docs.clone();
    reversed_docs.reverse();
    let reversed = state();
    seed_state_docs(&reversed, &reversed_docs, true).await;

    let apple = decode(round_as(&forward, None, state_request("apple pie apple")).await);
    let ocean = decode(round_as(&forward, None, state_request("ocean wave")).await);
    let apple_rev = decode(round_as(&reversed, None, state_request("apple pie apple")).await);
    let apple = apple.state.expect("state part answered");
    let ocean = ocean.state.expect("state part answered");
    let apple_rev = apple_rev.state.expect("state part answered");

    // The state is identical under two query contexts and under reversed candidate order.
    let digests = |r: &medulla_result_types::retrieval_round::StateResult| {
        let mut d: Vec<_> = r
            .scores
            .iter()
            .map(|s| (s.document_id.clone(), s.state_digest.clone()))
            .collect();
        d.sort();
        d
    };
    assert_eq!(digests(&apple), digests(&ocean));
    assert_eq!(digests(&apple), digests(&apple_rev));
    // Scores stay aligned per document under reversed candidate order.
    assert_eq!(apple.scores, apple_rev.scores);
    assert_eq!(apple.result_digest, apple_rev.result_digest);
    // The two query contexts rank differently, from the same stored state.
    assert_eq!(ocean.scores[0].document_id, "sdoc-b");
    assert_ne!(apple.scores[0].document_id, "sdoc-b");
    // No decoder or generation call, and the state costs less than the full-token baseline.
    assert_eq!(apple.decoder_calls, 0);
    assert!(apple.state_ops > 0 && apple.state_ops < apple.baseline_ops);
    // Revisions are reported with each score.
    assert!(apple.scores.iter().all(|s| {
        s.state_revision == medulla_plan::doc_state::BUILTIN_ENCODER_REVISION
            && s.segment_policy == "segment_bytes=512"
            && s.scorer_revision == medulla_plan::doc_state::SCORER_REVISION
    }));
}

// spec: MEDULLA-FEDERATED-QUERY-R081.101.3
#[tokio::test(flavor = "multi_thread")]
async fn missing_state_refuses_stale_and_writes_nothing() {
    let docs = state_docs();
    let state = state();
    seed_state_docs(&state, &docs, false).await;
    let response = round_as(&state, None, state_request("apple")).await;
    let error = response.error.expect("a missing state refuses");
    assert!(
        error.contains("RETRIEVAL_STATE_STALE"),
        "{error} {:?}",
        response.error_detail
    );
    // The refused read built nothing: the store still has no row for any document.
    let guard = state.read().await;
    let store = medulla_persistence::doc_state_store::shared_doc_state_store(&guard).unwrap();
    for (id, text) in &docs {
        let digest = medulla_plan::doc_state::content_digest(text);
        assert!(store
            .get(
                id,
                &digest,
                medulla_plan::doc_state::BUILTIN_ENCODER_REVISION,
                "segment_bytes=512"
            )
            .unwrap()
            .is_none());
    }
}
