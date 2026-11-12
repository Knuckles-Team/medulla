use medulla_enrich::semantic::{
    candidate_value, rank_candidates, rank_candidates_top, ConceptInput, INVALID_CONFIDENCE,
};
use medulla_enrich::Id;

fn input(concept: &str, impact: u32, confidence: u32, cost: u32) -> ConceptInput {
    ConceptInput {
        concept: Id::new(concept).unwrap(),
        impact_bp: impact,
        confidence_bp: confidence,
        cost_units: cost,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R614.2.2
#[test]
fn value_orders_and_breaks_ties_by_id() {
    assert_eq!(candidate_value(8_000, 5_000, 4), 10_000_000);
    assert_eq!(candidate_value(10, 10, 0), 100, "zero cost counts as one");
    assert_eq!(candidate_value(u32::MAX, u32::MAX, 1), u32::MAX, "clamped");

    let inputs = vec![
        input("concept:b", 100, 100, 1),
        input("concept:a", 100, 100, 1),
        input("concept:top", 9_000, 9_000, 1),
        input("concept:cheap", 100, 100, 0),
        input("concept:costly", 100, 100, 100),
    ];
    let ranked = rank_candidates(&inputs).unwrap();
    let order: Vec<&str> = ranked.iter().map(|c| c.concept.as_str()).collect();
    assert_eq!(
        order,
        [
            "concept:top",
            "concept:a",
            "concept:b",
            "concept:cheap",
            "concept:costly"
        ]
    );
    assert_eq!(ranked[0].value, 81_000_000);

    let top = rank_candidates_top(&inputs, 2).unwrap();
    assert_eq!(top.len(), 2);
    assert_eq!(top[1].concept.as_str(), "concept:a");

    assert_eq!(
        rank_candidates(&[input("concept:x", 1, 10_001, 1)])
            .unwrap_err()
            .code,
        INVALID_CONFIDENCE
    );
}

// spec: MEDULLA-DECISION-ENGINE-R614.2.2
#[test]
fn ranking_is_deterministic_over_shuffled_inputs() {
    let base: Vec<ConceptInput> = (0..12u32)
        .map(|n| {
            input(
                &format!("concept:{n:02}"),
                100 + (n % 3) * 50,
                5_000,
                1 + n % 2,
            )
        })
        .collect();
    let expected = rank_candidates(&base).unwrap();
    // Deterministic LCG shuffles: no extra dependency.
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..50 {
        let mut shuffled = base.clone();
        for i in (1..shuffled.len()).rev() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            shuffled.swap(i, (state >> 33) as usize % (i + 1));
        }
        assert_eq!(rank_candidates(&shuffled).unwrap(), expected);
    }
}
