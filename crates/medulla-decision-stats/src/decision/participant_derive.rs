//! Which perspectives a session derives (MEDULLA-DECISION-ENGINE-R651.10).
//!
//! A session holds members. Each speaker's turns are derived into conclusions from up to three
//! kinds of observer: the shared system view (always), the speaker itself (when its
//! `observe_self` flag is set) and every other member that observes the others (its
//! `observe_others` flag). The function here is pure: the same members and settings give the same
//! sorted, duplicate-free keys, and `enqueue_derivations` (R651.11) turns each key into one job.

use medulla_decision_types::claims::{ObserverRef, ParticipantId};
use medulla_work_types::memory_scope::ObservationConfig;

/// One derivation: whose turns (`subject`), seen by whom (`observer`), in which session. `scope_id`
/// names the memory scope the derivation runs under, empty when it runs over the whole view.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
}
