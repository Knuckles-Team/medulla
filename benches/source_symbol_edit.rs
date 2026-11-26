// spec: MEDULLA-SCM-R013.7
//! MEDULLA-SCM-R013.7 hot-path receipt: one symbol edit in a 5,000-line file, including the
//! re-parse.
//!
//! Budget (spec): p99 at most 30 ms per edit and resident growth at most 32 MiB. Each measured op is the engine's own pre-accept
//! path, `medulla_h_ingest::source_symbol_edit::prepare`: projection of the edit to file bytes,
//! the round-trip re-parse of the base and the projected file, and the changeset request build.
//! The accept path itself is the `SubmitChangeset` bench's subject.
//!
//! Deterministic: a fixed generated source, no RNG. The nightly suite stores the baseline and
//! fails on a 10 percent regression.
//!
//! Run: cargo bench --features server --bench source_symbol_edit

use std::cell::RefCell;
use std::collections::BTreeMap;

use medulla_compute::ast::parser::{parse_file, SourceLanguage};
use medulla_compute::source_symbol_edit::{ResolvedSymbol, SymbolResolver};
use medulla_h_ingest::source_changeset::BlobIo;
use medulla_h_ingest::source_symbol_edit::prepare;
use medulla_source_types::lifecycle::sha256_hex;
use medulla_source_types::source_changeset::SubmittedBlobRef;
use medulla_source_types::source_symbol_edit::{SymbolEdit, SymbolEditKind, SymbolEditOp};

/// The spec budget: p99 at most 30 ms.
const BUDGET_P99_NS: u128 = 30_000_000;
/// The spec budget: peak memory growth at most 32 MiB.
const BUDGET_PEAK_MIB: f64 = 32.0;
const RUNS: usize = 200;
const FUNCTIONS: usize = 1_000;

#[derive(Default)]
struct Mem(RefCell<BTreeMap<String, Vec<u8>>>);

impl BlobIo for Mem {
    fn read(&self, digest: &str) -> Result<Vec<u8>, String> {
        self.0.borrow().get(digest).cloned().ok_or_else(|| "absent".to_string())
    }
    fn store(&self, bytes: &[u8]) -> Result<SubmittedBlobRef, String> {
        let digest = sha256_hex(bytes);
        self.0.borrow_mut().insert(digest.clone(), bytes.to_vec());
        Ok(SubmittedBlobRef { digest, size: bytes.len() as u64 })
    }
}

struct One(String, ResolvedSymbol);

impl SymbolResolver for One {
    fn resolve(&self, _base_rev: &str, symbol_id: &str) -> Option<ResolvedSymbol> {
        (symbol_id == self.0).then(|| self.1.clone())
    }
}

/// 5,000 lines: 1,000 five-line functions.
fn source() -> String {
    let mut text = String::new();
    for n in 0..FUNCTIONS {
        text.push_str(&format!("def f{n}(x):\n    y = x + {n}\n    z = y * 2\n    return z\n\n"));
    }
    text
}

/// Verifies MEDULLA-SCM-R013.7: one symbol edit in a 5,000-line file stays under the p99 budget.
fn edit_one_symbol_in_5000_lines() {
    let source = source();
    assert_eq!(source.lines().count(), 5_000);
    let parsed = parse_file(&source, "big.py", SourceLanguage::Python);
    let target = parsed.symbols.iter().find(|s| s.name == "f500").expect("f500 parses");
    let (start, end) = (target.byte_start as usize, target.byte_end as usize);
    let blobs = Mem::default();
    let stored = blobs.store(source.as_bytes()).expect("seed");
    let edit = SymbolEdit {
        repo: "bench".into(),
        branch: "main".into(),
        base_rev: "c0".into(),
        ops: vec![SymbolEditOp {
            kind: SymbolEditKind::ReplaceSymbol,
            symbol_id: target.id.clone(),
            anchor_span_digest: sha256_hex(&source.as_bytes()[start..end]),
            new_text: "def f500(x):\n    return x".into(),
        }],
        message: "bench edit".into(),
        run_trace_id: String::new(),
    };
    let resolver = One(
        target.id.clone(),
        ResolvedSymbol {
            path: "big.py".into(),
            blob_digest: stored.digest,
            span: start..end,
            body_span: None,
            mode: 0o100644,
        },
    );
    let mut latencies_ns = Vec::with_capacity(RUNS);
    let memory_before = high_water_mib();
    for _ in 0..RUNS {
        let began = std::time::Instant::now();
        let prepared = prepare(&edit, "bench", 1, &resolver, &blobs).expect("the edit prepares");
        latencies_ns.push(began.elapsed().as_nanos());
        assert_eq!(prepared.request.paths.len(), 1);
    }
    let growth_mib = (high_water_mib() - memory_before).max(0.0);
    latencies_ns.sort_unstable();
    let p50 = latencies_ns[RUNS / 2];
    let p99 = latencies_ns[(RUNS as f64 * 0.99) as usize];
    println!("edit_one_symbol_in_5000_lines:");
    println!("  p50 = {p50} ns");
    println!("  p99 = {p99} ns");
    if p99 > BUDGET_P99_NS {
        println!("  FAIL (budget: p99 at most {BUDGET_P99_NS} ns)");
        std::process::exit(1);
    }
    if growth_mib > BUDGET_PEAK_MIB {
        println!("  FAIL (budget: peak growth at most {BUDGET_PEAK_MIB} MiB, saw {growth_mib} MiB)");
        std::process::exit(1);
    }
    println!("  PASS (budget: p99 at most {BUDGET_P99_NS} ns)");
}

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn main() {
    edit_one_symbol_in_5000_lines();
}
