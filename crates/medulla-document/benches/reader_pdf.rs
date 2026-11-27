//! MEDULLA-INGEST-WORKER-R700.6: read a 5 MiB, 1,000-page PDF through
//! `ReaderRegistry::read`; JSON on stdout. Group `ingest/reader_pdf`.
//! Run with `cargo bench -p medulla-document --features reader-pdf --bench reader_pdf`.
//! Budget per page batch (10 pages, the whole-read time divided by 100): p50 under
//! 20 ms, p99 under 120 ms, peak memory under 128 MiB.

use medulla_document::readers::{text_registry, ReaderLimits};
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

const PAGES: usize = 1000;
const STREAM_BYTES: usize = 5120;
const PAGES_PER_BATCH: usize = 10;
const SAMPLES: usize = 20;
const PEAK_BUDGET: usize = 128 * 1024 * 1024;

/// A PDF of `pages` Helvetica pages, each with a content stream of exactly
/// `STREAM_BYTES` bytes whose text is padded with `lorem ` repeated.
fn pdf_of_pages(pages: usize) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut object = |out: &mut Vec<u8>, body: Vec<u8>| {
        offsets.push(out.len());
        let number = offsets.len();
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(b"\nendobj\n");
    };
    object(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids: String = (0..pages)
        .map(|i| format!("{} 0 R", 4 + 2 * i))
        .collect::<Vec<_>>()
        .join(" ");
    object(
        &mut out,
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").into_bytes(),
    );
    object(
        &mut out,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    );
    for i in 0..pages {
        let content = 5 + 2 * i;
        object(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {content} 0 R >>"
            )
            .into_bytes(),
        );
        let head = format!("BT /F1 12 Tf 72 720 Td (page {} ", i + 1);
        let tail = ") Tj ET";
        let filler_len = STREAM_BYTES - head.len() - tail.len();
        let filler: String = "lorem ".repeat(filler_len / 6 + 1)[..filler_len].to_string();
        let stream = format!("{head}{filler}{tail}");
        assert_eq!(stream.len(), STREAM_BYTES);
        let mut body = format!("<< /Length {STREAM_BYTES} >>\nstream\n").into_bytes();
        body.extend_from_slice(stream.as_bytes());
        body.extend_from_slice(b"\nendstream");
        object(&mut out, body);
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", offsets.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let bytes = pdf_of_pages(PAGES);
    assert!(
        (5_000_000..=5_400_000).contains(&bytes.len()),
        "the fixture is {} bytes",
        bytes.len()
    );
    let registry =
        text_registry(ReaderLimits::new(64 << 20, 4).expect("limits")).expect("registry");
    let read = || {
        registry
            .read("application/pdf", black_box(&bytes))
            .expect("read")
            .len()
    };
    assert_eq!(read(), PAGES, "one fragment per page");
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    black_box(read());
    let peak_bytes = PEAK.load(Ordering::Relaxed) - base;
    let batches = (PAGES / PAGES_PER_BATCH) as u128;
    let mut micros = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        black_box(read());
        micros.push(start.elapsed().as_micros() / batches);
    }
    micros.sort_unstable();
    let (p50, p99) = (percentile(&micros, 0.50), percentile(&micros, 0.99));
    println!(
        "{{\"bench\":\"ingest/reader_pdf\",\"bytes\":{},\"pages\":{PAGES},\"samples\":{SAMPLES},\
         \"p50_us_per_batch\":{p50},\"p99_us_per_batch\":{p99},\"peak_bytes\":{peak_bytes},\
         \"budget_p50_us\":20000,\"budget_p99_us\":120000,\"optimized\":{}}}",
        bytes.len(),
        !cfg!(debug_assertions)
    );
    assert!(
        peak_bytes < PEAK_BUDGET,
        "peak memory {peak_bytes} over budget"
    );
    if !cfg!(debug_assertions) {
        assert!(
            p50 < 20_000 && p99 < 120_000,
            "reader_pdf exceeded its budget"
        );
    }
}
