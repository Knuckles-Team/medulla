//! MEDULLA-ORCH-VOCAB-R800.6 hot-path receipt: `render_template` over 100,000 renders.
//!
//! Budget (spec): p50 under 10 microseconds, p99 under 50 microseconds and under 4 KiB
//! allocated per render. The measured call is the function the served `AgentAssemble` render
//! step calls, over a template of the four slot types. Deterministic: no RNG. A plain `main`,
//! nightly only; the PR proof is the served test
//! `tests/orchestration/test_definitions_served.py`.
//!
//! Run: cargo bench --features server --bench definitions

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use medulla_work_types::orchestration::definitions::{
    render_template, PromptTemplate, SlotType, SlotValue, TemplateSlot,
};

const RENDERS: usize = 100_000;
const BUDGET_P50_NS: u128 = 10_000;
const BUDGET_P99_NS: u128 = 50_000;
const BUDGET_ALLOC_BYTES: u64 = 4 * 1024;

/// Counts the bytes each allocation asks for, so one render's allocation is measurable.
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

fn template() -> (PromptTemplate, String) {
    let slot = |name: &str, slot_type| TemplateSlot {
        name: name.to_string(),
        slot_type,
    };
    (
        PromptTemplate {
            id: "greet".to_string(),
            revision_ref: "program:greet".to_string(),
            slots: vec![
                slot("name", SlotType::Text),
                slot("count", SlotType::Integer),
                slot("task", SlotType::Iri),
                slot("tags", SlotType::List),
            ],
            digest: format!("sha256:{}", "0".repeat(64)),
        },
        "Hi {{name}}: {{count}} items for {{task}} ({{tags}})".to_string(),
    )
}

fn percentile(sorted: &[u128], numerator: usize, denominator: usize) -> u128 {
    sorted[(sorted.len() * numerator / denominator).min(sorted.len() - 1)]
}

fn main() {
    let (template, body) = template();
    let values = vec![
        ("name".to_string(), SlotValue::Text("Ada".to_string())),
        ("count".to_string(), SlotValue::Integer(3)),
        (
            "task".to_string(),
            SlotValue::Iri("urn:task:xyz".to_string()),
        ),
        (
            "tags".to_string(),
            SlotValue::List(vec!["a".to_string(), "b".to_string()]),
        ),
    ];
    let mut latencies_ns = Vec::with_capacity(RENDERS);
    let mut worst_alloc = 0_u64;
    for _ in 0..RENDERS {
        let before = ALLOCATED.load(Ordering::Relaxed);
        let started = Instant::now();
        let rendered = render_template(&template, &body, &values).expect("the template renders");
        latencies_ns.push(started.elapsed().as_nanos());
        worst_alloc = worst_alloc.max(ALLOCATED.load(Ordering::Relaxed) - before);
        assert_eq!(rendered, "Hi Ada: 3 items for urn:task:xyz (a, b)");
    }
    latencies_ns.sort_unstable();
    let p50 = percentile(&latencies_ns, 50, 100);
    let p99 = percentile(&latencies_ns, 99, 100);
    println!(
        "decide/definitions: {RENDERS} renders, p50 {p50} ns, p99 {p99} ns, \
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
        "a render allocated {worst_alloc} bytes, over {BUDGET_ALLOC_BYTES}"
    );
}
