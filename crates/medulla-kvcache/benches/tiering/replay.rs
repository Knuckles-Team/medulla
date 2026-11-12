//! Multi-turn trace replay over `TieredCache` for MEDULLA-DURABLE-KERNEL-R042.3.
//! Shared by the nightly bench `tiering` and the receipt-shape test.

use std::time::Instant;

use medulla_kvcache::{content_hash, Block, Tier, TieredCache};
use serde_json::{json, Value};

/// Hit rate over all tiers must be at least this.
pub const HIT_RATE_BUDGET: f64 = 0.90;
/// Cold page-in p99 for one 2 MiB block must be at most this many nanoseconds.
pub const COLD_PAGE_IN_P99_BUDGET_NS: u64 = 5_000_000;
/// Share of the working set the host tiers (hot plus warm) may hold.
pub const HOST_BUDGET_FRACTION: f64 = 0.25;
/// Tokens in the long-context check.
pub const LONG_CONTEXT_TOKENS: u64 = 10_000_000;
/// Tokens one block of the long-context check covers.
pub const TOKENS_PER_BLOCK: u64 = 16_384;

/// Replay parameters.
pub struct Trace {
    pub blocks: usize,
    pub block_bytes: usize,
    pub lookups: usize,
    /// Blocks of the long-context check (digest verification only).
    pub long_context_blocks: usize,
    pub long_context_block_bytes: usize,
}

/// What a replay measured.
#[derive(Debug)]
pub struct TieringReceipt {
    pub lookups: u64,
    pub hot_hits: u64,
    pub warm_hits: u64,
    pub cold_hits: u64,
    pub misses: u64,
    pub cold_page_in_p99_ns: u64,
    pub block_bytes: usize,
    pub long_context_tokens: u64,
    pub digest_mismatches: u64,
}

fn payload(index: usize, len: usize) -> Block {
    let mut state = (index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

impl TieringReceipt {
    fn rate(&self, hits: u64) -> f64 {
        if self.lookups == 0 {
            0.0
        } else {
            hits as f64 / self.lookups as f64
        }
    }

    /// Fraction of lookups served from any tier.
    pub fn hit_rate(&self) -> f64 {
        self.rate(self.hot_hits + self.warm_hits + self.cold_hits)
    }

    /// Every budget this receipt must meet; each miss is one line.
    pub fn breaches(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.hit_rate() < HIT_RATE_BUDGET {
            out.push(format!(
                "hit rate {:.3} < {HIT_RATE_BUDGET}",
                self.hit_rate()
            ));
        }
        if self.cold_page_in_p99_ns > COLD_PAGE_IN_P99_BUDGET_NS {
            out.push(format!(
                "cold page-in p99 {} ns > {COLD_PAGE_IN_P99_BUDGET_NS}",
                self.cold_page_in_p99_ns
            ));
        }
        if self.digest_mismatches != 0 {
            out.push(format!("{} digest mismatches", self.digest_mismatches));
        }
        out
    }

    /// The `BenchmarkClaim` receipt: configuration, measured result, pass flag and
    /// the per-tier hit rates.
    pub fn to_claim_json(&self, release: &str, case: &str) -> Value {
        let breaches = self.breaches();
        json!({
            "release": release,
            "case": case,
            "configuration": format!(
                "host budget {:.0}% of working set, {} byte blocks, {} lookups",
                HOST_BUDGET_FRACTION * 100.0, self.block_bytes, self.lookups
            ),
            "measured_result": format!(
                "hit rate {:.4}, cold page-in p99 {} ns, {} digest mismatches over {} tokens",
                self.hit_rate(), self.cold_page_in_p99_ns, self.digest_mismatches,
                self.long_context_tokens
            ),
            "passed": breaches.is_empty(),
            "breaches": breaches,
            "lookups": self.lookups,
            "hit_rate": self.hit_rate(),
            "tier_hit_rate": {
                "hot": self.rate(self.hot_hits),
                "warm": self.rate(self.warm_hits),
                "cold": self.rate(self.cold_hits),
                "miss": self.rate(self.misses),
            },
            "cold_page_in_p99_ns": self.cold_page_in_p99_ns,
            "long_context_tokens": self.long_context_tokens,
            "digest_mismatches": self.digest_mismatches,
        })
    }
}

/// Replay the trace: store every block, then look blocks up with a skewed
/// multi-turn pattern (most turns revisit a small recent set, some revisit old
/// context), recording the tier that served each lookup.
pub fn replay(trace: &Trace) -> TieringReceipt {
    let working_set = trace.blocks * trace.block_bytes;
    let host = (working_set as f64 * HOST_BUDGET_FRACTION) as usize;
    let mut cache: TieredCache<String, Block> =
        TieredCache::new(host * 3 / 5, host - host * 3 / 5, working_set * 2);
    let keys: Vec<String> = (0..trace.blocks).map(|i| format!("block-{i:08}")).collect();
    for (index, key) in keys.iter().enumerate() {
        cache.put(key.clone(), payload(index, trace.block_bytes));
    }

    let mut receipt = TieringReceipt {
        lookups: 0,
        hot_hits: 0,
        warm_hits: 0,
        cold_hits: 0,
        misses: 0,
        cold_page_in_p99_ns: 0,
        block_bytes: trace.block_bytes,
        long_context_tokens: 0,
        digest_mismatches: 0,
    };
    let mut cold_samples: Vec<u64> = Vec::new();
    let mut rng = 42u64;
    let recent = (trace.blocks / 5).max(1);
    for turn in 0..trace.lookups {
        let pick = lcg(&mut rng) as usize;
        let index = if pick % 10 < 8 {
            (turn / 4 + pick % recent) % trace.blocks
        } else {
            pick % trace.blocks
        };
        let tier = cache.tier_of(&keys[index]);
        let started = Instant::now();
        let served = cache.get(&keys[index]);
        let elapsed = started.elapsed().as_nanos() as u64;
        receipt.lookups += 1;
        match (served.is_some(), tier) {
            (true, Some(Tier::Hot)) => receipt.hot_hits += 1,
            (true, Some(Tier::Warm)) => receipt.warm_hits += 1,
            (true, _) => {
                receipt.cold_hits += 1;
                cold_samples.push(elapsed);
            }
            (false, _) => receipt.misses += 1,
        }
    }
    cold_samples.sort_unstable();
    if let Some(last) = cold_samples.len().checked_sub(1) {
        receipt.cold_page_in_p99_ns =
            cold_samples[((cold_samples.len() as f64 * 0.99) as usize).min(last)];
    }

    // Long context: every block is read back and its digest compared with the
    // digest taken when it was stored.
    let long_bytes = trace.long_context_block_bytes;
    let mut long: TieredCache<String, Block> = TieredCache::new(
        long_bytes * 4,
        long_bytes * 4,
        long_bytes * trace.long_context_blocks * 2,
    );
    let mut digests = Vec::with_capacity(trace.long_context_blocks);
    for index in 0..trace.long_context_blocks {
        let bytes = payload(index + 1_000_003, long_bytes);
        digests.push(content_hash(&bytes));
        long.put(format!("long-{index}"), bytes);
    }
    for (index, expected) in digests.iter().enumerate() {
        match long.get(&format!("long-{index}")) {
            Some(bytes) if &content_hash(&bytes) == expected => {}
            _ => receipt.digest_mismatches += 1,
        }
    }
    receipt.long_context_tokens = trace.long_context_blocks as u64 * TOKENS_PER_BLOCK;
    receipt
}
