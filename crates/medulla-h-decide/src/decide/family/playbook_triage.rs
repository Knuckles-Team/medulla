//! The `playbook_triage` question family (MEDULLA-DECISION-ENGINE-R704.3).
//!
//! Which remediation steps an inbound event burst calls for. The candidates
//! are the caller's own events, declared one option each: `option_id` is the
//! event id, `classification` is `[service, event kind, ticket classes...]`
//! and the number `at_ms` is the event time in whole milliseconds (not Q32).
//! The playbooks come from the tenant's Agent Library (kind `Playbook`), so
//! another tenant's playbooks never match.
//!
//! Events go through the `Correlator` (its `state_change` kind counts toward flapping) (window `knob playbook.window`, flapping
//! at `knob playbook.flap_min_events`). A flapping service selects
//! `service_flapping` playbooks, an event of kind `service_down` or
//! `resource_pressure` selects the playbooks of that trigger, and an event that
//! carries ticket classes selects `ticket_class` playbooks whose class one of
//! them entails (the event triage class of R041 is the trigger kind). The
//! outcome is `Advisory`: one option `step:<playbook_id>:<step_id>` per
//! proposed step, in playbook-id then step order, scored `1 << 32`. A step that
//! writes is only named, with the governed route (`connector_write_back`,
//! R043) a premise records; the engine never runs it and writes nothing.

use std::collections::{BTreeMap, BTreeSet};

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_stats::decision::playbook::{
    match_playbook, Correlator, CorrelatorLimits, PlaybookEvent, PlaybookIndex,
};
use medulla_decision_types::decision::playbook::{Playbook, TriggerKind};
use medulla_decision_types::decision::request::LibraryCandidateScope;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{CandidateSource, DecideRequest};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::candidates::library_candidates;
use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory_scored, invalid};
use super::DecisionFamily;

/// The Q32 score of a proposed step.
const STEP_SCORE: i64 = 1i64 << 32;
/// The declared number that carries the event time.
const AT_MS: &str = "at_ms";
/// The event kinds that name a trigger of their own.
const SERVICE_DOWN: &str = "service_down";
const RESOURCE_PRESSURE: &str = "resource_pressure";
/// The question kind a write step routes through (MEDULLA-DECISION-ENGINE-R043).
const WRITE_ROUTE: &str = "connector_write_back";

/// The family of the `playbook_triage` question kind.
pub struct PlaybookTriageFamily;

impl DecisionFamily for PlaybookTriageFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        events(request).map(drop)
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(triage(ctx, request))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<medulla_decision_types::decision::PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }
}

/// One event as declared, with the ticket classes it carries.
struct Declared {
    event: PlaybookEvent,
    classes: Vec<String>,
}

/// The events of a question: every option is one event, or the question is
/// refused.
fn events(request: &DecideRequest) -> Result<Vec<Declared>, String> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid(
            "a playbook_triage question declares its events as candidates",
        ));
    };
    options.iter().map(declared_event).collect()
}

fn declared_event(option: &DeclaredOption) -> Result<Declared, String> {
    let classification = option.classification.as_slice();
    let [service, kind, classes @ ..] = classification else {
        return Err(invalid(format!(
            "event {} names its service and kind in classification",
            option.option_id
        )));
    };
    let at_ms = option
        .numbers
        .iter()
        .find(|number| number.key == AT_MS)
        .and_then(|number| u64::try_from(number.q32).ok())
        .ok_or_else(|| {
            invalid(format!(
                "event {} needs a non-negative number {AT_MS}",
                option.option_id
            ))
        })?;
    Ok(Declared {
        event: PlaybookEvent {
            event_id: option.option_id.clone(),
            service: service.clone(),
            kind: kind.clone(),
            at_ms,
        },
        classes: classes.to_vec(),
    })
}

/// A trigger an event or a flapping service raised, with the playbooks it
/// selected.
struct Raised {
    trigger: TriggerKind,
    service: String,
    event_id: String,
}

fn triage(ctx: &ExecutionContext, request: &DecideRequest) -> Result<Executed, String> {
    let mut declared = events(request)?;
    // Events of one service reach the correlator in time order.
    declared.sort_by(|a, b| {
        (a.event.at_ms, &a.event.event_id).cmp(&(b.event.at_ms, &b.event.event_id))
    });
    let knobs = medulla_server_api::knob_config::knobs();
    let mut correlator = Correlator::new(CorrelatorLimits {
        window_ms: knobs.playbook_window.saturating_mul(1_000),
        flap_min_events: usize::try_from(knobs.playbook_flap_min_events).unwrap_or(usize::MAX),
    });
    for item in &declared {
        correlator
            .push(item.event.clone())
            .map_err(|e| invalid(format!("{e:?}")))?;
    }

    let scope = LibraryCandidateScope {
        kinds: BoundedVec::new(vec![AgentComponentKind::Playbook]).map_err(invalid)?,
        classification_under: None,
    };
    let library = library_candidates(ctx.store, ctx.tenant_id, &scope)?;
    let mut digests: BTreeMap<String, String> = BTreeMap::new();
    let mut playbooks: Vec<Playbook> = Vec::new();
    for entry in &library.entries {
        digests.insert(entry.component_id.clone(), entry.definition_digest.clone());
        playbooks.push(Playbook::from_attributes(
            &entry.component_id,
            entry.entry_revision,
            &entry.attributes,
        )?);
    }
    let by_id: BTreeMap<&str, &Playbook> = playbooks.iter().map(|p| (p.id.as_str(), p)).collect();
    let index = PlaybookIndex::new(playbooks.iter().cloned());

    let mut raised: Vec<Raised> = Vec::new();
    for service in correlator.flapping_services() {
        let newest = correlator
            .events(service)
            .last()
            .map(|event| event.event_id.clone())
            .unwrap_or_default();
        raised.push(Raised {
            trigger: TriggerKind::ServiceFlapping,
            service: service.to_string(),
            event_id: newest,
        });
    }
    for item in &declared {
        let trigger = match item.event.kind.as_str() {
            SERVICE_DOWN => Some(TriggerKind::ServiceDown),
            RESOURCE_PRESSURE => Some(TriggerKind::ResourcePressure),
            _ => None,
        };
        for trigger in trigger
            .into_iter()
            .chain(
                (!item.classes.is_empty()).then(|| TriggerKind::TicketClass {
                    name: String::new(),
                }),
            )
        {
            raised.push(Raised {
                trigger,
                service: item.event.service.clone(),
                event_id: item.event.event_id.clone(),
            });
        }
    }

    // A playbook proposes once, for the first trigger that selects it.
    let mut proposed: BTreeMap<String, (String, String)> = BTreeMap::new();
    for item in &raised {
        let classes: Vec<String> = declared
            .iter()
            .find(|d| d.event.event_id == item.event_id)
            .map(|d| d.classes.clone())
            .unwrap_or_default();
        for found in match_playbook(&index, &item.trigger, &item.event_id, &classes) {
            proposed.entry(found.playbook_id).or_insert_with(|| {
                (
                    item.trigger.attribute_value().to_string(),
                    item.service.clone(),
                )
            });
        }
    }

    let mut options: Vec<(String, i64)> = Vec::new();
    let mut premises: Vec<FamilyPremise> = Vec::new();
    let mut seen_steps: BTreeSet<String> = BTreeSet::new();
    for (playbook_id, (trigger, service)) in &proposed {
        let Some(playbook) = by_id.get(playbook_id.as_str()) else {
            continue;
        };
        premises.push(FamilyPremise {
            subject: "playbook_triage",
            fact: format!("trigger:{trigger}:service={service}:playbook={playbook_id}"),
            class: PremiseClass::Claim,
            source: FamilyPremiseSource::Request("candidates"),
        });
        if let Some(digest) = digests.get(playbook_id) {
            premises.push(FamilyPremise {
                subject: "playbook_triage",
                fact: format!("playbook:{playbook_id}:revision={}", playbook.revision),
                class: PremiseClass::Definition,
                source: FamilyPremiseSource::Publisher {
                    component_id: playbook_id.clone(),
                    definition_digest: digest.clone(),
                },
            });
        }
        for step in playbook.steps.iter() {
            let option_id = format!("step:{playbook_id}:{}", step.step_id);
            if !seen_steps.insert(option_id.clone()) {
                continue;
            }
            if step.writes {
                premises.push(FamilyPremise {
                    subject: "playbook_triage",
                    fact: format!(
                        "{option_id}:writes=true:route={WRITE_ROUTE}:action={}",
                        step.action_ref
                    ),
                    class: PremiseClass::Claim,
                    source: FamilyPremiseSource::Request("candidates"),
                });
            }
            options.push((option_id, STEP_SCORE));
        }
    }
    let mut executed = advisory_scored(options)?;
    executed.family_premises = premises;
    Ok(executed)
}
