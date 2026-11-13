//! Step verification and adversarial admission (MEDULLA-DECISION-ENGINE-R753.751).
//!
//! [`verify_steps`] recomputes the SHA-256 of the step digests in order and
//! binds the verdict to it. The first failed step selects the recovery: `Retry`
//! while the attempts are below `knob verify.max_retries`, otherwise `Rollback`
//! to the last step that passed before it, or `Abandon` when none passed.
//! [`admit`] takes between 1 and `knob verify.probe_budget` probes, allows no
//! `Compromised` verdict and at most `knob verify.max_inconclusive`
//! inconclusive ones. Pure functions of their inputs.

use medulla_decision_types::decision::digest::step_trace_digest;
use medulla_decision_types::decision::trajectory_verdict::{
    is_sha256_hex, AdmissionVerdict, ProbeResult, ProbeVerdict, RecoveryAction, StepCheck,
    StepVerdict, TraceStep, VerifyKnobs,
};

/// A step verdict with the recovery a failed step selects.
pub type Verified = (StepVerdict, Option<RecoveryAction>);

/// Verify a trace against one check per step, in step order.
pub fn verify_steps(
    steps: &[TraceStep],
    checks: &[StepCheck],
    attempts: u32,
    knobs: &VerifyKnobs,
) -> Result<Verified, String> {
    if steps.is_empty() {
        return Err("a trace needs at least one step".to_string());
    }
    if steps.len() != checks.len() {
        return Err("the trace needs exactly one check per step".to_string());
    }
    for (index, (step, check)) in steps.iter().zip(checks).enumerate() {
        if step.step_id != check.step_id {
            return Err(format!("check {index} does not name step {}", step.step_id));
        }
        if steps[..index]
            .iter()
            .any(|seen| seen.step_id == step.step_id)
        {
            return Err(format!("step {} appears twice", step.step_id));
        }
        if !is_sha256_hex(&step.digest) || !is_sha256_hex(&check.check_digest) {
            return Err(format!(
                "step {} carries a digest that is not 64 lowercase hex characters",
                step.step_id
            ));
        }
    }
    let digests: Vec<&str> = steps.iter().map(|s| s.digest.as_str()).collect();
    let trace_digest = step_trace_digest(&digests);
    let check_digests = checks.iter().map(|c| c.check_digest.clone()).collect();
    let failed = checks.iter().position(|c| !c.passed);
    let Some(failed) = failed else {
        let last = steps[steps.len() - 1].step_id.clone();
        return Ok((
            StepVerdict::new(last, true, trace_digest, check_digests)?,
            None,
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
        StepVerdict::new(step_id, false, trace_digest, check_digests)?,
        Some(recovery),
    ))
}

/// Admit a candidate from its probe results.
pub fn admit(probes: &[ProbeResult], knobs: &VerifyKnobs) -> Result<AdmissionVerdict, String> {
    if probes.is_empty() || probes.len() > knobs.probe_budget as usize {
        return Err(format!(
            "an admission takes between 1 and {} probes",
            knobs.probe_budget
        ));
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
            })
            .collect();
        let checks = passes
            .iter()
            .enumerate()
            .map(|(i, passed)| StepCheck {
                step_id: format!("s{i}"),
                passed: *passed,
                check_digest: hex(i as u8 + 0x80),
            })
            .collect();
        (steps, checks)
    }

    fn knobs() -> VerifyKnobs {
        VerifyKnobs {
            max_retries: 2,
            probe_budget: 8,
            max_inconclusive: 0,
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
        let (verdict, _) = verify_steps(&steps, &checks, 0, &knobs()).unwrap();
        assert_eq!((verdict.step_id.as_str(), verdict.passed), ("s0", false));
        assert_eq!(
            verdict.trace_digest,
            step_trace_digest(&[hex(1).as_str(), hex(2).as_str()])
        );
        assert_eq!(verdict.check_digests.as_slice(), [hex(0x80), hex(0x81)]);
        // A passing trace has no recovery and names its last step.
        let (steps, checks) = trace(&[true, true]);
        let (verdict, recovery) = verify_steps(&steps, &checks, 0, &knobs()).unwrap();
        assert!(verdict.passed && recovery.is_none());
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
        assert!(admit(&resisted(9), &knobs()).is_err());
        assert!(admit(&[], &knobs()).is_err());
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
}
