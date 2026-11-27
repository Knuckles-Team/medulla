//! Pure knowledge-flag kernels (MEDULLA-DECISION-ENGINE-R708.2).
//!
//! No I/O and no `MutationBatch`. [`raise`] decides what a `Raise` does to the flags of one
//! tenant: it creates a flag, deduplicates a repeat or returns a curated or dismissed flag to
//! `Open`. [`resolve`] moves a flag out of `Open`. Every state change goes through the one
//! machine [`KNOWLEDGE_FLAG_MACHINE`](medulla_decision_types::knowledge_flag::KNOWLEDGE_FLAG_MACHINE).
//! The handler (R708.3) turns the answer into one write.

use medulla_decision_types::knowledge_flag::{
    evidence_digest, flag_id_of, flag_key, target_form, FlagEvent, FlagResolution, FlagState,
    KnowledgeFlag, RaiseOutcome, FLAG_RUN_QUOTA_EXCEEDED, FLAG_STATE_ILLEGAL, FLAG_TARGET_INVALID,
    FLAG_TEXT_TOO_LARGE, SUGGESTED_CORRECTION_MAX_BYTES, WHAT_OBSERVED_MAX_BYTES,
};
use medulla_types::refusal_code::RefusalCode;

/// The knob `flag.max_open_per_run`, read once by the handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagKnobs {
    /// Most open flags one run may hold.
    pub max_open_per_run: u64,
}

/// What [`raise`] decided: the flag to store and what happened to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RaiseDecision {
    /// The flag after the raise.
    pub flag: KnowledgeFlag,
    /// Whether the flag is new, a repeat or returned to `Open`.
    pub outcome: RaiseOutcome,
}

/// Why a kernel refused: one registered code and a short reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlagRefusal {
    /// The registered refusal code.
    pub code: RefusalCode,
    /// What was wrong.
    pub reason: String,
}

fn refuse(code: RefusalCode, reason: impl Into<String>) -> FlagRefusal {
    FlagRefusal {
        code,
        reason: reason.into(),
    }
}

/// Bound the text of a flag.
pub fn check_text(flag: &KnowledgeFlag) -> Result<(), FlagRefusal> {
    if flag.what_observed.len() > WHAT_OBSERVED_MAX_BYTES {
        return Err(refuse(
            FLAG_TEXT_TOO_LARGE,
            format!("what_observed holds more than {WHAT_OBSERVED_MAX_BYTES} bytes"),
        ));
    }
    if flag
        .suggested_correction
        .as_ref()
        .is_some_and(|text| text.len() > SUGGESTED_CORRECTION_MAX_BYTES)
    {
        return Err(refuse(
            FLAG_TEXT_TOO_LARGE,
            format!("suggested_correction holds more than {SUGGESTED_CORRECTION_MAX_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// The flag `candidate` as a node-ready flag: its id is derived from its key.
pub fn with_derived_id(mut candidate: KnowledgeFlag) -> KnowledgeFlag {
    candidate.flag_id = flag_id_of(&flag_key(&candidate));
    candidate
}

/// Decide a `Raise` of `flag` against the `existing` flag of the same key, when there is one.
/// `run_open_count` is how many `Open` flags the flag's run already holds.
///
/// A new flag is `Created` with `raise_count` 1 and state `Open`. A repeat of an `Open` flag is
/// `Deduplicated`: one node, `raise_count` plus one. A repeat of a `Curated` flag applies
/// `Reraise` and is `Reopened` with `reopened = true`. A repeat of a `Dismissed` flag applies
/// `ReraiseChanged` only when the evidence digest differs, and is otherwise `Deduplicated`
/// and stays `Dismissed`. A raise that would add an `Open` flag to a run that already holds
/// `max_open_per_run` of them is refused and writes nothing.
pub fn raise(
    flag: &KnowledgeFlag,
    existing: Option<&KnowledgeFlag>,
    run_open_count: u64,
    knobs: &FlagKnobs,
) -> Result<RaiseDecision, FlagRefusal> {
    check_text(flag)?;
    if target_form(&flag.target_ref).is_none() {
        return Err(refuse(
            FLAG_TARGET_INVALID,
            "target_ref is none of skill:, term:, source: or topic:",
        ));
    }
    let quota = || {
        if run_open_count >= knobs.max_open_per_run {
            Err(refuse(
                FLAG_RUN_QUOTA_EXCEEDED,
                format!(
                    "run {} already holds {run_open_count} open flags (flag.max_open_per_run = {})",
                    flag.run_id, knobs.max_open_per_run
                ),
            ))
        } else {
            Ok(())
        }
    };
    let Some(existing) = existing else {
        quota()?;
        let mut created = with_derived_id(flag.clone());
        created.state = FlagState::Open;
        created.raise_count = 1;
        created.reopened = false;
        return Ok(RaiseDecision {
            flag: created,
            outcome: RaiseOutcome::Created,
        });
    };
    let mut next = existing.clone();
    next.raise_count = existing.raise_count.saturating_add(1);
    match existing.state {
        FlagState::Open => Ok(RaiseDecision {
            flag: next,
            outcome: RaiseOutcome::Deduplicated,
        }),
        FlagState::Curated => {
            quota()?;
            next.state = existing
                .state
                .apply(FlagEvent::Reraise)
                .map_err(|refused| refuse(FLAG_STATE_ILLEGAL, refused.to_string()))?;
            next.reopened = true;
            Ok(RaiseDecision {
                flag: next,
                outcome: RaiseOutcome::Reopened,
            })
        }
        FlagState::Dismissed => {
            let changed =
                evidence_digest(&flag.what_observed) != evidence_digest(&existing.what_observed);
            if !changed {
                return Ok(RaiseDecision {
                    flag: next,
                    outcome: RaiseOutcome::Deduplicated,
                });
            }
            quota()?;
            next.state = existing
                .state
                .apply(FlagEvent::ReraiseChanged)
                .map_err(|refused| refuse(FLAG_STATE_ILLEGAL, refused.to_string()))?;
            next.what_observed = flag.what_observed.clone();
            next.reopened = true;
            Ok(RaiseDecision {
                flag: next,
                outcome: RaiseOutcome::Reopened,
            })
        }
    }
}

/// Move `flag` out of `Open` by `resolution`. Any other state is refused with
/// `FLAG_STATE_ILLEGAL` and nothing changes.
pub fn resolve(
    flag: &KnowledgeFlag,
    resolution: FlagResolution,
) -> Result<KnowledgeFlag, FlagRefusal> {
    let event = match resolution {
        FlagResolution::Adopt => FlagEvent::Adopt,
        FlagResolution::Dismiss => FlagEvent::Dismiss,
    };
    let mut next = flag.clone();
    next.state = flag
        .state
        .apply(event)
        .map_err(|refused| refuse(FLAG_STATE_ILLEGAL, refused.to_string()))?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::knowledge_flag::{FlagKind, KnowledgeKind};

    const KNOBS: FlagKnobs = FlagKnobs {
        max_open_per_run: 20,
    };

    fn flag(observed: &str) -> KnowledgeFlag {
        KnowledgeFlag {
            flag_id: String::new(),
            tenant_id: "t1".into(),
            knowledge_kind: KnowledgeKind::Skill,
            flag_kind: FlagKind::Wrong,
            target_ref: "skill:triage@3".into(),
            what_observed: observed.into(),
            suggested_correction: Some("ask for the order id first".into()),
            raised_by: "agent-7".into(),
            run_id: "run-1".into(),
            trace_node_id: None,
            raised_at_ms: 1,
            state: FlagState::Open,
            raise_count: 0,
            reopened: false,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R708.2
    #[test]
    fn dedup_key_and_machine_are_total() {
        let key = flag_key(&flag("a"));
        assert_eq!(key.len(), 64);
        assert!(key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        // The key digests the suggestion when there is one, so a new observation keeps it.
        assert_eq!(key, flag_key(&flag("b")));
        let mut no_suggestion = flag("a");
        no_suggestion.suggested_correction = None;
        assert_ne!(key, flag_key(&no_suggestion));
        assert_eq!(flag_id_of(&key), format!("flag:{}", &key[..32]));

        // A new flag, then a repeat that keeps one node and adds one to the count.
        let created = raise(&flag("a"), None, 0, &KNOBS).expect("created");
        assert_eq!(created.outcome, RaiseOutcome::Created);
        assert_eq!(created.flag.raise_count, 1);
        assert_eq!(created.flag.flag_id, flag_id_of(&key));
        let repeat = raise(&flag("a"), Some(&created.flag), 1, &KNOBS).expect("dedup");
        assert_eq!(repeat.outcome, RaiseOutcome::Deduplicated);
        assert_eq!(repeat.flag.raise_count, 2);
        assert_eq!(repeat.flag.flag_id, created.flag.flag_id);

        // The 21st open flag of one run is refused; a repeat is not a new open flag.
        let over = raise(&flag("a"), None, 20, &KNOBS).expect_err("quota");
        assert_eq!(over.code, FLAG_RUN_QUOTA_EXCEEDED);
        assert!(raise(&flag("a"), Some(&created.flag), 20, &KNOBS).is_ok());

        // Text bounds and the target forms.
        let big = flag(&"x".repeat(WHAT_OBSERVED_MAX_BYTES + 1));
        assert_eq!(
            raise(&big, None, 0, &KNOBS).unwrap_err().code,
            FLAG_TEXT_TOO_LARGE
        );
        let mut bad = flag("a");
        bad.target_ref = "nowhere".into();
        assert_eq!(
            raise(&bad, None, 0, &KNOBS).unwrap_err().code,
            FLAG_TARGET_INVALID
        );

        // The machine is total: every state and event pair is either an edge or refused.
        let curated = resolve(&created.flag, FlagResolution::Adopt).expect("adopt");
        assert_eq!(curated.state, FlagState::Curated);
        assert_eq!(
            resolve(&curated, FlagResolution::Adopt).unwrap_err().code,
            FLAG_STATE_ILLEGAL
        );
        let reopened = raise(&flag("a"), Some(&curated), 0, &KNOBS).expect("reraise");
        assert_eq!(reopened.outcome, RaiseOutcome::Reopened);
        assert_eq!(reopened.flag.state, FlagState::Open);
        assert!(reopened.flag.reopened);

        // A dismissed flag re-raised with the same evidence stays dismissed; changed evidence
        // reopens it.
        let dismissed = resolve(&created.flag, FlagResolution::Dismiss).expect("dismiss");
        let same = raise(&flag("a"), Some(&dismissed), 0, &KNOBS).expect("same evidence");
        assert_eq!(same.flag.state, FlagState::Dismissed);
        assert_eq!(same.outcome, RaiseOutcome::Deduplicated);
        let changed = raise(&flag("c"), Some(&dismissed), 0, &KNOBS).expect("changed evidence");
        assert_eq!(changed.flag.state, FlagState::Open);
        assert_eq!(changed.flag.what_observed, "c");
        for state in FlagState::ALL {
            for event in [
                FlagEvent::Adopt,
                FlagEvent::Dismiss,
                FlagEvent::Reraise,
                FlagEvent::ReraiseChanged,
            ] {
                let legal = matches!(
                    (state, event),
                    (FlagState::Open, FlagEvent::Adopt)
                        | (FlagState::Open, FlagEvent::Dismiss)
                        | (FlagState::Curated, FlagEvent::Reraise)
                        | (FlagState::Dismissed, FlagEvent::ReraiseChanged)
                );
                assert_eq!(state.apply(event).is_ok(), legal, "{state:?} {event:?}");
            }
        }
    }
}
