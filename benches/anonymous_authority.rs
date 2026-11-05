//! MEDULLA-IDENTITY-R003.2 budget receipt: the bounded anonymous decoder
//! (`medulla_server_api::anonymous::request_authority`) measured apart from any
//! graph operation.
//!
//! Budget (spec): p99 at most 100 µs per decode and at most 8 KiB allocated per
//! decode, over metadata at the 2 KiB bound with a 256-byte correlation key.
//! A counting allocator local to this bench counts the bytes each decode
//! allocates. The receipt line carries `host` and `revision`.
//!
//! Run: cargo bench --bench anonymous_authority

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use medulla_protocol_types::protocol::{Method, Request};
use medulla_server_api::anonymous::{
    request_authority, MAX_CORRELATION_KEY_BYTES, MAX_METADATA_BYTES,
};

struct CountingAllocator;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: forwards the caller's layout contract to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was returned by `System.alloc` with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

const ITERATIONS: usize = 20_000;
const WARMUP: usize = 1_000;
const P99_BUDGET_NS: u128 = 100_000;
const ALLOC_BUDGET_BYTES: usize = 8 * 1024;

fn bound_request() -> Request {
    let key = "k".repeat(MAX_CORRELATION_KEY_BYTES);
    let body = format!("{{\"idempotency_key\":\"{key}\"}}");
    let padded = format!("{body}{}", " ".repeat(MAX_METADATA_BYTES - body.len()));
    Request {
        id: 1,
        graph: "__commons__".into(),
        agent_id: None,
        auth_token: format!("anon1.{padded}"),
        method: Method::Ping,
    }
}

fn percentile(sorted: &[u128], pct: usize) -> u128 {
    sorted[(sorted.len() * pct / 100).min(sorted.len() - 1)]
}

fn main() {
    let request = bound_request();
    for _ in 0..WARMUP {
        std::hint::black_box(request_authority(&request).expect("bound admits"));
    }
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut max_alloc = 0usize;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED.load(Ordering::Relaxed);
        let start = Instant::now();
        let context = request_authority(std::hint::black_box(&request)).expect("bound admits");
        let elapsed = start.elapsed().as_nanos();
        let allocated = ALLOCATED.load(Ordering::Relaxed) - before;
        drop(std::hint::black_box(context));
        samples.push(elapsed);
        max_alloc = max_alloc.max(allocated);
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 50);
    let p99 = percentile(&samples, 99);
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .unwrap_or_default();
    let revision = option_env!("MEDULLA_GIT_REVISION").unwrap_or("unknown");
    println!(
        "anonymous_authority host={} revision={revision} p50_ns={p50} p99_ns={p99} \
         max_alloc_bytes={max_alloc} budget_p99_ns={P99_BUDGET_NS} budget_alloc_bytes={ALLOC_BUDGET_BYTES}",
        host.trim()
    );
    assert!(p99 <= P99_BUDGET_NS, "p99 {p99} ns over the 100 µs budget");
    assert!(
        max_alloc <= ALLOC_BUDGET_BYTES,
        "decode allocated {max_alloc} bytes, over the 8 KiB budget"
    );
}
