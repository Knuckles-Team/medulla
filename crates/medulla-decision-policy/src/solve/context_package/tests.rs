//! L2 proofs of the package kernel (MEDULLA-DECISION-ENGINE-R137.8).

use medulla_decision_types::decision::context_package::{
    ContextPackageBudget, ContextPackageCandidate, ContextPackageErrorCode, ContextPackageRequest,
};
use medulla_types::contract::BoundedVec;

use super::{effective_capacity, lower, select};

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn candidate(
    id: &str,
    unit: &str,
    resolution: u8,
    tokens: u64,
    value: u64,
) -> ContextPackageCandidate {
    ContextPackageCandidate {
        candidate_id: id.into(),
        evidence_unit: unit.into(),
        resolution,
        source_id: "source:a".into(),
        cited_start: 0,
        cited_end: 10,
        source_revision: 1,
        content_digest: DIGEST.into(),
        tokenizer_id: "tok".into(),
        tokenizer_version: "1".into(),
        token_weight: tokens,
        value_micro: value,
        measurement_provenance: "measured".into(),
    }
}

fn budget(window: u64) -> ContextPackageBudget {
    ContextPackageBudget {
        window_tokens: window,
        reserved_output_tokens: 0,
        caller_ceiling_tokens: None,
        money_ceiling_micro: None,
        price_micro_per_token: None,
        latency_ceiling_ms: None,
        tokens_per_second: None,
    }
}

fn request(
    budget: ContextPackageBudget,
    candidates: Vec<ContextPackageCandidate>,
) -> ContextPackageRequest {
    ContextPackageRequest {
        budget,
        candidates: BoundedVec::new(candidates).unwrap(),
        value_floor_micro_per_token: 0,
        node_budget: 100_000,
        remaining: None,
    }
}

/// Three evidence units at two or three resolutions each.
fn pool() -> Vec<ContextPackageCandidate> {
    vec![
        candidate("a0", "a", 0, 10, 100),
        candidate("a1", "a", 1, 40, 260),
        candidate("b0", "b", 0, 15, 90),
        candidate("b1", "b", 1, 50, 300),
        candidate("c0", "c", 0, 20, 120),
        candidate("c1", "c", 1, 30, 150),
        candidate("c2", "c", 2, 70, 420),
    ]
}

/// Exhaustive search: best (value desc, tokens asc) among selections with at
/// most one candidate per unit and total tokens within capacity.
fn oracle(request: &ContextPackageRequest, capacity: u64) -> (i128, u64) {
    let list: Vec<&ContextPackageCandidate> = request.candidates.iter().collect();
    let mut best: (i128, u64) = (0, 0);
    for mask in 0_u32..(1 << list.len()) {
        let picked: Vec<_> = (0..list.len())
            .filter(|i| mask >> i & 1 == 1)
            .map(|i| list[i])
            .collect();
        let mut units = std::collections::BTreeSet::new();
        if !picked.iter().all(|c| units.insert(c.evidence_unit.clone())) {
            continue;
        }
        let tokens: u64 = picked.iter().map(|c| c.token_weight).sum();
        if tokens > capacity {
            continue;
        }
        let value: i128 = picked.iter().map(|c| i128::from(c.value_micro)).sum();
        if value > best.0 || (value == best.0 && tokens < best.1) {
            best = (value, tokens);
        }
    }
    best
}

fn picked_totals(request: &ContextPackageRequest) -> (i128, u64, Vec<String>) {
    let solved = select(request).expect("verified optimum");
    let ids: Vec<String> = solved.selection.chosen_ids.iter().cloned().collect();
    let value = request
        .candidates
        .iter()
        .filter(|c| ids.contains(&c.candidate_id))
        .map(|c| i128::from(c.value_micro))
        .sum();
    (value, solved.selection.total_tokens, ids)
}

#[test]
fn matches_tiny_multiresolution_oracle() {
    for window in [0, 10, 25, 60, 100, 150, 500] {
        let req = request(budget(window), pool());
        let (value, tokens, _) = picked_totals(&req);
        assert_eq!((value, tokens), oracle(&req, window), "window {window}");
    }
}

#[test]
fn each_ceiling_can_be_the_tightest() {
    let base = || {
        let mut b = budget(1_000);
        b.reserved_output_tokens = 100; // window leaves 900
        b.caller_ceiling_tokens = Some(5_000);
        b.money_ceiling_micro = Some(1_000_000);
        b.price_micro_per_token = Some(1);
        b.latency_ceiling_ms = Some(1_000_000);
        b.tokens_per_second = Some(1_000);
        b
    };
    assert_eq!(effective_capacity(&base()).unwrap(), 900);
    let mut caller = base();
    caller.caller_ceiling_tokens = Some(300);
    assert_eq!(effective_capacity(&caller).unwrap(), 300);
    let mut output = base();
    output.reserved_output_tokens = 1_000_000;
    assert_eq!(effective_capacity(&output).unwrap(), 0);
    let mut money = base();
    money.money_ceiling_micro = Some(250);
    money.price_micro_per_token = Some(2);
    assert_eq!(effective_capacity(&money).unwrap(), 125);
    let mut latency = base();
    latency.latency_ceiling_ms = Some(50);
    latency.tokens_per_second = Some(2_000);
    assert_eq!(effective_capacity(&latency).unwrap(), 100);
    // A known zero price drops the money term; a known zero rate gives zero.
    let mut free = base();
    free.price_micro_per_token = Some(0);
    assert_eq!(effective_capacity(&free).unwrap(), 900);
    let mut stalled = base();
    stalled.tokens_per_second = Some(0);
    assert_eq!(effective_capacity(&stalled).unwrap(), 0);
    // A ceiling without its rate is refused, never read as zero.
    let mut no_price = base();
    no_price.price_micro_per_token = None;
    assert_eq!(
        effective_capacity(&no_price).unwrap_err().code,
        ContextPackageErrorCode::MissingFact
    );
    let mut no_rate = base();
    no_rate.tokens_per_second = None;
    assert_eq!(
        effective_capacity(&no_rate).unwrap_err().code,
        ContextPackageErrorCode::MissingFact
    );
    // The lowered model carries the capacity as its knapsack row.
    let (_, capacity) = lower(&request(money_tight(), pool())).unwrap();
    assert_eq!(capacity, 60);
    let solved = select(&request(money_tight(), pool())).unwrap();
    assert!(solved.selection.total_tokens <= 60);
    assert_eq!(solved.selection.effective_capacity_tokens, 60);
}

fn money_tight() -> ContextPackageBudget {
    let mut b = budget(10_000);
    b.money_ceiling_micro = Some(120);
    b.price_micro_per_token = Some(2);
    b
}

#[test]
fn order_does_not_change_the_optimum() {
    let forward = request(budget(100), pool());
    let mut reversed_pool = pool();
    reversed_pool.reverse();
    let reversed = request(budget(100), reversed_pool);
    let a = select(&forward).unwrap().selection;
    let b = select(&reversed).unwrap().selection;
    assert_eq!(a.chosen_ids, b.chosen_ids);
    assert_eq!(a.model_digest, b.model_digest);
    assert_eq!(a.certificate_digest, b.certificate_digest);
}
