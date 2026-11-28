// MEDULLA-INGEST-WORKER-R606.6: the service level of `prepare` at batch size. Nightly bench
// group `ingest/materialize` calls the served `SourceIngest` (route, then `prepare`) with 100
// batches of 1,024 records, the batch bound `MAX_SOURCE_RECORDS`, over the served connector-pack
// harness: a seeded connector pack and the blob store. Budget per batch: p50 under 120 ms and
// p99 under 300 ms. A service level, not a pull request gate, so the run is ignored by default
// and the nightly job runs it with `--ignored`.

use std::time::{Duration, Instant};

use serde_json::json;

use medulla_served_test_harness::connector_pack_harness::{Served, GRAPH};

const CONNECTOR: &str = "materialize-bench-source";
const STREAM: &str = "doc";
const BATCHES: usize = 100;
const RECORDS: usize = 1_024;
const BUDGET_P50: Duration = Duration::from_millis(120);
const BUDGET_P99: Duration = Duration::from_millis(300);

// spec: MEDULLA-INGEST-WORKER-R606.6
#[tokio::test(flavor = "multi_thread")]
#[ignore = "nightly service level of MEDULLA-INGEST-WORKER-R606.6"]
async fn ingest_materialize_stays_within_the_batch_budget() {
    let served = Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "Document")]).await;
    let mut samples = Vec::with_capacity(BATCHES);
    for batch in 0..BATCHES {
        let ids: Vec<String> = (0..RECORDS)
            .map(|index| format!("r-{batch}-{index}"))
            .collect();
        let records: Vec<(&str, serde_json::Value)> = ids
            .iter()
            .map(|id| {
                (
                    id.as_str(),
                    json!({"id": id, "title": format!("record {id}")}),
                )
            })
            .collect();
        let began = Instant::now();
        let receipt = served.ingest(STREAM, &records).await;
        samples.push(began.elapsed());
        assert_eq!(receipt.affected_count as usize, RECORDS);
    }
    samples.sort();
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!(
        "ingest/materialize prepare p50 = {p50:?} (budget {BUDGET_P50:?}), p99 = {p99:?} (budget {BUDGET_P99:?}) per batch of {RECORDS}"
    );
    assert!(p50 < BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
