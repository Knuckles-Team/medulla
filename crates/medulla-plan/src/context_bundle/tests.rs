//! L1 and L2 proofs of the bundle model and compile (R650.1, R650.2).

use super::*;
use medulla_decision_types::decision::context_package::ContextPackageBudget;

const D1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const D2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn citation(source: &str, digest: &str) -> Citation {
    Citation {
        source_id: source.into(),
        cited_start: 0,
        cited_end: 100,
        source_revision: 1,
        content_digest: digest.into(),
    }
}

fn span(resolution: u8, tokens: u64, digest: &str) -> ResolutionSpan {
    ResolutionSpan {
        resolution,
        tokens,
        citation: citation("doc:x", digest),
    }
}

fn digest_for(n: usize) -> String {
    format!("{n:064x}")
}

fn hit(node: &str, score: f32, n: usize) -> SourceHit {
    SourceHit {
        node_id: node.into(),
        score,
        spans: vec![
            span(0, 20, &digest_for(n)),
            span(1, 80, &digest_for(n + 1000)),
        ],
        hop_path: vec![ProofEdge {
            from: node.into(),
            relationship: "mentions".into(),
            to: "seed".into(),
        }],
        derived_from: Vec::new(),
    }
}

fn request(window: u64) -> ContextBundleRequest {
    ContextBundleRequest {
        query_text: "invoice".into(),
        query_embedding: vec![0.1, 0.2],
        budget: ContextPackageBudget {
            window_tokens: window,
            reserved_output_tokens: 0,
            caller_ceiling_tokens: None,
            money_ceiling_micro: None,
            price_micro_per_token: None,
            latency_ceiling_ms: None,
            tokens_per_second: None,
        },
        tokenizer_id: "tok".into(),
        tokenizer_version: "1".into(),
        mode: ContextMode::Hybrid,
        as_of: None,
        agent_ref: Some("agent:a".into()),
        top_n: DEFAULT_TOP_N,
    }
}

fn inputs() -> CompileInputs {
    // Two sources agree on n1..n3; n4 and n5 trail far behind.
    let lexical = SourceRanking {
        source: "lexical".into(),
        curve: None,
        hits: vec![
            hit("n1", 9.0, 1),
            hit("n2", 8.5, 2),
            hit("n3", 8.0, 3),
            hit("n4", 0.2, 4),
            hit("n5", 0.1, 5),
        ],
    };
    let semantic = SourceRanking {
        source: "semantic".into(),
        curve: None,
        hits: vec![hit("n2", 0.95, 2), hit("n1", 0.9, 1), hit("n3", 0.85, 3)],
    };
    CompileInputs {
        tenant: "t1".into(),
        graph_revision: 7,
        policy_digest: D1.into(),
        rankings: vec![semantic, lexical],
        pre_excluded: vec![ExcludedItem {
            item_id: "secret".into(),
            reason: ExclusionReason::Denied,
        }],
        unchecked_dropped: vec![DroppedRecord {
            record_id: "r9".into(),
            reason: DroppedReason::Timeout,
        }],
        knobs: ContextKnobs::default(),
        rerank: None,
    }
}

#[test]
fn bundle_types_round_trip_and_reject_unknown_reason() {
    let bundle = compile(&request(500), &inputs()).unwrap();
    let wire = serde_json::to_string(&bundle).unwrap();
    assert_eq!(
        serde_json::from_str::<ContextBundle>(&wire).unwrap(),
        bundle
    );
    let req = request(500);
    let wire = serde_json::to_string(&req).unwrap();
    assert_eq!(
        serde_json::from_str::<ContextBundleRequest>(&wire).unwrap(),
        req
    );
    // top_n defaults when absent and is range checked.
    let mut value: serde_json::Value = serde_json::from_str(&wire).unwrap();
    value.as_object_mut().unwrap().remove("top_n");
    let parsed: ContextBundleRequest = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.top_n, 256);
    assert!(parsed.top_n_in_range());
    let mut wide = parsed;
    wide.top_n = 1_025;
    assert!(!wide.top_n_in_range());
    // The reason set is closed.
    for ok in [
        "below_gap",
        "over_budget",
        "denied",
        "out_of_scope",
        "dominated",
    ] {
        let item = format!(r#"{{"item_id":"x","reason":"{ok}"}}"#);
        assert!(serde_json::from_str::<ExcludedItem>(&item).is_ok());
    }
    assert!(serde_json::from_str::<ExcludedItem>(r#"{"item_id":"x","reason":"because"}"#).is_err());
    assert!(
        serde_json::from_str::<ExcludedItem>(r#"{"item_id":"x","reason":"denied","extra":1}"#)
            .is_err()
    );
    // The key is 64 hex characters and differs per tenant.
    assert_eq!(bundle.key.0.len(), 64);
    let mut other = inputs();
    other.tenant = "t2".into();
    assert_ne!(bundle_key(&request(500), &other), bundle.key);
}

#[test]
fn cut_select_and_exclusion_reasons() {
    // A wide window keeps the widest resolution of each kept item.
    let wide = compile(&request(1_000), &inputs()).unwrap();
    let ids: Vec<&str> = wide.items.iter().map(|i| i.item_id.as_str()).collect();
    assert_eq!(ids, ["n1", "n2", "n3"]);
    assert!(wide
        .items
        .iter()
        .all(|i| i.resolution == 1 && i.tokens == 80));
    assert!(wide.items.iter().all(|i| i.proof_edges.len() == 1));
    let reasons: BTreeMap<&str, ExclusionReason> = wide
        .excluded
        .iter()
        .map(|e| (e.item_id.as_str(), e.reason))
        .collect();
    assert_eq!(reasons["n4"], ExclusionReason::BelowGap);
    assert_eq!(reasons["n5"], ExclusionReason::BelowGap);
    assert_eq!(reasons["secret"], ExclusionReason::Denied);
    let certificate = wide.certificate.as_ref().unwrap();
    assert!(certificate.verified);
    assert!(certificate.total_tokens <= certificate.effective_capacity_tokens);
    assert_eq!(wide.evidence.unchecked_dropped.len(), 1);
    assert_eq!(wide.evidence.selected, ids);

    // A tight window downgrades resolution; one that fits only two units
    // excludes the third as over budget.
    let tight = compile(&request(45), &inputs()).unwrap();
    assert!(tight.items.iter().map(|i| i.tokens).sum::<u64>() <= 45);
    assert_eq!(tight.items.len(), 2);
    assert!(tight.items.iter().all(|i| i.resolution == 0));
    assert!(tight
        .excluded
        .iter()
        .any(|e| e.reason == ExclusionReason::OverBudget));

    // An item that duplicates a chosen item's content is dominated.
    let mut dup = inputs();
    dup.rankings[1].hits[2] = SourceHit {
        spans: dup.rankings[1].hits[0].spans.clone(),
        ..hit("n3", 8.0, 3)
    };
    let dominated = compile(&request(45), &dup).unwrap();
    assert!(dominated
        .excluded
        .iter()
        .any(|e| e.reason == ExclusionReason::Dominated));

    // A hit with no measured span is out of scope; no candidates, no solve.
    let mut bare = inputs();
    for ranking in &mut bare.rankings {
        for h in &mut ranking.hits {
            h.spans.clear();
        }
    }
    let empty = compile(&request(1_000), &bare).unwrap();
    assert!(empty.items.is_empty() && empty.certificate.is_none());
    assert!(empty
        .excluded
        .iter()
        .any(|e| e.reason == ExclusionReason::OutOfScope));
    let _ = D2;
}

#[test]
fn result_is_deterministic_across_thread_counts() {
    let baseline = compile(&request(100), &inputs()).unwrap();
    for threads in [1_usize, 2, 4, 8] {
        let results: Vec<ContextBundle> = std::thread::scope(|scope| {
            (0..threads)
                .map(|_| scope.spawn(|| compile(&request(100), &inputs()).unwrap()))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect()
        });
        assert!(results.iter().all(|r| *r == baseline), "{threads} threads");
    }
    // Source arrival order does not change the bundle.
    let mut reversed = inputs();
    reversed.rankings.reverse();
    assert_eq!(compile(&request(100), &reversed).unwrap(), baseline);
}

struct PrefersGarden;

impl medulla_exec::exec::Reranker for PrefersGarden {
    fn score(&self, _query: &str, texts: &[&str]) -> Vec<f32> {
        texts
            .iter()
            .map(|t| if t.contains("garden") { 1.0 } else { 0.0 })
            .collect()
    }
    fn revision(&self) -> String {
        "garden-encoder@sha256:aa".to_string()
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R652.7
#[test]
fn rerank_runs_after_fuse_and_names_the_model() {
    use crate::rerank::{ModelBinding, ModelScorer};
    let mut base = inputs();
    let docs = ["n1", "n2", "n3", "n4", "n5"]
        .iter()
        .map(|id| {
            let name = if *id == "n3" { "garden" } else { "ledger" };
            (
                (*id).to_string(),
                RerankDoc {
                    name: name.to_string(),
                    ..RerankDoc::default()
                },
            )
        })
        .collect();
    base.rerank = Some(RerankInputs {
        query: "garden".to_string(),
        rerank_k: 3,
        curve: None,
        docs,
    });
    // Lexical only: the receipt names the lexical scorer; the two candidates past
    // rerank_k are excluded as not scored.
    let lexical = compile(&request(1_000), &base).unwrap();
    let receipt = lexical.evidence.rerank.as_ref().unwrap();
    assert_eq!(receipt.scorer_name, "lexical_bm25");
    assert_eq!(receipt.input_ranks.len(), 3);
    // A bound model decides the order and its revision is the scorer version.
    let model = PrefersGarden;
    let scorer = ModelScorer::new(&model);
    let modelled = compile_with(&request(1_000), &base, ModelBinding::Bound(&scorer)).unwrap();
    let receipt = modelled.evidence.rerank.as_ref().unwrap();
    assert_eq!(receipt.scorer_version, "garden-encoder@sha256:aa");
    assert_eq!(
        modelled.evidence.selected.first().map(String::as_str),
        Some("n3")
    );
    // A configured model that is not loaded falls back and says so.
    let missing = compile_with(&request(1_000), &base, ModelBinding::Unavailable).unwrap();
    let receipt = missing.evidence.rerank.as_ref().unwrap();
    assert_eq!(receipt.scorer_name, "lexical_bm25");
    assert_eq!(
        receipt.fallback,
        Some(medulla_decision_types::decision::context_bundle::FallbackReason::ScorerUnavailable)
    );
    // No rerank inputs, no receipt.
    base.rerank = None;
    assert!(compile(&request(1_000), &base)
        .unwrap()
        .evidence
        .rerank
        .is_none());
}
