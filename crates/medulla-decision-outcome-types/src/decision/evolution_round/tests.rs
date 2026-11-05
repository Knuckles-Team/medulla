use super::*;

fn prediction() -> PredictionDeclaration {
    PredictionDeclaration {
        round: 3,
        fixes: (0..10).map(|i| format!("t{i}")).collect(),
        regressions: vec![],
    }
}

/// `up` tasks flip fail to pass as declared, `down` flip pass to fail.
fn outcomes(up: usize, down: usize) -> Vec<RoundTaskOutcome> {
    (0..10)
        .map(|i| {
            let task_id = format!("t{i}");
            if i < up {
                RoundTaskOutcome {
                    task_id,
                    passed_before: false,
                    passed_after: true,
                }
            } else if i < up + down {
                RoundTaskOutcome {
                    task_id,
                    passed_before: true,
                    passed_after: false,
                }
            } else {
                RoundTaskOutcome {
                    task_id,
                    passed_before: false,
                    passed_after: false,
                }
            }
        })
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R136.1
#[test]
fn round_types_round_trip() {
    let defaults = EvolutionDefaults::default();
    assert_eq!(defaults.rounds, 1);
    assert_eq!(
        (
            defaults.flip_fraction.numerator(),
            defaults.flip_fraction.denominator()
        ),
        (1, 2)
    );
    let allocation = StageAllocation {
        stage: "propose".into(),
        model_ref: "model:large@3".into(),
        impact: 12,
    };
    let budget = RunBudget {
        max_cost: 5_000,
        max_stronger_stages: 1,
    };
    let value = serde_json::to_value((&prediction(), &defaults, &allocation, &budget)).unwrap();
    assert_eq!(
        serde_json::from_value::<(
            PredictionDeclaration,
            EvolutionDefaults,
            StageAllocation,
            RunBudget
        )>(value)
        .unwrap(),
        (prediction(), defaults, allocation, budget)
    );
    assert_eq!(
        serde_json::to_value(RoundVerdict::Unverified).unwrap(),
        "unverified"
    );
    assert!(serde_json::from_value::<RoundVerdict>(serde_json::json!("refuted")).is_err());
    use medulla_decision_types::decision::statistical::QuestionKind;
    assert_eq!(
        serde_json::to_value([QuestionKind::RoundVerdict, QuestionKind::StageAllocation]).unwrap(),
        serde_json::json!(["round_verdict", "stage_allocation"])
    );
}

// spec: MEDULLA-DECISION-ENGINE-R136.2
#[test]
fn verdict_follows_flip_fraction() {
    let defaults = EvolutionDefaults::default();
    let verdict = |up, down| decide_round_verdict(&prediction(), &outcomes(up, down), &defaults);
    assert_eq!(verdict(6, 4), RoundVerdict::Verified);
    assert_eq!(verdict(4, 6), RoundVerdict::Falsified);
    assert_eq!(verdict(5, 5), RoundVerdict::Unverified);
    assert_eq!(verdict(2, 2), RoundVerdict::Unverified);
    // A declared regression that does regress counts as flipping as declared.
    let regressions = PredictionDeclaration {
        round: 3,
        fixes: vec![],
        regressions: vec!["t0".into()],
    };
    assert_eq!(
        decide_round_verdict(&regressions, &outcomes(0, 1), &defaults),
        RoundVerdict::Verified
    );
    let empty = PredictionDeclaration {
        round: 3,
        fixes: vec![],
        regressions: vec![],
    };
    assert_eq!(
        decide_round_verdict(&empty, &outcomes(6, 0), &defaults),
        RoundVerdict::Unverified
    );
}

fn model(model_ref: &str, cost: u64, strength: u32, adequate: bool) -> ModelOption {
    ModelOption {
        model_ref: model_ref.into(),
        cost,
        strength,
        adequate,
    }
}

fn stage(id: &str, impact: Option<i64>) -> StageInput {
    StageInput {
        stage: id.into(),
        impact,
    }
}

#[test]
fn stronger_models_go_to_high_impact_stages() {
    let models = vec![
        model("base", 1, 1, true),
        model("strong", 5, 9, true),
        model("cheap-but-unfit", 0, 0, false),
    ];
    let stages = vec![
        stage("a", Some(2)),
        stage("b", Some(7)),
        stage("c", None),
        stage("d", Some(7)),
    ];
    // Base total 4; one upgrade costs 4 more: budget fits exactly one.
    let one = RunBudget {
        max_cost: 8,
        max_stronger_stages: 3,
    };
    let got = allocate_stage_models(&stages, &models, &one).unwrap();
    let refs: Vec<&str> = got.iter().map(|a| a.model_ref.as_str()).collect();
    assert_eq!(refs, ["base", "strong", "base", "base"]);
    assert_eq!(got[2].impact, 0);
    // The stronger-stage cap binds before cost.
    let capped = RunBudget {
        max_cost: 100,
        max_stronger_stages: 2,
    };
    let got = allocate_stage_models(&stages, &models, &capped).unwrap();
    let refs: Vec<&str> = got.iter().map(|a| a.model_ref.as_str()).collect();
    assert_eq!(refs, ["base", "strong", "base", "strong"]);
    // Refusals.
    let poor = RunBudget {
        max_cost: 3,
        max_stronger_stages: 1,
    };
    let refused = allocate_stage_models(&stages, &models, &poor).unwrap_err();
    assert_eq!(
        refused,
        AllocationRefusal::BudgetBelowMinimum { shortfall: 1 }
    );
    assert_eq!(refused.code(), "ALLOCATION_BUDGET_BELOW_MINIMUM");
    let none = allocate_stage_models(&stages, &[model("x", 1, 1, false)], &one).unwrap_err();
    assert_eq!(none, AllocationRefusal::NoAdequateModel { models: 1 });
    assert_eq!(none.code(), "ALLOCATION_NO_ADEQUATE_MODEL");
}
