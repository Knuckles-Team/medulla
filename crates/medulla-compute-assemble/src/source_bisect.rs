//! Binary search over the gate verdicts of a chain (MEDULLA-SCM-R009).
//!
//! Pure: the chain and the verdicts are inputs, the answer is the next changeset that needs a
//! verdict or the culprit. Verdicts already recorded inside the range narrow it first, so a
//! repeated call after `RecordGateVerdict` always makes progress.

use std::collections::BTreeMap;

use medulla_source_types::source_bisect::{
    BisectLink, BisectOutcome, BisectRefusal, BisectStepReceipt,
};
use medulla_work_types::step_verdict::VerificationVerdict;

/// The next step of a bisect.
///
/// `chain` is oldest first and ends at the failing changeset (`bad`, the last link).
/// `verdicts` maps a changeset id to its recorded verdict of the bisected gate. `good` is the
/// index of the known-good link, or `None` to use the last passing link below `bad`.
///
/// Refusals: `BISECT_VERDICT_COVERAGE` when no link below `bad` passes (or `good` does not
/// pass), `INVALID_ARGUMENT` for an empty chain or a `bad` that does not fail.
pub fn next_step(
    chain: &[BisectLink],
    verdicts: &BTreeMap<String, VerificationVerdict>,
    good: Option<usize>,
) -> Result<BisectStepReceipt, BisectRefusal> {
    let Some(bad_index) = chain.len().checked_sub(1) else {
        return Err(BisectRefusal::InvalidArgument(
            "the chain is empty".to_string(),
        ));
    };
    let verdict_at = |index: usize| verdicts.get(&chain[index].changeset).copied();
    if verdict_at(bad_index) == Some(VerificationVerdict::Pass) {
        return Err(BisectRefusal::InvalidArgument(
            "bad has a passing verdict".to_string(),
        ));
    }
    let coverage = || {
        BisectRefusal::VerdictCoverage("no changeset below bad has a passing verdict".to_string())
    };
    let mut low = match good {
        Some(index)
            if index < bad_index && verdict_at(index) == Some(VerificationVerdict::Pass) =>
        {
            index
        }
        Some(_) => return Err(coverage()),
        None => (0..bad_index)
            .rev()
            .find(|index| verdict_at(*index) == Some(VerificationVerdict::Pass))
            .ok_or_else(coverage)?,
    };
    let mut high = bad_index;
    // Recorded verdicts inside the range narrow it: the last pass raises the floor, then the
    // first failure above that floor lowers the ceiling.
    if let Some(passing) = (low + 1..high)
        .rev()
        .find(|index| verdict_at(*index) == Some(VerificationVerdict::Pass))
    {
        low = passing;
    }
    if let Some(failing) =
        (low + 1..high).find(|index| verdict_at(*index) == Some(VerificationVerdict::Fail))
    {
        high = failing;
    }
    if high - low == 1 {
        return Ok(BisectStepReceipt {
            outcome: BisectOutcome::Culprit,
            link: chain[high].clone(),
            remaining: 0,
        });
    }
    Ok(BisectStepReceipt {
        outcome: BisectOutcome::NeedsVerdict,
        link: chain[low + (high - low) / 2].clone(),
        remaining: (high - low - 1) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(count: usize) -> Vec<BisectLink> {
        (0..count)
            .map(|n| BisectLink {
                commit: format!("c{n}"),
                changeset: format!("s{n}"),
            })
            .collect()
    }

    fn verdicts(pairs: &[(usize, VerificationVerdict)]) -> BTreeMap<String, VerificationVerdict> {
        pairs.iter().map(|(n, v)| (format!("s{n}"), *v)).collect()
    }

    #[test]
    fn bisect_converges_on_the_first_failing_changeset() {
        use VerificationVerdict::{Fail, Pass};
        let links = chain(9);
        let first_bad = 5;
        let mut known = verdicts(&[(0, Pass), (8, Fail)]);
        for _ in 0..8 {
            let answer = next_step(&links, &known, None).expect("step");
            match answer.outcome {
                BisectOutcome::Culprit => {
                    assert_eq!(answer.link.commit, format!("c{first_bad}"));
                    return;
                }
                BisectOutcome::NeedsVerdict => {
                    let n: usize = answer.link.commit[1..].parse().expect("index");
                    known.insert(
                        answer.link.changeset,
                        if n >= first_bad { Fail } else { Pass },
                    );
                }
            }
        }
        panic!("bisect did not converge");
    }

    #[test]
    fn no_passing_verdict_refuses_coverage() {
        let links = chain(4);
        let refusal = next_step(&links, &verdicts(&[(3, VerificationVerdict::Fail)]), None)
            .expect_err("coverage");
        assert_eq!(refusal.code(), "BISECT_VERDICT_COVERAGE");
    }

    #[test]
    fn adjacent_bounds_name_the_culprit() {
        use VerificationVerdict::{Fail, Pass};
        let links = chain(2);
        let answer = next_step(&links, &verdicts(&[(0, Pass), (1, Fail)]), Some(0)).expect("step");
        assert_eq!(answer.outcome, BisectOutcome::Culprit);
        assert_eq!(answer.link, links[1]);
    }
}
