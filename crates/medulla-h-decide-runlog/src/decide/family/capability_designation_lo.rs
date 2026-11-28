//! The executed-decision helper of the capability-designation family, split out so lower crates reach it.

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::StatisticalOutcome;
use {
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::FamilyExecuted,
};

pub fn executed(outcome: StatisticalOutcome, answered: Option<FamilyExecuted>) -> Executed {
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        family_premises: Vec::new(),
        route: None,
        family: answered,
    }
}
