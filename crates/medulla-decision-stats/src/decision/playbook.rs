//! Playbook triage kernel (MEDULLA-DECISION-ENGINE-R704.2).
//!
//! Pure and deterministic. A [`Correlator`] keeps one queue of events per
//! service and evicts events older than the window, so an insert is O(1)
//! amortized and a queue never outgrows the window. A service is flapping when
//! its window holds at least `flap_min_events` state-change events.
//! [`match_playbook`] finds the playbooks of a trigger kind in a hash map and,
//! for a ticket class, checks that the event's own classes entail the
//! playbook's class through the derivation records of
//! `medulla-decision-outcome-types`. Nothing here calls a connector or writes.
//!
//! A correlator is built per request from the caller's own events, so one
//! tenant's events are never grouped with another's.

use std::collections::{BTreeMap, HashMap, VecDeque};

use medulla_decision_outcome_types::decision::derivation::classification_chain;
use medulla_decision_types::decision::playbook::{
    EventCorrelation, Playbook, TriggerKind, MAX_CORRELATED_EVENTS,
};
use medulla_decision_types::decision::DerivationEdge;
use medulla_types::contract::BoundedVec;

/// The event kind that counts toward flapping.
pub const STATE_CHANGE: &str = "state_change";

/// One inbound event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybookEvent {
    pub event_id: String,
    pub service: String,
    /// The event kind; [`STATE_CHANGE`] counts toward flapping.
    pub kind: String,
    pub at_ms: u64,
}

/// Why an event was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorrelateError {
    /// The event is older than the newest event held for its service.
    OutOfOrder {
        service: String,
        at_ms: u64,
        newest_ms: u64,
    },
    /// More events in the window than one correlation may carry.
    TooManyEvents { service: String },
}

/// The registry knobs the correlator reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelatorLimits {
    /// `playbook.window`, in milliseconds.
    pub window_ms: u64,
    /// `playbook.flap_min_events`.
    pub flap_min_events: usize,
}

/// Events of every service inside the window.
#[derive(Debug, Clone)]
pub struct Correlator {
    limits: CorrelatorLimits,
    per_service: BTreeMap<String, VecDeque<PlaybookEvent>>,
}

impl Correlator {
    pub fn new(limits: CorrelatorLimits) -> Self {
        Self {
            limits,
            per_service: BTreeMap::new(),
        }
    }

    /// Add `event`, first evicting the service's events older than the window
    /// as of `event.at_ms`. Events of one service arrive in time order.
    pub fn push(&mut self, event: PlaybookEvent) -> Result<(), CorrelateError> {
        let queue = self.per_service.entry(event.service.clone()).or_default();
        if let Some(newest) = queue.back() {
            if event.at_ms < newest.at_ms {
                return Err(CorrelateError::OutOfOrder {
                    service: event.service,
                    at_ms: event.at_ms,
                    newest_ms: newest.at_ms,
                });
            }
        }
        while queue
            .front()
            .is_some_and(|oldest| oldest.at_ms.saturating_add(self.limits.window_ms) < event.at_ms)
        {
            queue.pop_front();
        }
        if queue.len() >= MAX_CORRELATED_EVENTS {
            return Err(CorrelateError::TooManyEvents {
                service: event.service,
            });
        }
        queue.push_back(event);
        Ok(())
    }

    /// Events of `service` currently inside the window, oldest first.
    pub fn events(&self, service: &str) -> impl Iterator<Item = &PlaybookEvent> {
        self.per_service.get(service).into_iter().flatten()
    }

    /// Whether `service` holds at least `flap_min_events` state changes.
    pub fn is_flapping(&self, service: &str) -> bool {
        self.events(service)
            .filter(|event| event.kind == STATE_CHANGE)
            .count()
            >= self.limits.flap_min_events
    }

    /// The services that are flapping, in name order.
    pub fn flapping_services(&self) -> Vec<&str> {
        self.per_service
            .keys()
            .filter(|service| self.is_flapping(service))
            .map(String::as_str)
            .collect()
    }

    /// The correlation of `service`: its event ids in arrival order.
    pub fn correlation(&self, service: &str) -> Option<EventCorrelation> {
        let ids: Vec<String> = self.events(service).map(|e| e.event_id.clone()).collect();
        if ids.is_empty() {
            return None;
        }
        Some(EventCorrelation {
            key: service.to_string(),
            window_ms: self.limits.window_ms,
            events: BoundedVec::new(ids).ok()?,
        })
    }
}

/// The playbooks of a tenant's library, by trigger attribute value.
#[derive(Debug, Clone, Default)]
pub struct PlaybookIndex {
    by_trigger: HashMap<&'static str, Vec<Playbook>>,
}

impl PlaybookIndex {
    pub fn new(playbooks: impl IntoIterator<Item = Playbook>) -> Self {
        let mut by_trigger: HashMap<&'static str, Vec<Playbook>> = HashMap::new();
        for playbook in playbooks {
            by_trigger
                .entry(playbook.trigger.attribute_value())
                .or_default()
                .push(playbook);
        }
        Self { by_trigger }
    }
}

/// A playbook the trigger selects, and why a ticket class entails it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybookMatch {
    pub playbook_id: String,
    pub revision: u32,
    /// The derivation from the event's class to the playbook's ticket class;
    /// empty for a trigger that names no class.
    pub chain: Vec<DerivationEdge>,
}

/// The playbooks `trigger` selects, in id order. A `ticket_class` playbook
/// matches only when one of the event's `classes` entails its class.
pub fn match_playbook(
    index: &PlaybookIndex,
    trigger: &TriggerKind,
    event_id: &str,
    classes: &[String],
) -> Vec<PlaybookMatch> {
    let Some(candidates) = index.by_trigger.get(trigger.attribute_value()) else {
        return Vec::new();
    };
    let mut matches: Vec<PlaybookMatch> = candidates
        .iter()
        .filter_map(|playbook| match (&playbook.trigger, trigger) {
            (TriggerKind::TicketClass { name: required }, TriggerKind::TicketClass { .. }) => {
                classification_chain(event_id, classes.iter().map(String::as_str), required)
                    .map(|chain| (playbook, chain))
            }
            (own, asked) if own == asked => Some((playbook, Vec::new())),
            _ => None,
        })
        .map(|(playbook, chain)| PlaybookMatch {
            playbook_id: playbook.id.clone(),
            revision: playbook.revision,
            chain,
        })
        .collect();
    matches.sort_by(|a, b| a.playbook_id.cmp(&b.playbook_id));
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_ontology::CAPABILITY_ROOT;
    use medulla_decision_types::decision::playbook::ProposedStep;
    use medulla_decision_types::decision::EvidenceClass;

    const LIMITS: CorrelatorLimits = CorrelatorLimits {
        window_ms: 10_000,
        flap_min_events: 3,
    };

    fn event(id: usize, service: &str, kind: &str, at_ms: u64) -> PlaybookEvent {
        PlaybookEvent {
            event_id: format!("e{id}"),
            service: service.to_string(),
            kind: kind.to_string(),
            at_ms,
        }
    }

    fn playbook(id: &str, trigger: TriggerKind) -> Playbook {
        Playbook {
            id: id.to_string(),
            revision: 1,
            trigger,
            steps: BoundedVec::new(vec![ProposedStep {
                step_id: "s1".into(),
                action_ref: "action:inspect".into(),
                writes: false,
            }])
            .unwrap(),
            evidence_class: EvidenceClass::Observation,
        }
    }

    /// `n` state changes, one second apart, alternating between two services.
    fn burst(n: usize) -> (Correlator, usize) {
        let mut correlator = Correlator::new(LIMITS);
        let mut longest = 0;
        for i in 0..n {
            let service = if i % 2 == 0 { "svc-a" } else { "svc-b" };
            correlator
                .push(event(i, service, STATE_CHANGE, i as u64 * 1_000))
                .unwrap();
            longest = longest.max(correlator.events(service).count());
        }
        (correlator, longest)
    }

    // spec: MEDULLA-DECISION-ENGINE-R704.2
    #[test]
    fn window_eviction_and_flap_detection() {
        for n in [10usize, 1_000, 100_000] {
            let (correlator, longest) = burst(n);
            // Each service sees one event every two seconds, so a ten second
            // window holds six of its events; a queue never outgrows it.
            let held = if n == 10 { 5 } else { 6 };
            assert_eq!(longest, held, "{n} events");
            assert_eq!(correlator.flapping_services(), vec!["svc-a", "svc-b"]);
            let kept = correlator.correlation("svc-a").unwrap();
            assert_eq!(kept.events.len(), held);
            assert_eq!(kept.window_ms, 10_000);
        }

        // Fewer state changes than the minimum, or other event kinds, do not flap.
        let mut correlator = Correlator::new(LIMITS);
        correlator.push(event(0, "svc", STATE_CHANGE, 0)).unwrap();
        correlator
            .push(event(1, "svc", STATE_CHANGE, 1_000))
            .unwrap();
        correlator.push(event(2, "svc", "log_line", 2_000)).unwrap();
        assert!(!correlator.is_flapping("svc"));
        correlator
            .push(event(3, "svc", STATE_CHANGE, 3_000))
            .unwrap();
        assert!(correlator.is_flapping("svc"));

        // Silence longer than the window evicts everything older than it.
        correlator
            .push(event(4, "svc", STATE_CHANGE, 20_000))
            .unwrap();
        assert!(!correlator.is_flapping("svc"));
        assert_eq!(correlator.events("svc").count(), 1);
        assert!(correlator.correlation("other").is_none());

        // An event is kept exactly at the window edge and dropped just past it.
        let mut edge = Correlator::new(LIMITS);
        edge.push(event(0, "svc", STATE_CHANGE, 0)).unwrap();
        edge.push(event(1, "svc", STATE_CHANGE, 10_000)).unwrap();
        assert_eq!(edge.events("svc").count(), 2);
        edge.push(event(2, "svc", STATE_CHANGE, 10_001)).unwrap();
        assert_eq!(edge.events("svc").count(), 2);

        // An older event than the newest held is refused, naming the service.
        assert_eq!(
            edge.push(event(3, "svc", STATE_CHANGE, 5)),
            Err(CorrelateError::OutOfOrder {
                service: "svc".into(),
                at_ms: 5,
                newest_ms: 10_001
            })
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R704.2
    #[test]
    fn match_playbook_selects_by_trigger_and_entailed_class() {
        let index = PlaybookIndex::new([
            playbook("playbook:b-flap", TriggerKind::ServiceFlapping),
            playbook("playbook:a-flap", TriggerKind::ServiceFlapping),
            playbook("playbook:down", TriggerKind::ServiceDown),
            playbook(
                "playbook:analysis",
                TriggerKind::TicketClass {
                    name: CAPABILITY_ROOT.to_string(),
                },
            ),
            playbook(
                "playbook:image",
                TriggerKind::TicketClass {
                    name: "eg:modality/image".to_string(),
                },
            ),
        ]);
        let flapping = match_playbook(&index, &TriggerKind::ServiceFlapping, "e1", &[]);
        let ids: Vec<&str> = flapping.iter().map(|m| m.playbook_id.as_str()).collect();
        assert_eq!(ids, vec!["playbook:a-flap", "playbook:b-flap"]);
        assert!(flapping.iter().all(|m| m.chain.is_empty()));
        assert!(match_playbook(&index, &TriggerKind::ResourcePressure, "e1", &[]).is_empty());

        // The ticket class is entailed through the native vocabulary: an
        // analysis event is a capability, and is not an image.
        let ticket = TriggerKind::TicketClass {
            name: "any".to_string(),
        };
        let classes = vec!["eg:capability/analysis".to_string()];
        let matched = match_playbook(&index, &ticket, "e1", &classes);
        let ids: Vec<&str> = matched.iter().map(|m| m.playbook_id.as_str()).collect();
        assert_eq!(ids, vec!["playbook:analysis"]);
        assert_eq!(
            matched[0].chain.first().map(|e| e.narrower.as_str()),
            Some("e1")
        );
        assert!(matched[0].chain.len() >= 2);
        assert!(match_playbook(&index, &ticket, "e1", &[]).is_empty());
    }
}
