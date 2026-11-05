//! MEDULLA-FEDERATED-QUERY-R053.701: hot-path cost of `SubmissionId::derive`.
//! Target: p50 under 0.02 ms, p99 under 0.2 ms, memory under 8 MiB. Reported,
//! never a PR gate.

use std::collections::BTreeMap;
use std::time::Instant;

use medulla_federation::federation_spark::SubmissionId;

fn main() {
    let mut params = BTreeMap::new();
    params.insert("window".to_string(), "7d".to_string());
    params.insert("tenant".to_string(), "commons".to_string());
    let digests = ["sha256:aa", "sha256:bb", "sha256:cc"];
    let mut samples = Vec::with_capacity(10_000);
    for _ in 0..10_000 {
        let start = Instant::now();
        let id = SubmissionId::derive("daily", &digests, &params);
        std::hint::black_box(id);
        samples.push(start.elapsed().as_nanos() as u64);
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2] as f64 / 1e6;
    let p99 = samples[samples.len() * 99 / 100] as f64 / 1e6;
    println!("spark_submission_id p50_ms={p50:.5} p99_ms={p99:.5}");
}
