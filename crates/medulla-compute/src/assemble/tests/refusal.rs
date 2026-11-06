//! An assembly refusal folds under the `Assemble` origin with a registered code
//! (MEDULLA-DECISION-ENGINE-R010.100.2).

use crate::assemble::AssembleError;
use medulla_decision_types::decision::codes;
use medulla_decision_types::decision::record::outcome::{
    AbstainReason, RefusalOrigin, RefusalSurface,
};
use medulla_decision_types::decision::DecisionErrorCode;

/// Verifies MEDULLA-DECISION-ENGINE-R010.100.2: Fold existing per-feature refusal enums
// spec: MEDULLA-DECISION-ENGINE-R010.100.2
#[test]
fn assembly_refusals_fold_under_the_assemble_origin() {
    for code in DecisionErrorCode::ALL {
        let refusal = AssembleError::new(*code, "detail");
        let folded = AbstainReason::from_refusal(RefusalOrigin::ASSEMBLE, &refusal);
        assert_eq!(folded.code().as_str(), code.as_str());
        assert_eq!(
            codes::resolve(code.as_str()),
            Some(folded.code()),
            "{code} is registered"
        );
        assert!(matches!(
            folded,
            AbstainReason::Surface { surface: RefusalSurface::Assemble, ref detail, .. } if detail == "detail"
        ));
    }
}
