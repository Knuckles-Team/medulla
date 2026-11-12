//! Nightly receipt for MEDULLA-DURABLE-KERNEL-R042.3: replay a multi-turn trace over
//! `TieredCache` with a host budget of 25 percent of the working set. Budgets: hit
//! rate at least 0.90 over all tiers, cold page-in p99 at most 5 ms for a 2 MiB
//! block, zero digest mismatches over a 10 000 000-token context. Writes one
//! `BenchmarkClaim` receipt to `benches/receipts/<release>/tiering.json` and exits
//! non-zero on a missed budget. Runs in the nightly job and never gates a pull request.

#[path = "../../../benches/common/claim.rs"]
mod claim;
#[path = "tiering/replay.rs"]
mod replay;

use std::path::Path;

fn main() {
    let trace = replay::Trace {
        blocks: 128,
        block_bytes: 2 * 1024 * 1024,
        lookups: 2_000,
        long_context_blocks: (replay::LONG_CONTEXT_TOKENS / replay::TOKENS_PER_BLOCK + 1) as usize,
        long_context_block_bytes: 4 * 1024,
    };
    let receipt = replay::replay(&trace);
    let release = std::env::var("MEDULLA_BENCH_RELEASE").unwrap_or_else(|_| "dev".to_string());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benches/receipts");
    let path = claim::write_receipt(
        &root,
        &release,
        "tiering",
        &receipt.to_claim_json(&release, "tiering"),
    )
    .expect("write the tiering receipt");
    println!("receipt {}", path.display());
    println!("{receipt:#?}");
    let breaches = receipt.breaches();
    if !breaches.is_empty() {
        eprintln!("tiering budgets missed: {}", breaches.join("; "));
        std::process::exit(1);
    }
}
