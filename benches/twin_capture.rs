//! MEDULLA-ORCH-VOCAB-R750.753 hot-path receipt: capture the twin of a 1,000-node agent graph.
//!
//! Budget (spec): p50 under 8 ms, p99 under 20 ms and under 16 MiB allocated per capture. A
//! capture pins every node revision of the agent graph into one `AgentTwin` and encodes it, the
//! form the twin travels in a `recruit` request. Deterministic: no RNG. A plain `main`,
//! nightly only; the PR proof is the served journey
//! `tests/orchestration/test_org_runtime_served.py`.
//!
//! Run: cargo bench --features server --bench twin_capture

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use medulla_work_types::orchestration::org::AgentTwin;

const NODES: usize = 1_000;
const CAPTURES: usize = 200;
const BUDGET_P50_NS: u128 = 8_000_000;
const BUDGET_P99_NS: u128 = 20_000_000;
const BUDGET_ALLOC_BYTES: u64 = 16 * 1024 * 1024;

/// Counts the bytes each allocation asks for, so one capture's allocation is measurable.
struct Counting;

static ALLOCATED: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call forwards to the system allocator unchanged; the counter only observes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn percentile(sorted: &[u128], numerator: usize, denominator: usize) -> u128 {
    sorted[(sorted.len() * numerator / denominator).min(sorted.len() - 1)]
}

fn main() {
    // The revision each node of the agent graph is pinned at.
    let revisions: Vec<String> = (0..NODES)
        .map(|node| format!("node:{node:04}@rev-{node}"))
        .collect();
    let mut latencies_ns = Vec::with_capacity(CAPTURES);
    let mut worst_alloc = 0_u64;
    for _ in 0..CAPTURES {
        let before = ALLOCATED.load(Ordering::Relaxed);
        let started = Instant::now();
        let twin =
            AgentTwin::new("agent:bench", revisions.clone()).expect("the twin pins revisions");
        let encoded = serde_json::to_vec(&twin).expect("the twin encodes");
        latencies_ns.push(started.elapsed().as_nanos());
        worst_alloc = worst_alloc.max(ALLOCATED.load(Ordering::Relaxed) - before);
        let back: AgentTwin = serde_json::from_slice(&encoded).expect("the twin decodes");
        assert_eq!(back.pinned_revisions().len(), NODES);
    }
    latencies_ns.sort_unstable();
    let p50 = percentile(&latencies_ns, 50, 100);
    let p99 = percentile(&latencies_ns, 99, 100);
    println!(
        "decide/twin: {CAPTURES} captures of {NODES} nodes, p50 {p50} ns, p99 {p99} ns, \
         worst allocation {worst_alloc} bytes"
    );
    assert!(
        p50 < BUDGET_P50_NS,
        "p50 {p50} ns is over {BUDGET_P50_NS} ns"
    );
    assert!(
        p99 < BUDGET_P99_NS,
        "p99 {p99} ns is over {BUDGET_P99_NS} ns"
    );
    assert!(
        worst_alloc < BUDGET_ALLOC_BYTES,
        "a capture allocated {worst_alloc} bytes, over {BUDGET_ALLOC_BYTES}"
    );
}
