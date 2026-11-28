//! Connector system prompts as candidates of the one `system_prompt` slot
//! (MEDULLA-DECISION-ENGINE-R814.3).
//!
//! The exact solve fills exactly one system prompt per agent. Before it runs, the
//! `SystemPrompt` entries already in the Agent Library scope are narrowed to the one
//! [`select_system_prompt`] picks for the request's required capabilities. There is no
//! second candidate read: the candidates are the scope's own entries.

use std::collections::BTreeSet;

use medulla_agent_types::agent_component::{AgentComponentEntry, AgentComponentKind};
use medulla_decision_outcome_types::decision::prompt_candidate::{PromptCandidate, PromptRole};
use {
    medulla_compute_assemble::assemble::prompt::select_system_prompt,
    medulla_compute_assemble::assemble::prompt::SelectedPrompt,
};

/// A component confidence of 1.0 in Q32: no component confidence fact exists, so every
/// candidate carries the same one.
const FULL_CONFIDENCE_Q32: i64 = 1 << 32;

/// The connector segment of a pack component id `mcp:<connector>/<kind>/<name>`: the text
/// between `mcp:` and the first `/`. Empty for an id that is not a pack id.
fn connector_of(component_id: &str) -> String {
    component_id
        .strip_prefix("mcp:")
        .and_then(|rest| rest.split_once('/'))
        .map(|(connector, _)| connector.to_string())
        .unwrap_or_default()
}

/// The candidate one system-prompt entry is, or `None` for an entry that provides no
/// capability (it serves nothing, so it is never a candidate).
fn candidate_of(entry: &AgentComponentEntry) -> Option<PromptCandidate> {
    if entry.kind != AgentComponentKind::SystemPrompt || entry.classification.is_empty() {
        return None;
    }
    Some(PromptCandidate {
        component_id: entry.component_id.clone(),
        connector: connector_of(&entry.component_id),
        role: PromptRole::System,
        serves: entry.classification.iter().cloned().collect(),
        digest: entry
            .content_digest
            .strip_prefix("sha256:")
            .unwrap_or(&entry.content_digest)
            .to_string(),
        confidence_q32: FULL_CONFIDENCE_Q32,
    })
}

/// Narrow the one system-prompt slot to the selected candidate.
///
/// With no candidate in scope, or no legal one, the entries pass through unchanged and the
/// answer is `None`: the solve then decides over the full set. With a selection, every
/// other `SystemPrompt` entry leaves the scope and the selection is answered.
pub fn narrow_system_prompts(
    entries: Vec<AgentComponentEntry>,
    required: &BTreeSet<String>,
    min_match_micros: i64,
) -> (Vec<AgentComponentEntry>, Option<SelectedPrompt>) {
    let candidates: Vec<PromptCandidate> = entries.iter().filter_map(candidate_of).collect();
    if candidates.is_empty() {
        return (entries, None);
    }
    match select_system_prompt(required, &candidates, min_match_micros) {
        Ok(selected) => {
            let kept = entries
                .into_iter()
                .filter(|entry| {
                    entry.kind != AgentComponentKind::SystemPrompt
                        || entry.component_id == selected.component_id
                })
                .collect();
            (kept, Some(selected))
        }
        Err(_) => (entries, None),
    }
}

/// Hide the system prompts of every connector whose pack projection failed ontology
/// admission (MEDULLA-DECISION-ENGINE-R814.4): such a prompt is never a candidate. Answers
/// the kept entries and the sorted component ids it hid. A connector whose status cannot be
/// read, or which is not a pack connector, hides nothing: only a recorded failure excludes.
pub fn exclude_failed_connectors(
    entries: Vec<AgentComponentEntry>,
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    tenant_id: &str,
) -> (Vec<AgentComponentEntry>, Vec<String>) {
    use medulla_catalog_types::connector_pack::PackProjectionState;
    use medulla_types::contract::ResourceId;
    use std::collections::BTreeMap;

    let mut failed: BTreeMap<String, bool> = BTreeMap::new();
    let mut hidden = Vec::new();
    let kept = entries
        .into_iter()
        .filter(|entry| {
            if entry.kind != AgentComponentKind::SystemPrompt {
                return true;
            }
            let connector = connector_of(&entry.component_id);
            if connector.is_empty() {
                return true;
            }
            let failed = *failed.entry(connector.clone()).or_insert_with(|| {
                ResourceId::new(connector.as_str())
                    .ok()
                    .and_then(|id| store.connector_pack_status(tenant_id, &id).ok())
                    .is_some_and(|status| {
                        matches!(status.projection, PackProjectionState::Failed { .. })
                    })
            });
            if failed {
                hidden.push(entry.component_id.clone());
            }
            !failed
        })
        .collect();
    hidden.sort();
    (kept, hidden)
}

/// The record of the narrowing: the selected prompt with the required capabilities it
/// serves, and the hidden prompts. `None` when nothing was selected or hidden.
pub fn prompt_selection(
    selected: Option<&SelectedPrompt>,
    hidden: Vec<String>,
    required: &BTreeSet<String>,
    entries: &[AgentComponentEntry],
) -> Option<medulla_decision_types::decision::record::PromptSelection> {
    use medulla_types::contract::BoundedVec;
    use {
        medulla_decision_types::decision::record::PromptSelection,
        medulla_decision_types::decision::record::SelectedPromptFacts,
    };

    if selected.is_none() && hidden.is_empty() {
        return None;
    }
    let selected = selected.and_then(|picked| {
        let entry = entries
            .iter()
            .find(|entry| entry.component_id == picked.component_id)?;
        let candidate = candidate_of(entry)?;
        let covered: Vec<String> = required.intersection(&candidate.serves).cloned().collect();
        Some(SelectedPromptFacts {
            prompt_component_id: picked.component_id.clone(),
            connector: candidate.connector,
            digest: picked.digest.clone(),
            covered_capabilities: BoundedVec::new(covered).ok()?,
            score_q32: picked.score_q32,
        })
    });
    Some(PromptSelection {
        selected,
        excluded: BoundedVec::new(hidden).ok()?,
    })
}
