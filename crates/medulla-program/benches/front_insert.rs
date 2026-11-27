//! MEDULLA-DECISION-ENGINE-R800.6: the `decide/program_front` group inserts
//! 100000 candidates into a Pareto front kept at up to 256 candidates, as a
//! reflective job does per evaluated candidate. JSON on stdout. Budget: p99
//! insert (insert plus the bound) at most 50 microseconds in the optimized
//! profile, and resident growth at most 64 MiB. A benchmark never gates a PR;
//! the nightly budget run reads this line.
//! Run with `cargo bench -p medulla-program --bench front_insert`.

use std::hint::black_box;
use std::time::Instant;

use medulla_program::optimize::{ParetoFront, PolicyCandidate};

const CANDIDATES: usize = 100_000;
const FRONT_SIZE: usize = 256;
const BUDGET_P99_US: u128 = 50;
const BUDGET_PEAK_MIB: f64 = 64.0;

/// One SplitMix64 step: the deterministic stream the candidates are drawn from.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

/// Candidate `index`: accuracy and benchmark trade off along an arc with noise,
/// so a large share of them stay non-dominated and the front fills to its bound.
fn candidate(state: &mut u64, index: usize) -> PolicyCandidate<2> {
    let unit = |state: &mut u64| (splitmix64(state) >> 40) as f32 / (1u64 << 24) as f32;
    let along = unit(state);
    let noise = unit(state) * 0.05;
    PolicyCandidate {
        id: format!("candidate-{index:06}"),
        parent: None,
        genes_digest: format!("{:064x}", index),
        scores: [along, (1.0 - along - noise).clamp(0.0, 1.0)],
        cost_micro_usd: splitmix64(state) % 10_000,
    }
}

fn main() {
    let mut state = 0xD1B5_4A32_D192_ED03u64;
    let candidates: Vec<PolicyCandidate<2>> = (0..CANDIDATES)
        .map(|index| candidate(&mut state, index))
        .collect();
    let before = high_water_mib();
    let mut front = ParetoFront::<2>::default();
    let mut micros = Vec::with_capacity(CANDIDATES);
    for candidate in candidates {
        let start = Instant::now();
        let changed = black_box(
            front
                .insert(black_box(candidate))
                .expect("a valid candidate"),
        );
        if changed {
            front.bound(FRONT_SIZE);
        }
        micros.push(start.elapsed().as_micros());
    }
    micros.sort_unstable();
    let (p50, p99) = (micros[CANDIDATES / 2], micros[CANDIDATES * 99 / 100]);
    let peak = (high_water_mib() - before).max(0.0);
    println!(
        "{{\"bench\":\"decide/program_front\",\"candidates\":{CANDIDATES},\
         \"front\":{},\"p50_us\":{p50},\"p99_us\":{p99},\"budget_p99_us\":{BUDGET_P99_US},\
         \"peak_mib\":{peak:.1},\"budget_peak_mib\":{BUDGET_PEAK_MIB},\"optimized\":{}}}",
        front.candidates().len(),
        !cfg!(debug_assertions)
    );
    assert!(front.candidates().len() <= FRONT_SIZE);
    if !cfg!(debug_assertions) {
        assert!(p99 <= BUDGET_P99_US, "front insert exceeded its budget");
        assert!(
            peak <= BUDGET_PEAK_MIB,
            "front insert exceeded its memory budget"
        );
    }
}
