//! MEDULLA-REPO-INGEST-R652.6: chunk a 1 MiB Markdown document and build and
//! verify its section tree; JSON on stdout.
//! Run with `cargo bench -p medulla-document --bench document_chunk_tree`.
//! Budget: p50 3 ms and p99 15 ms per document in the optimized profile, and
//! peak heap growth during one run at most twice the document size.

use medulla_document::processing::{
    build_section_tree, chunk_text, verify_section_tree, ChunkingConfig, SectionSource,
    SectionTreeCheck,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// Counts live and peak heap bytes so the run can check its memory budget.
struct PeakAlloc;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for PeakAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwards the caller's layout contract to the system allocator.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` above with this same layout.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: PeakAlloc = PeakAlloc;

/// One served-path run: chunk the text, build its section tree, verify it.
fn process(text: &str, config: &ChunkingConfig) -> usize {
    let chunks = chunk_text(text, config).unwrap();
    let tree = build_section_tree(text, SectionSource::Markdown);
    assert_eq!(
        verify_section_tree(&tree, text.len()),
        SectionTreeCheck::Valid
    );
    chunks.len() + tree.len()
}

const SAMPLES: usize = 200;
const WARMUP: usize = 20;
const TARGET_BYTES: usize = 1024 * 1024;

// spec: MEDULLA-REPO-INGEST-R652.6
fn markdown() -> String {
    let paragraph = "Retrieval addresses a passage by its offsets. Each window ends at a \
                     paragraph or sentence break, and windows overlap so a fact split across \
                     a cut is still whole in one chunk. Ünïcode text keeps exact offsets.\n\n";
    let mut text = String::with_capacity(TARGET_BYTES + 4096);
    let mut section = 0;
    while text.len() < TARGET_BYTES {
        section += 1;
        text.push_str(&format!("## Section {section}\n\n"));
        for _ in 0..6 {
            text.push_str(paragraph);
        }
    }
    text
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let text = markdown();
    let config = ChunkingConfig::default();
    for _ in 0..WARMUP {
        black_box(process(black_box(&text), &config));
    }
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    black_box(process(black_box(&text), &config));
    let peak_bytes = PEAK.load(Ordering::Relaxed) - base;
    let mut micros = Vec::with_capacity(SAMPLES);
    let mut chunks = 0;
    for _ in 0..SAMPLES {
        let start = Instant::now();
        chunks = black_box(process(black_box(&text), &config));
        micros.push(start.elapsed().as_micros());
    }
    micros.sort_unstable();
    let (p50, p99) = (percentile(&micros, 0.50), percentile(&micros, 0.99));
    println!(
        "{{\"bench\":\"document_chunk_tree\",\"bytes\":{},\"chunks_and_sections\":{chunks},\"samples\":{SAMPLES},\
         \"p50_us\":{p50},\"peak_bytes\":{peak_bytes},\"p99_us\":{p99},\"budget_p50_us\":3000,\"budget_p99_us\":15000,\
         \"optimized\":{}}}",
        text.len(),
        !cfg!(debug_assertions)
    );
    assert!(
        peak_bytes <= 2 * text.len(),
        "peak memory {peak_bytes} exceeded twice the document size"
    );
    if !cfg!(debug_assertions) {
        assert!(
            p50 <= 3_000 && p99 <= 15_000,
            "chunking exceeded its budget"
        );
    }
}
