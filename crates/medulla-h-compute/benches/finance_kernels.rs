//! MEDULLA-FINANCE-PRIMITIVES-R901.6: nightly group `finance/aml_screen_100k`.
//! One sample loops 100k `banking::screen_transaction` calls (the served route
//! screens one transaction per call). Budgets in the optimized profile: loop p50
//! under 20 ms, p99 under 50 ms, peak resident growth under 8 MiB. With
//! `MEDULLA_BENCH_BASELINE=<p50_us>` a p50 more than 10 percent above the baseline
//! fails. JSON on stdout.
//! Run with `cargo bench -p medulla-h-compute --bench finance_kernels`.

use std::hint::black_box;
use std::time::Instant;

use medulla_finance::banking;
use medulla_finance::banking::AmlRule;

const CALLS: i64 = 100_000;
const SAMPLES: usize = 50;
const BUDGET_P50_US: u128 = 20_000;
const BUDGET_P99_US: u128 = 50_000;
const BUDGET_RSS_KIB: u64 = 8 * 1024;

/// Peak resident set (VmHWM) of this process in KiB; 0 where unavailable.
fn peak_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse().ok())
        })
        .unwrap_or(0)
}

fn main() {
    let rule = AmlRule {
        rule_id: "ctr".into(),
        version: 1,
        threshold_minor: 10_000,
    };
    let rss_before = peak_rss_kib();
    let mut micros = Vec::with_capacity(SAMPLES);
    let mut alerts = 0usize;
    for _ in 0..SAMPLES {
        let start = Instant::now();
        alerts = 0;
        for amount in 1..=CALLS {
            let screened =
                banking::screen_transaction(black_box(&rule), black_box(amount % 20_000 + 1));
            alerts += usize::from(black_box(screened).expect("valid input").is_some());
        }
        micros.push(start.elapsed().as_micros());
    }
    let rss_growth = peak_rss_kib().saturating_sub(rss_before);
    micros.sort_unstable();
    let (p50, p99) = (
        micros[SAMPLES / 2],
        micros[(SAMPLES * 99 / 100).min(SAMPLES - 1)],
    );
    let optimized = !cfg!(debug_assertions);
    println!(
        "{{\"bench\":\"finance/aml_screen_100k\",\"calls\":{CALLS},\"alerts\":{alerts},\
         \"p50_us\":{p50},\"p99_us\":{p99},\"peak_rss_growth_kib\":{rss_growth},\
         \"budget_p50_us\":{BUDGET_P50_US},\"budget_p99_us\":{BUDGET_P99_US},\
         \"budget_rss_kib\":{BUDGET_RSS_KIB},\"optimized\":{optimized}}}"
    );
    assert!(alerts > 0, "the loop must cross the alert bands");
    if optimized {
        assert!(
            p50 < BUDGET_P50_US,
            "aml_screen_100k p50 {p50} us over budget"
        );
        assert!(
            p99 < BUDGET_P99_US,
            "aml_screen_100k p99 {p99} us over budget"
        );
        assert!(
            rss_growth < BUDGET_RSS_KIB,
            "aml_screen_100k grew {rss_growth} KiB"
        );
        if let Some(baseline) = std::env::var("MEDULLA_BENCH_BASELINE")
            .ok()
            .and_then(|value| value.trim().parse::<u128>().ok())
        {
            assert!(
                p50 * 10 <= baseline * 11,
                "aml_screen_100k regressed over 10 percent: {p50} us vs baseline {baseline} us"
            );
        }
    }
}
