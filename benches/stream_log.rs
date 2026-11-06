//! MEDULLA-DURABLE-KERNEL-R869 budget receipt: the replayable append-only stream
//! log (`medulla_core::broker::stream_publish` / `stream_read`) measured apart
//! from the wire.
//!
//! Budget (spec): append p50 under 1 ms and append p99 under 5 ms; a read of
//! 100 messages stays under 3 ms at p99. The receipt line carries `host` and
//! `revision`.
//!
//! Run: cargo bench --features broker --bench stream_log

use std::time::Instant;

use medulla_core::broker::{stream_publish, stream_read, ReadFrom};
use medulla_graph_core::graph::GraphCore;

const APPENDS: usize = 10_000;
const READS: usize = 1_000;
const READ_BATCH: usize = 100;
const APPEND_P50_BUDGET_NS: u128 = 1_000_000;
const APPEND_P99_BUDGET_NS: u128 = 5_000_000;
const READ_P99_BUDGET_NS: u128 = 3_000_000;
const PAYLOAD: [u8; 256] = [0x5a; 256];

fn percentile(sorted: &[u128], pct: usize) -> u128 {
    sorted[(sorted.len() * pct / 100).min(sorted.len() - 1)]
}

fn main() {
    let core = GraphCore::new();
    let mut appends = Vec::with_capacity(APPENDS);
    for index in 0..APPENDS {
        let start = Instant::now();
        let offset = stream_publish(&core, "bench", std::hint::black_box(&PAYLOAD), index as u64);
        appends.push(start.elapsed().as_nanos());
        assert_eq!(offset, index as i64, "offsets are gap-free and monotonic");
    }
    let mut reads = Vec::with_capacity(READS);
    for index in 0..READS {
        let from = ((index * READ_BATCH) % (APPENDS - READ_BATCH)) as i64;
        let start = Instant::now();
        let batch = stream_read(&core, "bench", ReadFrom::Offset(from), READ_BATCH);
        reads.push(start.elapsed().as_nanos());
        assert_eq!(batch.len(), READ_BATCH);
        assert_eq!(batch[0].0, from);
    }
    appends.sort_unstable();
    reads.sort_unstable();
    let append_p50 = percentile(&appends, 50);
    let append_p99 = percentile(&appends, 99);
    let read_p99 = percentile(&reads, 99);
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .unwrap_or_default();
    let revision = option_env!("MEDULLA_GIT_REVISION").unwrap_or("unknown");
    println!(
        "stream_log host={} revision={revision} append_p50_ns={append_p50} \
         append_p99_ns={append_p99} read100_p99_ns={read_p99}",
        host.trim()
    );
    assert!(
        append_p50 < APPEND_P50_BUDGET_NS,
        "append p50 {append_p50} ns over 1 ms"
    );
    assert!(
        append_p99 < APPEND_P99_BUDGET_NS,
        "append p99 {append_p99} ns over 5 ms"
    );
    assert!(
        read_p99 < READ_P99_BUDGET_NS,
        "read-100 p99 {read_p99} ns over 3 ms"
    );
}
