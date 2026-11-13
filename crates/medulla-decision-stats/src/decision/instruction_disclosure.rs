//! The next instruction-disclosure event (MEDULLA-DECISION-ENGINE-R143.2).
//!
//! Pure. The caller submits the trigger kind; this versions and hashes it and
//! never derives a trigger from graph state. `initial` needs no prior event
//! and yields version 1. Any other trigger needs a prior event of the same set
//! and yields `prior.version + 1` with a hash that differs from the prior one.
//! `timeout` also needs `elapsed_s` at or above `disclosure.timeout`.

use medulla_decision_types::decision::instruction_disclosure::{
    instructions_hash, DisclosureEvent, DisclosureTrigger, DISCLOSURE_HASH_UNCHANGED,
    DISCLOSURE_PRIOR_MISSING, DISCLOSURE_PRIOR_UNEXPECTED, DISCLOSURE_SET_MISMATCH,
    DISCLOSURE_TIMEOUT_NOT_ELAPSED,
};
use medulla_decision_types::decision::record::outcome::RefusalCode;

/// Why no next event could be built: the registered code and a detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisclosureRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

fn refuse(code: RefusalCode, detail: &str) -> DisclosureRefusal {
    DisclosureRefusal {
        code,
        detail: detail.to_string(),
    }
}

/// What one disclosure asks: the set, its instruction text, the submitted
/// trigger and the clock.
#[derive(Debug, Clone, Copy)]
pub struct DisclosureInput<'a> {
    pub set_id: &'a str,
    pub instructions: &'a str,
    pub trigger: DisclosureTrigger,
    pub now_ms: u64,
    pub elapsed_s: u64,
    /// The resolved `disclosure.timeout` knob, in seconds.
    pub timeout_s: u64,
}

/// The event that follows `prior` (the latest event of the set, if any).
pub fn next_disclosure_event(
    input: &DisclosureInput<'_>,
    prior: Option<&DisclosureEvent>,
) -> Result<DisclosureEvent, DisclosureRefusal> {
    let hash = instructions_hash(input.instructions);
    let version = match (input.trigger, prior) {
        (DisclosureTrigger::Initial, Some(_)) => {
            return Err(refuse(
                DISCLOSURE_PRIOR_UNEXPECTED,
                "the initial disclosure has no prior event",
            ))
        }
        (DisclosureTrigger::Initial, None) => 1,
        (_, None) => {
            return Err(refuse(
                DISCLOSURE_PRIOR_MISSING,
                "a refinement needs the prior disclosure event",
            ))
        }
        (trigger, Some(prior)) => {
            if prior.set_id != input.set_id {
                return Err(refuse(
                    DISCLOSURE_SET_MISMATCH,
                    "the prior event belongs to another instruction set",
                ));
            }
            if trigger == DisclosureTrigger::Timeout && input.elapsed_s < input.timeout_s {
                return Err(refuse(
                    DISCLOSURE_TIMEOUT_NOT_ELAPSED,
                    "the timeout trigger needs disclosure.timeout seconds elapsed",
                ));
            }
            if prior.instructions_hash == hash {
                return Err(refuse(
                    DISCLOSURE_HASH_UNCHANGED,
                    "the new instructions hash equals the prior hash",
                ));
            }
            prior.version.saturating_add(1)
        }
    };
    Ok(DisclosureEvent {
        set_id: input.set_id.to_string(),
        version,
        trigger: input.trigger,
        at_ms: input.now_ms,
        instructions_hash: hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        trigger: DisclosureTrigger,
        text: &'static str,
        elapsed_s: u64,
    ) -> DisclosureInput<'static> {
        DisclosureInput {
            set_id: "set-a",
            instructions: text,
            trigger,
            now_ms: 5_000,
            elapsed_s,
            timeout_s: 300,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R143.2
    #[test]
    fn triggers_produce_distinct_versions() {
        let first =
            next_disclosure_event(&input(DisclosureTrigger::Initial, "v1", 0), None).unwrap();
        assert_eq!(first.version, 1);
        assert_eq!(first.trigger, DisclosureTrigger::Initial);
        assert_eq!(first.at_ms, 5_000);

        let second = next_disclosure_event(
            &input(DisclosureTrigger::RelevanceUpdate, "v2", 0),
            Some(&first),
        )
        .unwrap();
        let third = next_disclosure_event(
            &input(DisclosureTrigger::ConstraintChange, "v3", 0),
            Some(&second),
        )
        .unwrap();
        assert_eq!((second.version, third.version), (2, 3));
        assert_ne!(first.instructions_hash, second.instructions_hash);
        assert_ne!(second.instructions_hash, third.instructions_hash);
        assert_ne!(first.instructions_hash, third.instructions_hash);

        // The initial trigger with a prior event, and a refinement without one.
        assert_eq!(
            next_disclosure_event(&input(DisclosureTrigger::Initial, "v1", 0), Some(&first))
                .unwrap_err()
                .code,
            DISCLOSURE_PRIOR_UNEXPECTED
        );
        assert_eq!(
            next_disclosure_event(&input(DisclosureTrigger::RelevanceUpdate, "v2", 0), None)
                .unwrap_err()
                .code,
            DISCLOSURE_PRIOR_MISSING
        );
        // A prior event of another set.
        let mut other = first.clone();
        other.set_id = "set-b".to_string();
        assert_eq!(
            next_disclosure_event(
                &input(DisclosureTrigger::RelevanceUpdate, "v2", 0),
                Some(&other)
            )
            .unwrap_err()
            .code,
            DISCLOSURE_SET_MISMATCH
        );
        // The same text never makes a new version.
        assert_eq!(
            next_disclosure_event(
                &input(DisclosureTrigger::RelevanceUpdate, "v1", 0),
                Some(&first)
            )
            .unwrap_err()
            .code,
            DISCLOSURE_HASH_UNCHANGED
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R143.2
    #[test]
    fn timeout_gate_is_inclusive() {
        let first =
            next_disclosure_event(&input(DisclosureTrigger::Initial, "v1", 0), None).unwrap();
        let refused =
            next_disclosure_event(&input(DisclosureTrigger::Timeout, "v2", 299), Some(&first))
                .unwrap_err();
        assert_eq!(refused.code, DISCLOSURE_TIMEOUT_NOT_ELAPSED);
        let event =
            next_disclosure_event(&input(DisclosureTrigger::Timeout, "v2", 300), Some(&first))
                .unwrap();
        assert_eq!(
            (event.version, event.trigger),
            (2, DisclosureTrigger::Timeout)
        );
        // Deterministic: the same inputs give the same event.
        assert_eq!(
            event,
            next_disclosure_event(&input(DisclosureTrigger::Timeout, "v2", 300), Some(&first))
                .unwrap()
        );
    }
}
