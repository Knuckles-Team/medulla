//! MEDULLA-FEDERATED-QUERY-R736.1: the evidence forest model keeps support and verify cost
//! as separate bounded values and refuses with registered codes.

use medulla_result_types::evidence_forest::{
    check_budget, check_candidate_cap, check_seeds, EvidenceCandidate, ForestErrorCode,
    ForestSelection,
};

// spec: MEDULLA-FEDERATED-QUERY-R736.1
#[test]
fn scores_stay_separate_and_bounded() {
    let candidate = EvidenceCandidate::new("m1", 0.8, 0.3, 120).unwrap();
    assert_eq!(candidate.support, 0.8);
    assert_eq!(candidate.verify_cost, 0.3);
    let round_trip: EvidenceCandidate =
        serde_json::from_str(&serde_json::to_string(&candidate).unwrap()).unwrap();
    assert_eq!(round_trip, candidate);

    for (support, cost) in [
        (1.1, 0.0),
        (-0.1, 0.0),
        (0.5, 1.5),
        (f32::NAN, 0.0),
        (0.0, f32::INFINITY),
    ] {
        assert!(EvidenceCandidate::new("m", support, cost, 1).is_err());
    }
    // A bound value at either end passes.
    assert!(EvidenceCandidate::new("m", 0.0, 1.0, 1).is_ok());

    let selection = ForestSelection::new(
        vec!["a".into()],
        vec![("a".into(), "b".into()), ("b".into(), "c".into())],
        90,
        vec!["d".into()],
        100,
    )
    .unwrap();
    assert_eq!(selection.tokens_used, 90);

    let zero = ForestSelection::new(vec!["a".into()], vec![], 0, vec![], 0).unwrap_err();
    assert!(zero.starts_with("FOREST_BUDGET_ZERO"), "{zero}");
    for bad in [
        ForestSelection::new(vec![], vec![], 0, vec![], 10),
        ForestSelection::new(vec!["a".into()], vec![], 11, vec![], 10),
        ForestSelection::new(
            vec!["a".into()],
            vec![("x".into(), "y".into())],
            1,
            vec![],
            10,
        ),
    ] {
        assert!(bad.unwrap_err().starts_with("INVALID_ARGUMENT"));
    }

    assert_eq!(
        check_budget(0).unwrap_err().code,
        ForestErrorCode::BudgetZero
    );
    assert!(check_budget(1).is_ok());
    assert_eq!(check_seeds(0).unwrap_err().code, ForestErrorCode::NoSeeds);
    assert!(check_seeds(1).is_ok());
    assert_eq!(
        check_candidate_cap(3, 2).unwrap_err().code,
        ForestErrorCode::CandidateCap
    );
    assert!(check_candidate_cap(2, 2).is_ok());
    assert_eq!(
        ForestErrorCode::ALL
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>(),
        [
            "FOREST_BUDGET_ZERO",
            "FOREST_CANDIDATE_CAP",
            "FOREST_NO_SEEDS"
        ]
    );
}
