//! The `probe_selection` question family (MEDULLA-DECISION-ENGINE-R818).
//!
//! Which red-team attacks and transforms to run for a stated goal. The
//! request holds `goal_text` and optional `target_categories`; the candidates
//! are the tenant's `RedTeamAttack` and `RedTeamTransform` components, read
//! from the Agent Library. A legal attack is one whose category is among the
//! target categories (every attack is legal when none is named). A transform
//! carries no category, so it applies to any attack and is legal once at least
//! one attack is. Legal entries are ranked by how many words of the goal their
//! own text names, ties broken by id, attacks before transforms. The sealed
//! answer is advisory and lists the identifiers, at most `red_team.max_probes`
//! of them. A goal that selects no legal attack refuses with
//! `NO_CATALOG_ENTRY_SELECTED`. Nothing is written.

use std::collections::BTreeSet;

use medulla_agent_types::agent_component::red_team::{RedTeamAttack, RedTeamTransform};
use medulla_agent_types::agent_component::{AgentComponentEntry, AgentComponentKind};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    codes, DecisionPolicy, EvidenceClass, LibraryCandidateScope, PremiseClass, PremiseRef,
    ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::candidates::library_candidates;
use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, invalid, param, text_param};
use super::DecisionFamily;

/// The family of the `probe_selection` question kind.
pub struct ProbeSelectionFamily;

/// The goal's lower-case alphanumeric words.
fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// How many of the goal's words `own` names.
fn overlap(goal: &BTreeSet<String>, own: &str) -> usize {
    words(own).intersection(goal).count()
}

/// The goal text of the request.
fn goal(request: &DecideRequest) -> Result<String, String> {
    match text_param(request, "goal_text")? {
        Some(text) if !text.trim().is_empty() => Ok(text.to_string()),
        _ => Err(invalid("parameter goal_text is a required non-blank text")),
    }
}

/// The target categories the request names; empty names every category.
fn categories(request: &DecideRequest) -> Result<BTreeSet<String>, String> {
    match param(request, "target_categories")? {
        None => Ok(BTreeSet::new()),
        Some(TypedValue::IriList(ids)) => Ok(ids.iter().cloned().collect()),
        Some(_) => Err(invalid("parameter target_categories is not an IRI list")),
    }
}

/// Order by overlap, highest first, then by id.
fn ranked(mut entries: Vec<(usize, &AgentComponentEntry)>) -> Vec<&AgentComponentEntry> {
    entries.sort_by(|(a, x), (b, y)| b.cmp(a).then_with(|| x.component_id.cmp(&y.component_id)));
    entries.into_iter().map(|(_, entry)| entry).collect()
}

fn definition(entry: &AgentComponentEntry) -> FamilyPremise {
    FamilyPremise {
        subject: "probe_selection",
        fact: format!("selected:{}", entry.component_id),
        class: PremiseClass::Definition,
        source: FamilyPremiseSource::Publisher {
            component_id: entry.component_id.clone(),
            definition_digest: entry.definition_digest.clone(),
        },
    }
}

fn conclude_selection(ctx: &ExecutionContext, request: &DecideRequest) -> Result<Executed, String> {
    let goal_text = goal(request)?;
    let wanted = categories(request)?;
    let goal_words = words(&goal_text);
    let scope = LibraryCandidateScope {
        kinds: BoundedVec::new(vec![
            AgentComponentKind::RedTeamAttack,
            AgentComponentKind::RedTeamTransform,
        ])
        .map_err(invalid)?,
        classification_under: None,
    };
    // The tenant's own components only: the read is keyed by the verified tenant.
    let entries = library_candidates(ctx.store, ctx.tenant_id, &scope)?.entries;
    let mut attacks = Vec::new();
    let mut transforms = Vec::new();
    for entry in &entries {
        match entry.kind {
            AgentComponentKind::RedTeamAttack => {
                let attack = RedTeamAttack::from_attributes(&entry.component_id, &entry.attributes)
                    .map_err(invalid)?;
                if wanted.is_empty() || wanted.contains(&attack.category) {
                    let own = format!("{} {}", attack.category, attack.goal);
                    attacks.push((overlap(&goal_words, &own), entry));
                }
            }
            AgentComponentKind::RedTeamTransform => {
                let transform =
                    RedTeamTransform::from_attributes(&entry.component_id, &entry.attributes)
                        .map_err(invalid)?;
                transforms.push((overlap(&goal_words, &transform.kind), entry));
            }
            _ => {}
        }
    }
    if attacks.is_empty() {
        return Err(invalid(format!(
            "{}: no attack of the tenant matches the categories of goal '{goal_text}'",
            codes::NO_CATALOG_ENTRY_SELECTED.as_str()
        )));
    }
    let cap = usize::try_from(medulla_server_api::knob_config::knobs().red_team_max_probes)
        .unwrap_or(usize::MAX);
    let selected: Vec<&AgentComponentEntry> = ranked(attacks)
        .into_iter()
        .chain(ranked(transforms))
        .take(cap)
        .collect();
    let mut executed = advisory(selected.iter().map(|e| e.component_id.clone()).collect())?;
    let mut premises: Vec<FamilyPremise> = selected.iter().map(|e| definition(e)).collect();
    premises.push(FamilyPremise {
        subject: "probe_selection",
        fact: format!("goal:{goal_text}"),
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request("params"),
    });
    executed.family_premises = premises;
    Ok(executed)
}

impl DecisionFamily for ProbeSelectionFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        goal(request)?;
        categories(request)?;
        match request.candidates {
            CandidateSource::AgentLibrary { .. } => Ok(()),
            _ => Err(invalid(
                "probe_selection reads its candidates from the Agent Library",
            )),
        }
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_selection(ctx, request))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    /// An advisory answer is capped at Claim (DE-ADVISORY-CLASS-CAP).
    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Claim)
    }
}
