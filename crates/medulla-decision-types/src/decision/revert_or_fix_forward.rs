//! The `revert_or_fix_forward` question family (MEDULLA-DECISION-ENGINE-R827).
//!
//! `Decide` with
//! [`QuestionKind::RevertOrFixForward`](super::statistical::QuestionKind::RevertOrFixForward)
//! answers how to remove a culprit changeset the bisect (`BisectStep`,
//! MEDULLA-SCM-R009) named: `Revert` takes it out, `FixForward` lands a change
//! on top of it. The caller names the culprit, the changesets that depend on it
//! and the gate verdicts recorded for it. `Revert` is illegal when any
//! dependent exists, because reverting would take the dependents' base away;
//! the legal set is ranked, `Revert` first (it restores the last green state
//! without a new change), and an answer never names an illegal option. The
//! decision writes nothing.

use medulla_types::refusal_code::RefusalCode;
use medulla_work_types::step_verdict::VerificationVerdict;
use serde::{Deserialize, Serialize};

use super::record::outcome::DecisionRefusal;

/// Text parameter: the id of the culprit changeset.
pub const PARAM_CULPRIT: &str = "culprit";
/// Text parameter: a JSON array of the ids of the changesets that depend on the culprit.
pub const PARAM_DEPENDENTS: &str = "dependents";
/// Text parameter: a JSON array of [`RecordedVerdict`], the gate verdicts recorded for the culprit.
pub const PARAM_VERDICTS: &str = "verdicts";

/// The option that takes the culprit out.
pub const OPTION_REVERT: &str = "revert";
/// The option that lands a change on top of the culprit.
pub const OPTION_FIX_FORWARD: &str = "fix_forward";

/// `Revert` is the only option the question declares and dependents exist.
pub const REVERT_HAS_DEPENDENTS: RefusalCode = RefusalCode::new("REVERT_HAS_DEPENDENTS");

/// The refusal table of the family.
pub const REFUSALS: &[RefusalCode] = &[REVERT_HAS_DEPENDENTS];

/// One gate verdict recorded for the culprit changeset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedVerdict {
    /// The gate that judged the changeset.
    pub gate_id: String,
    /// What the gate answered.
    pub verdict: VerificationVerdict,
}

/// The options legal for a culprit with `dependents`, in rank order: `Revert`
/// first when it is legal, then `FixForward`, which is always legal.
pub fn legal_ranked(dependents: &[String]) -> Vec<&'static str> {
    let mut legal = Vec::with_capacity(2);
    if dependents.is_empty() {
        legal.push(OPTION_REVERT);
    }
    legal.push(OPTION_FIX_FORWARD);
    legal
}

/// A refused answer: the only option declared is `Revert` and the culprit has
/// `dependents`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevertRefusal {
    /// How many changesets depend on the culprit.
    pub dependents: usize,
}

impl DecisionRefusal for RevertRefusal {
    fn code(&self) -> RefusalCode {
        REVERT_HAS_DEPENDENTS
    }

    fn detail(&self) -> String {
        format!(
            "reverting the culprit would remove the base of {} dependent changeset(s)",
            self.dependents
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R827
    #[test]
    fn legality_follows_the_dependents() {
        assert_eq!(legal_ranked(&[]), [OPTION_REVERT, OPTION_FIX_FORWARD]);
        assert_eq!(legal_ranked(&["cs-2".to_string()]), [OPTION_FIX_FORWARD]);
    }
}
