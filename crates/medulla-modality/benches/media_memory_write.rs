//! Nightly bench `decide/media_memory_write` (MEDULLA-MEDIA-VOCAB-R650.6): `plan_media_write`
//! over a 1 MiB image with one 4 KiB caption and a 384-float embedding.
//!
//! A plain `main`, not a PR gate. It records p50 and p99 over 1000 calls and asserts the budgets:
//! p50 1 ms and p99 8 ms. The plan holds the blob reference and the derivation text and vector,
//! never the blob bytes, so its memory is bounded by the text and the vector.
//!
//!   cargo bench -p medulla-modality --bench media_memory_write

use medulla_modality::media_memory::{
    plan_media_write, DerivationInput, DerivationKind, MediaBlobRef,
};
use std::time::Instant;

const CALLS: usize = 1000;
const P50_BUDGET_MS: f64 = 1.0;
const P99_BUDGET_MS: f64 = 8.0;

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let at = ((sorted.len() as f64 * fraction).ceil() as usize).saturating_sub(1);
    sorted[at.min(sorted.len() - 1)]
}

fn media_memory_write() {
    let blob = MediaBlobRef {
        digest: "ab".repeat(32),
        mime: "image/png".to_string(),
        byte_len: 1_048_576,
    };
    let derivations = [DerivationInput {
        kind: DerivationKind::Caption,
        text: "a caption of the image ".repeat(4096 / 23 + 1)[..4096].to_string(),
        embedding: (0..384).map(|i| i as f32 / 384.0).collect(),
    }];
    let mut millis: Vec<f64> = Vec::with_capacity(CALLS);
    for _ in 0..CALLS {
        let started = Instant::now();
        let plan = plan_media_write(&blob, false, &derivations).expect("a well-formed write");
        std::hint::black_box(&plan);
        millis.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    millis.sort_by(f64::total_cmp);
    let (p50, p99) = (percentile(&millis, 0.50), percentile(&millis, 0.99));
    println!("decide/media_memory_write: p50 {p50:.4} ms, p99 {p99:.4} ms over {CALLS} calls");
    assert!(
        p50 <= P50_BUDGET_MS,
        "p50 {p50} ms is above {P50_BUDGET_MS} ms"
    );
    assert!(
        p99 <= P99_BUDGET_MS,
        "p99 {p99} ms is above {P99_BUDGET_MS} ms"
    );
}

// spec: MEDULLA-MEDIA-VOCAB-R650.6
fn main() {
    media_memory_write();
}
