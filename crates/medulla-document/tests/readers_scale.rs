//! Readers stream: time and memory grow linearly with the source
//! (MEDULLA-INGEST-WORKER-R700.2).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use medulla_document::readers::{text_registry, ReaderLimits, READER_TOO_LARGE};

struct Counting;
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = CURRENT.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
        PEAK.fetch_max(now, Ordering::SeqCst);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const SIZES: [usize; 3] = [10, 1_000, 100_000];

fn csv(units: usize) -> String {
    (0..units)
        .map(|i| format!("row{i},\"a\nb{i}\",{i}\n"))
        .collect()
}

fn vtt(units: usize) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for i in 0..units {
        out.push_str(&format!(
            "00:00:{:02}.000 --> 00:00:{:02}.500\ncue {i}\n\n",
            i % 60,
            i % 60
        ));
    }
    out
}

fn html(units: usize) -> String {
    let mut out = String::from("<!doctype html><html><body>");
    for i in 0..units {
        out.push_str(&format!("<p>paragraph {i}</p>\n"));
    }
    out.push_str("</body></html>");
    out
}

struct Run {
    elapsed: Duration,
    peak: usize,
}

fn measure(
    media_type: &str,
    source: &str,
    parallelism: usize,
) -> (Run, Vec<medulla_document::fragment::Fragment>) {
    let registry = text_registry(ReaderLimits::new(MAX_BYTES, parallelism).unwrap()).unwrap();
    let base = CURRENT.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let started = Instant::now();
    let fragments = registry.read(media_type, source.as_bytes()).unwrap();
    let elapsed = started.elapsed();
    let peak = PEAK.load(Ordering::SeqCst) - base;
    (Run { elapsed, peak }, fragments)
}

/// A media type and the generator of a source document of that type.
type FormatGenerator = (&'static str, fn(usize) -> String);

// spec: MEDULLA-INGEST-WORKER-R700.2
#[test]
fn time_and_memory_scale_linearly() {
    let formats: [FormatGenerator; 3] = [("text/csv", csv), ("text/vtt", vtt), ("text/html", html)];
    for (media_type, make) in formats {
        let mut runs = Vec::new();
        for units in SIZES {
            let source = make(units);
            let (run, fragments) = measure(media_type, &source, 4);
            assert!(!fragments.is_empty(), "{media_type} {units}: no fragments");
            // Source order: spans are contiguous, ordinals ascend, nothing is lost.
            let mut next_byte = 0;
            for (ordinal, fragment) in fragments.iter().enumerate() {
                assert_eq!(fragment.span.ordinal as usize, ordinal);
                if media_type != "text/html" {
                    assert_eq!(fragment.span.start_byte, next_byte);
                    assert_eq!(
                        &source.as_bytes()[fragment.span.start_byte..fragment.span.end_byte],
                        fragment.text.as_bytes()
                    );
                }
                next_byte = fragment.span.end_byte;
            }
            if media_type != "text/html" {
                assert_eq!(next_byte, source.len());
            }
            // Memory: within twice the byte limit, and for the line formats
            // within twice the source itself (one copy of the text).
            assert!(
                run.peak <= 2 * MAX_BYTES,
                "{media_type} {units}: peak {}",
                run.peak
            );
            if media_type != "text/html" {
                assert!(
                    run.peak <= 2 * source.len() + 64 * 1024,
                    "{media_type} {units}: peak {} for {} bytes",
                    run.peak,
                    source.len()
                );
            }
            runs.push((source.len(), run));
        }
        // Time: the per-byte cost of the largest source stays within a constant
        // factor of the mid-size source's, with a floor for scheduler noise.
        let (mid_bytes, mid) = &runs[1];
        let (big_bytes, big) = &runs[2];
        let allowed =
            mid.elapsed.as_secs_f64() * (*big_bytes as f64 / *mid_bytes as f64) * 8.0 + 0.05;
        assert!(
            big.elapsed.as_secs_f64() <= allowed,
            "{media_type}: {:?} for {big_bytes} bytes vs {:?} for {mid_bytes}",
            big.elapsed,
            mid.elapsed
        );
    }

    // Parallelism changes nothing about the output.
    let source = csv(100_000);
    let (_, serial) = measure("text/csv", &source, 1);
    let (_, parallel) = measure("text/csv", &source, 8);
    assert_eq!(serial, parallel);

    // A source over the limit is refused, never read.
    let tiny = text_registry(ReaderLimits::new(8, 1).unwrap()).unwrap();
    assert_eq!(
        tiny.read("text/csv", b"a,b,c,d,e,f\n").unwrap_err().code,
        READER_TOO_LARGE
    );
}

/// Every text-layer fragment re-extracts from its span: re-reading the source
/// bytes at the fragment's byte span yields the fragment text, the character
/// span is consistent with it, and the fragments cover the source in order.
// spec: MEDULLA-INGEST-WORKER-R700.5
#[test]
fn spans_reextract_to_fragment_text() {
    let registry = text_registry(ReaderLimits::new(MAX_BYTES, 2).unwrap()).unwrap();
    type SourceMaker = fn(usize) -> String;
    let formats: [(&str, SourceMaker); 2] = [("text/csv", csv), ("text/vtt", vtt)];
    for (media_type, make) in formats {
        for units in [10, 1_000] {
            let source = make(units);
            let fragments = registry.read(media_type, source.as_bytes()).unwrap();
            assert!(!fragments.is_empty(), "{media_type} {units}: no fragments");
            for fragment in &fragments {
                let span = &fragment.span;
                let reread = &source.as_bytes()[span.start_byte..span.end_byte];
                assert_eq!(
                    reread,
                    fragment.text.as_bytes(),
                    "{media_type} {units}: fragment {} does not re-extract",
                    span.ordinal
                );
                assert_eq!(
                    std::str::from_utf8(reread).unwrap().chars().count(),
                    span.end_char - span.start_char,
                    "{media_type} {units}: character span of fragment {}",
                    span.ordinal
                );
            }
            // Re-extracting every span in order loses nothing of the source.
            let joined: Vec<u8> = fragments
                .iter()
                .flat_map(|f| source.as_bytes()[f.span.start_byte..f.span.end_byte].to_vec())
                .collect();
            assert_eq!(joined, source.as_bytes(), "{media_type} {units}");
        }
    }
}
