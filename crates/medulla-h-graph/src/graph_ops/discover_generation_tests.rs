//! One index generation per `Discover` response (MEDULLA-DECISION-ENGINE-R174.8.3.1).
//! A swap of the resident index while calls are running never lets hits of one
//! generation be reported under another.

use super::discover_plan::DeepArms;
use super::semantic::{discover, DiscoverCall};
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_query_types::wire::DiscoverResult;
use std::sync::atomic::{AtomicBool, Ordering};

/// Above the ANN build threshold (4096), so a ready index image exists.
const NODES: usize = 4_600;
const DIM: usize = 32;
const CALLS: usize = 100;

fn vector(i: usize) -> Vec<f32> {
    let mut seed = (i as u64).wrapping_mul(2862933555777941757).wrapping_add(3037000493);
    (0..DIM)
        .map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
        })
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

fn result_of(response: &Response) -> DiscoverResult {
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone()).unwrap(),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected a discover answer, got {other:?} / {:?}", response.error),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R174.8.3.1
#[test]
fn concurrent_readers_see_one_generation() {
    let core = GraphCore::new();
    let vectors: Vec<Vec<f32>> = (0..NODES).map(vector).collect();
    for (i, v) in vectors.iter().enumerate() {
        core.semantic_store
            .write()
            .add_embedding(format!("n{i}"), v.clone())
            .unwrap();
    }
    core.semantic_store.read().warm("discover-generation");
    let image = core.semantic_store.read().export_generation().unwrap();

    // The arena never changes, so every generation ranks the same hit: the query is
    // the stored vector of `n0`, whose exact top-1 is itself.
    let query = vectors[0].clone();
    let exact = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (format!("n{i}"), cosine(&query, v)))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|(id, _)| id)
        .unwrap();
    assert_eq!(exact, "n0");

    let keywords: Vec<String> = Vec::new();
    let call = DiscoverCall {
        keywords: &keywords,
        query_embedding: &query,
        k: 1,
        hops: 0,
        as_of: None,
        graph_name: "default",
        tenant: None,
        adapter_digest: None,
        quality_policy_ref: None,
        implicit_feedback: &[],
        allowed: None,
        scope: None,
        scope_digest: "",
    };

    let stop = AtomicBool::new(false);
    let (mut succeeded, mut refused) = (0_usize, 0_usize);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Acquire) {
                core.semantic_store.read().adopt_generation(&image).unwrap();
            }
        });
        for n in 0..CALLS {
            let before = core.semantic_store.read().generation();
            let response = discover(&core, &call, DeepArms::default(), n as u64);
            let after = core.semantic_store.read().generation();
            if let Some(error) = response.error.as_deref() {
                assert!(error.starts_with("INDEX_UNAVAILABLE"), "{error}");
                refused += 1;
                continue;
            }
            let answer = result_of(&response);
            let ids: Vec<&str> = answer.hits.iter().map(|hit| hit.id.as_str()).collect();
            assert_eq!(ids, [exact.as_str()], "call {n} answers the baseline hits");
            let stamp = answer.verdict.index_generation;
            assert!(
                before <= stamp && stamp <= after,
                "call {n}: generation {stamp} outside [{before}, {after}]"
            );
            succeeded += 1;
        }
        stop.store(true, Ordering::Release);
    });
    assert!(succeeded >= 1, "at least one call succeeds ({refused} refused)");
    assert_eq!(succeeded + refused, CALLS);
}
