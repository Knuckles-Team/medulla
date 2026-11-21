//! Step verification and adversarial admission (MEDULLA-DECISION-ENGINE-R753.751).
//!
//! [`verify_steps`] recomputes the SHA-256 of the step digests in order and
//! binds the verdict to it. The first failed step selects the recovery: `Retry`
//! while the attempts are below `knob verify.max_retries`, otherwise `Rollback`
//! to the last step that passed before it, or `Abandon` when none passed. When
//! no check failed, the first step whose check interval is wider than
//! `knob verify.max_interval_width` is `Uncertain`; otherwise the last step is
//! `Verified`. A failed or uncertain verdict withholds every later step.
//! [`admit`] takes between 1 and `knob verify.probe_budget` probes, allows no
//! `Compromised` verdict and at most `knob verify.max_inconclusive`
//! inconclusive ones. Pure functions of their inputs.

use medulla_decision_types::decision::digest::step_trace_digest;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::trajectory_verdict::{
    is_sha256_hex, AdmissionVerdict, ProbeResult, ProbeVerdict, RecoveryAction, StepCheck,
    StepOutcome, StepVerdict, TraceStep, VerifyKnobs, MAX_STEP_EVIDENCE,
};

/// A step verdict, the recovery a failed step selects and the ids of the
/// steps the verdict withholds.
pub type Verified = (StepVerdict, Option<RecoveryAction>, Vec<String>);

/// The outcome one check alone gives its step.
pub fn check_outcome(check: &StepCheck, knobs: &VerifyKnobs) -> StepOutcome {
    if !check.passed {
        StepOutcome::Failed
    } else if check
        .interval
        .is_some_and(|interval| interval.width_micros() > knobs.max_interval_width_micros)
    {
        StepOutcome::Uncertain
    } else {
        StepOutcome::Verified
    }
}

/// Verify a trace against one check per step, in step order.
pub fn verify_steps(
    steps: &[TraceStep],
    checks: &[StepCheck],
    attempts: u32,
    knobs: &VerifyKnobs,
) -> Result<Verified, String> {
    let invalid = |detail: String| StatisticalErrorCode::ParameterInvalid.refusal(detail);
    if steps.is_empty() {
        return Err(invalid("a trace needs at least one step".to_string()));
    }
    if steps.len() != checks.len() {
        return Err(invalid(
            "the trace needs exactly one check per step".to_string(),
        ));
    }
    for (index, (step, check)) in steps.iter().zip(checks).enumerate() {
        if step.step_id != check.step_id {
            return Err(invalid(format!(
                "check {index} does not name step {}",
                step.step_id
            )));
        }
        if steps[..index]
            .iter()
            .any(|seen| seen.step_id == step.step_id)
        {
            return Err(invalid(format!("step {} appears twice", step.step_id)));
        }
        if !is_sha256_hex(&step.digest) || !is_sha256_hex(&check.check_digest) {
            return Err(invalid(format!(
                "step {} carries a digest that is not 64 lowercase hex characters",
                step.step_id
            )));
        }
        if step.evidence_ids.len() > MAX_STEP_EVIDENCE
            || step.evidence_ids.iter().any(|id| id.trim().is_empty())
        {
            return Err(invalid(format!(
                "step {} cites at most {MAX_STEP_EVIDENCE} non-blank evidence ids",
                step.step_id
            )));
        }
        if let Some(interval) = &check.interval {
            interval.check().map_err(invalid)?;
        }
    }
    let digests: Vec<&str> = steps.iter().map(|s| s.digest.as_str()).collect();
    let trace_digest = step_trace_digest(&digests);
    let check_digests = checks.iter().map(|c| c.check_digest.clone()).collect();
    let withheld_after = |index: usize| -> Vec<String> {
        steps[index + 1..]
            .iter()
            .map(|step| step.step_id.clone())
            .collect()
    };
    let Some(failed) = checks.iter().position(|c| !c.passed) else {
        // No check failed: the first too-wide interval leaves its step uncertain.
        if let Some(wide) = checks
            .iter()
            .position(|c| check_outcome(c, knobs) == StepOutcome::Uncertain)
        {
            return Ok((
                StepVerdict::new(
                    steps[wide].step_id.clone(),
                    StepOutcome::Uncertain,
                    trace_digest,
                    check_digests,
                )
                .map_err(invalid)?,
                None,
                withheld_after(wide),
            ));
        }
        let last = steps[steps.len() - 1].step_id.clone();
        return Ok((
            StepVerdict::new(last, StepOutcome::Verified, trace_digest, check_digests)
                .map_err(invalid)?,
            None,
            Vec::new(),
        ));
    };
    let step_id = steps[failed].step_id.clone();
    let recovery = if attempts < knobs.max_retries {
        RecoveryAction::Retry {
            step_id: step_id.clone(),
        }
    } else {
        match checks[..failed].iter().rposition(|c| c.passed) {
            Some(passed) => RecoveryAction::Rollback {
                step_id: steps[passed].step_id.clone(),
            },
            None => RecoveryAction::Abandon,
        }
    };
    Ok((
        StepVerdict::new(step_id, StepOutcome::Failed, trace_digest, check_digests)
            .map_err(invalid)?,
        Some(recovery),
        withheld_after(failed),
    ))
}

/// Bind a verdict to the trace digest the caller names: the verification
/// carries no proof until the digest it was asked about is the digest of the
/// steps. An absent or malformed digest is `VERIFY_VERDICT_UNBOUND`; a digest
/// that differs from the recomputed one is `VERIFY_TRACE_MISMATCH`.
pub fn bind_verdict(verdict: &StepVerdict, claimed_trace_digest: &str) -> Result<(), String> {
    if !is_sha256_hex(claimed_trace_digest) {
        return Err(StatisticalErrorCode::VerifyVerdictUnbound.refusal(
            "a verification names the trace digest to bind to: 64 lowercase hex characters",
        ));
    }
    if claimed_trace_digest != verdict.trace_digest {
        return Err(StatisticalErrorCode::VerifyTraceMismatch.refusal(format!(
            "the named digest {claimed_trace_digest} differs from the recomputed {}",
            verdict.trace_digest
        )));
    }
    Ok(())
}

/// Admit a candidate from its probe results.
pub fn admit(probes: &[ProbeResult], knobs: &VerifyKnobs) -> Result<AdmissionVerdict, String> {
    if probes.is_empty() {
        return Err(
            StatisticalErrorCode::ProbeSetEmpty.refusal("an admission needs at least one probe")
        );
    }
    if probes.len() > knobs.probe_budget as usize {
        return Err(StatisticalErrorCode::ProbeBudgetExceeded.refusal(format!(
            "{} probes exceed knob verify.probe_budget {}",
            probes.len(),
            knobs.probe_budget
        )));
    }
    let inconclusive = probes
        .iter()
        .filter(|p| p.verdict == ProbeVerdict::Inconclusive)
        .count();
    let compromised = probes
        .iter()
        .any(|p| p.verdict == ProbeVerdict::Compromised);
    Ok(
        if compromised || inconclusive > knobs.max_inconclusive as usize {
            AdmissionVerdict::Reject
        } else {
            AdmissionVerdict::Admit
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }

    fn trace(passes: &[bool]) -> (Vec<TraceStep>, Vec<StepCheck>) {
        let steps = (0..passes.len())
            .map(|i| TraceStep {
                step_id: format!("s{i}"),
                digest: hex(i as u8 + 1),
                evidence_ids: Vec::new(),
            })
            .collect();
        let checks = passes
            .iter()
            .enumerate()
            .map(|(i, passed)| StepCheck {
                step_id: format!("s{i}"),
                passed: *passed,
                check_digest: hex(i as u8 + 0x80),
                interval: None,
            })
            .collect();
        (steps, checks)
    }

    fn knobs() -> VerifyKnobs {
        VerifyKnobs {
            max_retries: 2,
            probe_budget: 8,
            max_inconclusive: 0,
            max_interval_width_micros: 100_000,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R753.751
    #[test]
    fn recovery_follows_attempts() {
        let (steps, checks) = trace(&[true, true, false, true]);
        let recovery = |attempts| {
            verify_steps(&steps, &checks, attempts, &knobs())
                .unwrap()
                .1
                .unwrap()
        };
        // Below the retry budget: retry the failed step.
        for attempts in [0, 1] {
            assert_eq!(
                recovery(attempts),
                RecoveryAction::Retry {
                    step_id: "s2".into()
                }
            );
        }
        // At and past it: roll back to the last step that passed before it.
        for attempts in [2, 3] {
            assert_eq!(
                recovery(attempts),
                RecoveryAction::Rollback {
                    step_id: "s1".into()
                }
            );
        }
        // No step passed before the failure: abandon.
        let (steps, checks) = trace(&[false, true]);
        assert_eq!(
            verify_steps(&steps, &checks, 5, &knobs()).unwrap().1,
            Some(RecoveryAction::Abandon)
        );
        // The verdict names the first failed step and is bound to the trace.
        let (verdict, _, withheld) = verify_steps(&steps, &checks, 0, &knobs()).unwrap();
        assert_eq!(
            (verdict.step_id.as_str(), verdict.outcome),
            ("s0", StepOutcome::Failed)
        );
        assert_eq!(withheld, ["s1"]);
        assert_eq!(
            verdict.trace_digest,
            step_trace_digest(&[hex(1).as_str(), hex(2).as_str()])
        );
        assert_eq!(verdict.check_digests.as_slice(), [hex(0x80), hex(0x81)]);
        // A passing trace has no recovery and names its last step.
        let (steps, checks) = trace(&[true, true]);
        let (verdict, recovery, withheld) = verify_steps(&steps, &checks, 0, &knobs()).unwrap();
        assert!(verdict.outcome == StepOutcome::Verified && recovery.is_none());
        assert!(withheld.is_empty());
        assert_eq!(verdict.step_id, "s1");
        // A retry budget of zero rolls back at once.
        let none = VerifyKnobs {
            max_retries: 0,
            ..knobs()
        };
        let (steps, checks) = trace(&[true, false]);
        assert_eq!(
            verify_steps(&steps, &checks, 0, &none).unwrap().1,
            Some(RecoveryAction::Rollback {
                step_id: "s0".into()
            })
        );
        // The trace digest changes when any step digest does.
        let (mut other_steps, other_checks) = trace(&[true, false]);
        other_steps[0].digest = hex(0x55);
        assert_ne!(
            verify_steps(&steps, &checks, 0, &none)
                .unwrap()
                .0
                .trace_digest,
            verify_steps(&other_steps, &other_checks, 0, &none)
                .unwrap()
                .0
                .trace_digest
        );
        // Malformed traces are refused.
        let (steps, mut checks) = trace(&[true, true]);
        checks.pop();
        assert!(verify_steps(&steps, &checks, 0, &knobs()).is_err());
        assert!(verify_steps(&[], &[], 0, &knobs()).is_err());
        let (mut steps, checks) = trace(&[true]);
        steps[0].digest = "XYZ".into();
        assert!(verify_steps(&steps, &checks, 0, &knobs()).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R121.100
    #[test]
    fn a_wide_interval_is_uncertain_and_withholds_the_steps_after_it() {
        use medulla_decision_types::decision::trajectory_verdict::CheckInterval;
        let (steps, mut checks) = trace(&[true, true, true]);
        // A tight interval verifies the last step.
        checks[1].interval = Some(CheckInterval {
            low_micros: 450_000,
            high_micros: 550_000,
        });
        let (verdict, recovery, withheld) = verify_steps(&steps, &checks, 0, &knobs()).unwrap();
        assert_eq!(
            (verdict.step_id.as_str(), verdict.outcome),
            ("s2", StepOutcome::Verified)
        );
        assert!(recovery.is_none() && withheld.is_empty());
        // One micro wider is uncertain at step s1; s2 is withheld.
        checks[1].interval = Some(CheckInterval {
            low_micros: 450_000,
            high_micros: 550_001,
        });
        let (verdict, recovery, withheld) = verify_steps(&steps, &checks, 0, &knobs()).unwrap();
        assert_eq!(
            (verdict.step_id.as_str(), verdict.outcome),
            ("s1", StepOutcome::Uncertain)
        );
        assert!(recovery.is_none());
        assert_eq!(withheld, ["s2"]);
        // A failed step outranks an earlier uncertain one.
        checks[2].passed = false;
        let (verdict, recovery, withheld) = verify_steps(&steps, &checks, 0, &knobs()).unwrap();
        assert_eq!(verdict.outcome, StepOutcome::Failed);
        assert!(recovery.is_some() && withheld.is_empty());
        // An inverted or out-of-range interval, or a blank evidence id, is refused.
        checks[2].passed = true;
        checks[1].interval = Some(CheckInterval {
            low_micros: 9,
            high_micros: 1,
        });
        assert!(verify_steps(&steps, &checks, 0, &knobs()).is_err());
        checks[1].interval = Some(CheckInterval {
            low_micros: 0,
            high_micros: 1_000_001,
        });
        assert!(verify_steps(&steps, &checks, 0, &knobs()).is_err());
        checks[1].interval = None;
        let mut blank = steps.clone();
        blank[0].evidence_ids = vec!["  ".into()];
        assert!(verify_steps(&blank, &checks, 0, &knobs()).is_err());
    }

    fn probe(id: &str, verdict: ProbeVerdict) -> ProbeResult {
        ProbeResult {
            probe_id: id.into(),
            verdict,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R753.751
    #[test]
    fn admission_follows_the_probe_budget_and_verdicts() {
        let resisted = |n: usize| -> Vec<ProbeResult> {
            (0..n)
                .map(|i| probe(&format!("p{i}"), ProbeVerdict::Resisted))
                .collect()
        };
        assert_eq!(admit(&resisted(1), &knobs()), Ok(AdmissionVerdict::Admit));
        assert_eq!(admit(&resisted(8), &knobs()), Ok(AdmissionVerdict::Admit));
        assert!(admit(&resisted(9), &knobs())
            .unwrap_err()
            .starts_with("PROBE_BUDGET_EXCEEDED"));
        assert!(admit(&[], &knobs())
            .unwrap_err()
            .starts_with("PROBE_SET_EMPTY"));
        let mut compromised = resisted(3);
        compromised[1] = probe("bad", ProbeVerdict::Compromised);
        assert_eq!(admit(&compromised, &knobs()), Ok(AdmissionVerdict::Reject));
        let mut unsure = resisted(3);
        unsure[2] = probe("maybe", ProbeVerdict::Inconclusive);
        assert_eq!(admit(&unsure, &knobs()), Ok(AdmissionVerdict::Reject));
        let lenient = VerifyKnobs {
            max_inconclusive: 1,
            ..knobs()
        };
        assert_eq!(admit(&unsure, &lenient), Ok(AdmissionVerdict::Admit));
    }

    // spec: MEDULLA-DECISION-ENGINE-R753.752
    #[test]
    fn a_verdict_binds_to_the_recomputed_trace_digest() {
        let (steps, checks) = trace(&[true, true]);
        let (verdict, _, _) = verify_steps(&steps, &checks, 0, &knobs()).unwrap();
        assert_eq!(bind_verdict(&verdict, &verdict.trace_digest), Ok(()));
        for unbound in ["", "not-hex", &"AB".repeat(32)] {
            let error = bind_verdict(&verdict, unbound).unwrap_err();
            assert!(error.starts_with("VERIFY_VERDICT_UNBOUND"), "{error}");
        }
        let error = bind_verdict(&verdict, &hex(9)).unwrap_err();
        assert!(error.starts_with("VERIFY_TRACE_MISMATCH"), "{error}");
    }
}
