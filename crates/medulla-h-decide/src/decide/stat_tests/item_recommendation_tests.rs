//! Item recommendation by residual quantization (MEDULLA-DECISION-ENGINE-R175.2):
//! pause steps refine the query toward the history, so the ranking over a
//! seeded catalog of four clusters never loses NDCG.

use medulla_decision_stats::decision::recommend::{
    encode_catalog, fit_codebooks, refine_and_rank, CatalogItem,
};
use medulla_decision_types::decision::knob_families::recommend::RecommendKnobs;

const TOP_K: u16 = 6;

fn catalog() -> Vec<CatalogItem> {
    let mut items = Vec::new();
    for cluster in 0..4usize {
        for member in 0..6usize {
            let mut vector = vec![0.05f32 * member as f32; 4];
            vector[cluster] = 1.0 + 0.01 * member as f32;
            items.push(CatalogItem {
                item_id: format!("c{cluster}-i{member}"),
                time_bucket: 0,
                vector,
            });
        }
    }
    items
}

fn ndcg(ranked: &[String], relevant_prefix: &str) -> f64 {
    let gain = |position: usize| 1.0 / ((position as f64) + 2.0).log2();
    let dcg: f64 = ranked
        .iter()
        .enumerate()
        .filter(|(_, id)| id.starts_with(relevant_prefix))
        .map(|(position, _)| gain(position))
        .sum();
    let ideal: f64 = (0..usize::from(TOP_K)).map(gain).sum();
    dcg / ideal
}

fn ranked_ids(pause_steps: u8) -> Vec<String> {
    let knobs = RecommendKnobs {
        levels: 2,
        codes_per_level: 4,
        kmeans_iters: 10,
        pause_alpha: 0.5,
        nearest_pull: 0.25,
    };
    let items = catalog();
    let mut digest = [0u8; 32];
    digest[..8].copy_from_slice(&0x0123_4567_89ab_cdefu64.to_be_bytes());
    let encoder = fit_codebooks(&items, &digest, &knobs).expect("fit");
    let encoded = encode_catalog(&encoder, &items).expect("encode");
    refine_and_rank(
        &encoder,
        &encoded,
        &[0.7, 0.6, 0.0, 0.0],
        &["c0-i0".to_string(), "c0-i1".to_string()],
        pause_steps,
        TOP_K,
        &knobs,
    )
    .expect("rank")
    .into_iter()
    .map(|item| item.item_id)
    .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R175.2
#[test]
fn pause_steps_lift_ndcg() {
    let without = ndcg(&ranked_ids(0), "c0-");
    let with = ndcg(&ranked_ids(2), "c0-");
    assert!(with >= without, "{with} < {without}");
}
