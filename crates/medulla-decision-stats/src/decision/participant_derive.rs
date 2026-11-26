//! Which perspectives a session derives (MEDULLA-DECISION-ENGINE-R651.10).
//!
//! A session holds members. Each speaker's turns are derived into conclusions from up to three
//! kinds of observer: the shared system view (always), the speaker itself (when its
//! `observe_self` flag is set) and every other member that observes the others (its
//! `observe_others` flag). The function here is pure: the same members and settings give the same
//! sorted, duplicate-free keys, and `enqueue_derivations` (R651.11) turns each key into one job.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use medulla_decision_types::claims::{ObserverRef, ParticipantId};
use medulla_work_types::memory_scope::ObservationConfig;

/// One derivation: whose turns (`subject`), seen by whom (`observer`), in which session. `scope_id`
/// names the memory scope the derivation runs under, empty when it runs over the whole view.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeriveKey {
    pub scope_id: String,
    pub session_id: String,
    pub subject: ParticipantId,
    pub observer: ObserverRef,
}

/// The inputs of one session's derivation targets.
#[derive(Clone, Debug)]
pub struct DerivationInput<'a> {
    pub scope_id: &'a str,
    pub session_id: &'a str,
    pub members: &'a [ParticipantId],
    pub configs: &'a [ObservationConfig],
}

/// Which observers a flag pair adds, indexed by `2 * observe_self + observe_others`:
/// `(the participant observes its own turns, the participant observes every other member's)`.
const OBSERVER_SETS: [(bool, bool); 4] = [(false, false), (false, true), (true, false), (true, true)];

/// The setting of `member` in `session_id`: the last stored one, or the default (observes itself,
/// not the others).
fn flags_of(input: &DerivationInput<'_>, member: &ParticipantId) -> (bool, bool) {
    let setting = input
        .configs
        .iter()
        .rev()
        .find(|config| config.session_id == input.session_id && &config.participant_id == member);
    let (observe_self, observe_others) = setting.map_or((true, false), |config| (config.observe_self, config.observe_others));
    OBSERVER_SETS[usize::from(observe_self) * 2 + usize::from(observe_others)]
}

/// The derivations one session needs, sorted and without a repeated key. The system view is a
/// target for every member's turns; a member that observes itself is the observer of its own
/// turns; a member that observes the others is the observer of every other member's turns. A
/// setting for someone who is not a member changes nothing. Cost is O(m squared) for m members.
pub fn derivation_targets(input: &DerivationInput<'_>) -> Vec<DeriveKey> {
    let key = |subject: &ParticipantId, observer: ObserverRef| DeriveKey {
        scope_id: input.scope_id.to_string(),
        session_id: input.session_id.to_string(),
        subject: subject.clone(),
        observer,
    };
    let mut keys = Vec::new();
    for member in input.members {
        keys.push(key(member, ObserverRef::System));
    }
    for observer in input.members {
        let (own, others) = flags_of(input, observer);
        if own {
            keys.push(key(observer, ObserverRef::Participant(observer.clone())));
        }
        if others {
            for subject in input.members.iter().filter(|subject| *subject != observer) {
                keys.push(key(subject, ObserverRef::Participant(observer.clone())));
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

/// The pending derivation of one [`DeriveKey`]: the turns it covers and when it is due
/// (MEDULLA-DECISION-ENGINE-R651.11). It is stored as a `DerivePayload` graph node; the queue
/// task names it by `payload_ref`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivePayload {
    /// The perspective the derivation concludes for.
    pub key: DeriveKey,
    /// The first turn the range covers.
    pub first_turn_id: String,
    /// The last turn the range covers.
    pub last_turn_id: String,
    /// How many turns the range covers.
    pub turns: u32,
    /// The queue task that will run the range.
    pub task_id: String,
    /// Timestamp of the newest turn of the range.
    pub last_turn_ms: u64,
    /// When the range is due: the newest turn plus the debounce, or the newest turn itself once
    /// the range holds `batch_turns` turns.
    pub due_at_ms: u64,
}

/// The thresholds a range's due time follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueRule {
    /// Quiet time after the last turn before the range is due.
    pub debounce_ms: u64,
    /// Turns after which the range is due at once.
    pub batch_turns: u32,
}

/// When a range of `turns` turns ending at `last_turn_ms` is due.
pub fn due_at(last_turn_ms: u64, turns: u32, rule: DueRule) -> u64 {
    if turns >= rule.batch_turns {
        last_turn_ms
    } else {
        last_turn_ms.saturating_add(rule.debounce_ms)
    }
}

/// Extend a pending range by one newer turn; the due time follows the rule.
pub fn extend_payload(payload: &DerivePayload, turn_id: &str, turn_ms: u64, rule: DueRule) -> DerivePayload {
    let turns = payload.turns.saturating_add(1);
    let last_turn_ms = payload.last_turn_ms.max(turn_ms);
    DerivePayload {
        last_turn_id: turn_id.to_string(),
        turns,
        last_turn_ms,
        due_at_ms: due_at(last_turn_ms, turns, rule),
        ..payload.clone()
    }
}

/// The SHA-256 of a key and the first turn of its range: the queue's idempotency key and the
/// task id, so a retry after a crash names the same task. Every part is length-framed.
pub fn key_digest(key: &DeriveKey, first_turn_id: &str) -> String {
    let mut hasher = Sha256::new();
    for part in [
        key.scope_id.as_str(),
        key.session_id.as_str(),
        key.subject.as_str(),
        &key.observer.as_property(),
        first_turn_id,
    ] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The digest of a key alone: the id of the key's payload node (one node per key; its range is
/// rewritten when a new range starts).
pub fn key_node_digest(key: &DeriveKey) -> String {
    key_digest(key, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(slug: &str) -> ParticipantId {
        ParticipantId::new(slug).unwrap()
    }

    fn config(session: &str, slug: &str, observe_self: bool, observe_others: bool) -> ObservationConfig {
        ObservationConfig {
            session_id: session.into(),
            participant_id: id(slug),
            observe_self,
            observe_others,
        }
    }

    fn targets(members: &[ParticipantId], configs: &[ObservationConfig]) -> Vec<(String, String)> {
        derivation_targets(&DerivationInput { scope_id: "", session_id: "s1", members, configs })
            .into_iter()
            .map(|key| {
                let observer = match key.observer {
                    ObserverRef::System => "system".to_string(),
                    ObserverRef::Participant(id) => id.as_str().to_string(),
                };
                (key.subject.as_str().to_string(), observer)
            })
            .collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.10
    #[test]
    fn targets_follow_flags_and_never_repeat() {
        let members = [id("ada"), id("bo"), id("cy")];
        // No setting: the system view of every speaker, and each member observes itself.
        let defaults = targets(&members, &[]);
        assert_eq!(defaults.len(), 6);
        for member in ["ada", "bo", "cy"] {
            assert!(defaults.contains(&(member.into(), "system".into())));
            assert!(defaults.contains(&(member.into(), member.into())));
        }
        // observe_others adds the other members' turns for that observer only.
        let others = targets(&members, &[config("s1", "ada", true, true)]);
        assert_eq!(others.len(), 8);
        assert!(others.contains(&("bo".into(), "ada".into())));
        assert!(others.contains(&("cy".into(), "ada".into())));
        assert!(!others.contains(&("ada".into(), "bo".into())));
        // Turning observe_self off drops the member's own perspective; the system view stays.
        let blind = targets(&members, &[config("s1", "bo", false, false)]);
        assert_eq!(blind.len(), 5);
        assert!(!blind.contains(&("bo".into(), "bo".into())));
        assert!(blind.contains(&("bo".into(), "system".into())));
        // Observing only the others skips the participant's own turns.
        let only_others = targets(&members, &[config("s1", "cy", false, true)]);
        assert!(!only_others.contains(&("cy".into(), "cy".into())));
        assert!(only_others.contains(&("ada".into(), "cy".into())));
        // A setting of another session or of a non-member changes nothing; the last setting wins.
        let ignored = targets(
            &members,
            &[config("s2", "ada", true, true), config("s1", "zed", true, true), config("s1", "bo", true, true), config("s1", "bo", true, false)],
        );
        assert_eq!(ignored, defaults);
        // Sorted, no key twice, for every flag pair.
        for (own, others_flag) in [(false, false), (false, true), (true, false), (true, true)] {
            let all = derivation_targets(&DerivationInput {
                scope_id: "sc",
                session_id: "s1",
                members: &members,
                configs: &[config("s1", "ada", own, others_flag), config("s1", "bo", others_flag, own)],
            });
            let mut sorted = all.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(all, sorted);
        }
        // One member alone, or none, has only its own perspectives.
        assert_eq!(targets(&[id("ada")], &[]).len(), 2);
        assert!(targets(&[], &[]).is_empty());
    }

    const RULE: DueRule = DueRule { debounce_ms: 30_000, batch_turns: 3 };

    fn payload(turns: u32, last_turn_ms: u64) -> DerivePayload {
        DerivePayload {
            key: DeriveKey {
                scope_id: String::new(),
                session_id: "s1".into(),
                subject: id("ada"),
                observer: ObserverRef::System,
            },
            first_turn_id: "t0".into(),
            last_turn_id: "t0".into(),
            turns,
            task_id: "task".into(),
            last_turn_ms,
            due_at_ms: due_at(last_turn_ms, turns, RULE),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.11
    #[test]
    fn a_range_is_due_after_the_debounce_or_at_the_batch() {
        assert_eq!(due_at(1_000, 1, RULE), 31_000);
        assert_eq!(due_at(1_000, 2, RULE), 31_000);
        assert_eq!(due_at(1_000, 3, RULE), 1_000);
        assert_eq!(due_at(u64::MAX, 1, RULE), u64::MAX);
        let mut range = payload(1, 1_000);
        for (turn, ms) in [("t1", 2_000), ("t2", 3_000)] {
            range = extend_payload(&range, turn, ms, RULE);
        }
        assert_eq!(range.turns, 3);
        assert_eq!(range.first_turn_id, "t0");
        assert_eq!(range.last_turn_id, "t2");
        assert_eq!(range.due_at_ms, 3_000);
        // An older turn never moves the newest time backwards.
        assert_eq!(extend_payload(&range, "t3", 1, RULE).last_turn_ms, 3_000);
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.11
    #[test]
    fn the_key_digest_separates_keys_and_first_turns() {
        let base = payload(1, 1).key;
        let mut other = base.clone();
        other.observer = ObserverRef::Participant(id("ada"));
        assert_ne!(key_digest(&base, "t0"), key_digest(&other, "t0"));
        assert_ne!(key_digest(&base, "t0"), key_digest(&base, "t1"));
        assert_eq!(key_digest(&base, "t0"), key_digest(&base.clone(), "t0"));
        assert_eq!(key_node_digest(&base), key_digest(&base, ""));
        assert_eq!(key_digest(&base, "t0").len(), 64);
    }
}
