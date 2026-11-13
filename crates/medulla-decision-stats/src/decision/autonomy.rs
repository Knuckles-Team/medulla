//! Trust ledger and corrigibility kernel (MEDULLA-DECISION-ENGINE-R752.755).
//!
//! [`autonomy_verdict`] allows a reversible action class once its ledger holds
//! `knob autonomy.clean_outcomes` consecutive independent successes. An
//! irreversible class, and a question of safety class `Security`, `Policy`,
//! `WriteBack` or `Irreversible`, always requires confirmation with the code
//! `ACTION_NEEDS_APPROVAL`. No float, no clock, no store.

use medulla_decision_types::decision::autonomy::{
    AutonomyReading, CorrigibilityVerdict, IrreversibilityClass, TrustLedger, ACTION_NEEDS_APPROVAL,
};
use medulla_decision_types::decision::statistical::QuestionSafety;

/// The ledger of `action_class` from its independent `outcomes`, newest first
/// (`true` is a success): the successes since the last failure.
pub fn ledger_from_outcomes(action_class: &str, outcomes: &[bool]) -> TrustLedger {
    TrustLedger {
        action_class: action_class.to_string(),
        clean_outcomes: u32::try_from(outcomes.iter().take_while(|ok| **ok).count())
            .unwrap_or(u32::MAX),
        last_failure_evaluation: None,
    }
}

/// Whether the rule, not the ledger, requires confirmation: an irreversible
/// class or a question of any safety class but `Ordinary`.
pub fn rule_requires_approval(class: IrreversibilityClass, safety: QuestionSafety) -> bool {
    class == IrreversibilityClass::Irreversible || safety != QuestionSafety::Ordinary
}

/// The corrigibility verdict of one action.
pub fn autonomy_verdict(
    class: IrreversibilityClass,
    safety: QuestionSafety,
    ledger: &TrustLedger,
    clean_needed: u32,
) -> AutonomyReading {
    if rule_requires_approval(class, safety) {
        return AutonomyReading {
            verdict: CorrigibilityVerdict::RequireConfirmation,
            clean_outcomes: ledger.clean_outcomes,
            code: Some(ACTION_NEEDS_APPROVAL.as_str().to_string()),
        };
    }
    let verdict = if ledger.clean_outcomes >= clean_needed {
        CorrigibilityVerdict::Allow
    } else {
        CorrigibilityVerdict::RequireConfirmation
    };
    AutonomyReading {
        verdict,
        clean_outcomes: ledger.clean_outcomes,
        code: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R752.755
    #[test]
    fn clean_run_clears_a_reversible_class_and_a_failure_resets_it() {
        let clean = ledger_from_outcomes("file.write", &[true; 20]);
        let allowed = autonomy_verdict(
            IrreversibilityClass::Reversible,
            QuestionSafety::Ordinary,
            &clean,
            20,
        );
        assert_eq!(allowed.verdict, CorrigibilityVerdict::Allow);
        assert_eq!(allowed.code, None);
        // The newest outcome failed: the count is 0.
        let mut outcomes = vec![false];
        outcomes.extend([true; 30]);
        let reset = ledger_from_outcomes("file.write", &outcomes);
        assert_eq!(reset.clean_outcomes, 0);
        let held = autonomy_verdict(
            IrreversibilityClass::Reversible,
            QuestionSafety::Ordinary,
            &reset,
            20,
        );
        assert_eq!(held.verdict, CorrigibilityVerdict::RequireConfirmation);
        assert_eq!(held.code, None);
    }

    // spec: MEDULLA-DECISION-ENGINE-R752.755
    #[test]
    fn the_rule_overrides_a_clean_ledger() {
        let clean = ledger_from_outcomes("db.drop", &[true; 1000]);
        for (class, safety) in [
            (IrreversibilityClass::Irreversible, QuestionSafety::Ordinary),
            (IrreversibilityClass::Reversible, QuestionSafety::Security),
            (IrreversibilityClass::Reversible, QuestionSafety::Policy),
            (IrreversibilityClass::Reversible, QuestionSafety::WriteBack),
        ] {
            let reading = autonomy_verdict(class, safety, &clean, 5);
            assert_eq!(reading.verdict, CorrigibilityVerdict::RequireConfirmation);
            assert_eq!(reading.code.as_deref(), Some("ACTION_NEEDS_APPROVAL"));
        }
    }
}
