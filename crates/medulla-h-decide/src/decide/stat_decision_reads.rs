//! The committed-decision reads of `DecisionLog` (MEDULLA-DECISION-ENGINE-R870.1,
//! R870.2, R870.3): `List` pages a tenant's records, `Provenance` states what one
//! record rested on, `WhyNot` re-solves why one option was not chosen. All three
//! filter by each record's own visibility and write nothing.

use medulla_decision_types::decision::knob_families::decision_reads::list_page_max;
use medulla_decision_types::decision::record::{PremiseProvenance, ResolutionKind};
use medulla_decision_types::decision::statistical::log::{
    AssemblyOutcomeState, DecisionListItem, DecisionLogList, DecisionProvenance, DecisionWhyNot,
    InputsState, ProvenanceFact, ProvenancePremise, ProvenanceRule, RecordKind,
};
use medulla_decision_types::decision::DecisionRecord;
use medulla_graph_core::graph::GraphCore;
use serde_json::Value;
use std::sync::Arc;
use medulla_decision_types::decision::statistical::QuestionKind;
use medulla_persistence_records::agent_library::AgentLibraryStore;

use super::stat_log::{visible_entries, visible_entry, LogReader, MAX_LOG_ROWS};

/// A page filter: the optional narrowing fields of `List`.
pub(super) struct ListFilter {
    pub(super) question_kind: Option<QuestionKind>,
    pub(super) resolution_kind: Option<ResolutionKind>,
    pub(super) from_ms: Option<u64>,
    pub(super) to_ms: Option<u64>,
    pub(super) limit: u32,
    pub(super) cursor: Option<String>,
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: {detail}")
}

/// The sort key a cursor names: `<decided_at_ms>:<record_id>`.
fn parse_cursor(cursor: &str) -> Result<(u64, String), String> {
    let (at, id) = cursor
        .split_once(':')
        .ok_or_else(|| invalid("cursor is not a list cursor"))?;
    let at = at
        .parse::<u64>()
        .map_err(|_| invalid("cursor is not a list cursor"))?;
    if id.is_empty() {
        return Err(invalid("cursor is not a list cursor"));
    }
    Ok((at, id.to_string()))
}

/// One page of the caller's visible records, `decided_at_ms` descending then
/// `record_id` ascending.
pub(super) fn list(
    store: &AgentLibraryStore,
    reader: &LogReader,
    filter: &ListFilter,
) -> Result<DecisionLogList, String> {
    let page_max = list_page_max();
    if filter.limit == 0 || filter.limit > page_max {
        return Err(invalid(format!(
            "limit must be between 1 and {page_max}, got {}",
            filter.limit
        )));
    }
    if let (Some(from), Some(to)) = (filter.from_ms, filter.to_ms) {
        if from > to {
            return Err(invalid("from_ms is after to_ms"));
        }
    }
    let after = filter.cursor.as_deref().map(parse_cursor).transpose()?;
    let mut items: Vec<DecisionListItem> = visible_entries(store, reader)?
        .into_iter()
        .map(|entry| DecisionListItem {
            record_id: entry.record.record_id.clone(),
            record_kind: RecordKind::Statistical,
            question_kind: Some(entry.record.question.kind),
            resolution_kind: entry.record.resolution_kind,
            outcome: Some(entry.record.outcome.clone()),
            assembly_outcome: None,
            decided_at_ms: entry.record.created_at_ms,
            record_digest: entry.record.record_digest.clone(),
            inputs: InputsState::from(&entry.inputs),
        })
        .collect();
    // Assembly records are tenant-visible, like the library they decided over.
    for (_, body) in store.decision_record_bodies(&reader.tenant_id, MAX_LOG_ROWS)? {
        let record: DecisionRecord = serde_json::from_slice(&body)
            .map_err(|error| format!("DECISION_RECORD_UNREADABLE: {error}"))?;
        items.push(assembly_item(&record));
    }
    items.retain(|item| {
        filter.question_kind.is_none_or(|k| item.question_kind == Some(k))
            && filter.resolution_kind.is_none_or(|k| k == item.resolution_kind)
            && filter.from_ms.is_none_or(|from| item.decided_at_ms >= from)
            && filter.to_ms.is_none_or(|to| item.decided_at_ms <= to)
    });
    items.sort_by(|a, b| {
        b.decided_at_ms
            .cmp(&a.decided_at_ms)
            .then_with(|| a.record_id.cmp(&b.record_id))
    });
    if let Some((at, id)) = &after {
        items.retain(|item| {
            item.decided_at_ms < *at || (item.decided_at_ms == *at && item.record_id.as_str() > id.as_str())
        });
    }
    let limit = filter.limit as usize;
    let more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = more
        .then(|| items.last())
        .flatten()
        .map(|last| format!("{}:{}", last.decided_at_ms, last.record_id));
    Ok(DecisionLogList {
        records: items,
        next_cursor,
    })
}

/// The list item of one committed assembly record. Its inputs are always held
/// inline: only statistical entries are compacted.
fn assembly_item(record: &DecisionRecord) -> DecisionListItem {
    DecisionListItem {
        record_id: record.record_id.clone(),
        record_kind: RecordKind::Assembly,
        question_kind: None,
        resolution_kind: record.resolution_kind,
        outcome: None,
        assembly_outcome: Some(if record.is_solved() {
            AssemblyOutcomeState::Solved
        } else {
            AssemblyOutcomeState::Abstained
        }),
        decided_at_ms: record.created_at_ms,
        record_digest: record.record_digest.clone(),
        inputs: InputsState::Inline,
    }
}

/// What one committed record rested on. An unknown id and another tenant's id
/// answer with the same refusal, so existence is not disclosed.
pub(super) fn provenance(
    store: &AgentLibraryStore,
    reader: &LogReader,
    record_id: &str,
    graphs: &[(String, Arc<GraphCore>)],
) -> Result<DecisionProvenance, String> {
    let Some(entry) = visible_entry(store, reader, record_id)? else {
        return assembly_provenance(store, reader, record_id);
    };
    let record = &entry.record;
    let premises = record
        .premises
        .as_slice()
        .iter()
        .map(|premise| ProvenancePremise {
            kind: serde_json::to_value(&premise.provenance)
                .ok()
                .and_then(|value| value.get("provenance").and_then(|t| t.as_str().map(String::from)))
                .unwrap_or_default(),
            reference: premise.subject.clone(),
            class: premise.class,
        })
        .collect();
    // The graph rows keep the write version the decision read them at.
    let sealed_facts: Vec<ProvenanceFact> = record
        .inputs
        .graph_rows
        .as_slice()
        .iter()
        .map(|row| ProvenanceFact {
            node_id: row.node_id.clone(),
            revision: row.graph_version,
            digest: record.inputs_digest.clone(),
        })
        .collect();
    let sealed_rules: Vec<ProvenanceRule> = record
        .premises
        .as_slice()
        .iter()
        .filter_map(|premise| match &premise.provenance {
            PremiseProvenance::Policy { policy_digest } => Some(ProvenanceRule {
                rule_id: premise.subject.clone(),
                version: 0,
                digest: policy_digest.clone(),
            }),
            _ => None,
        })
        .collect();
    // The `:ActionRecord` rows reached through the record's gate decision state the
    // revision and digest each fact and rule was used at; the sealed premises
    // answer only when the record has none.
    let (facts, rules) = match action_links(record.inputs.graph_rows.as_slice(), graphs) {
        Some(links) => links,
        None => (sealed_facts, sealed_rules),
    };
    Ok(DecisionProvenance {
        record_id: record.record_id.clone(),
        record_kind: RecordKind::Statistical,
        record_digest: record.record_digest.clone(),
        premises,
        facts,
        rules,
        approver: entry
            .approval
            .as_ref()
            .map(|approval| approval.reviewer_principal.clone()),
        inputs: InputsState::from(&entry.inputs),
    })
}

/// The provenance of a committed assembly record: its sealed premises, the
/// candidate revisions it was decided over as facts, the policy premises as
/// rules, and the approval receipt its component carries. An unknown id and
/// another tenant's id answer with the same refusal.
fn assembly_provenance(
    store: &AgentLibraryStore,
    reader: &LogReader,
    record_id: &str,
) -> Result<DecisionProvenance, String> {
    let not_found = || "DECISION_NOT_FOUND: no committed decision with this id".to_string();
    let body = store
        .decision_record_body(&reader.tenant_id, record_id)?
        .ok_or_else(not_found)?;
    let record: DecisionRecord = serde_json::from_slice(&body)
        .map_err(|error| format!("DECISION_RECORD_UNREADABLE: {error}"))?;
    let premises = record
        .premises
        .as_slice()
        .iter()
        .map(|premise| ProvenancePremise {
            kind: serde_json::to_value(&premise.provenance)
                .ok()
                .and_then(|value| value.get("provenance").and_then(|t| t.as_str().map(String::from)))
                .unwrap_or_default(),
            reference: premise.subject.clone(),
            class: premise.class,
        })
        .collect();
    let facts = record
        .inputs
        .candidates
        .as_slice()
        .iter()
        .map(|candidate| ProvenanceFact {
            node_id: candidate.component_id.clone(),
            revision: candidate.entry_revision,
            digest: candidate.definition_digest.clone(),
        })
        .collect();
    let rules = record
        .premises
        .as_slice()
        .iter()
        .filter_map(|premise| match &premise.provenance {
            PremiseProvenance::Policy { policy_digest } => Some(ProvenanceRule {
                rule_id: premise.subject.clone(),
                version: 0,
                digest: policy_digest.clone(),
            }),
            _ => None,
        })
        .collect();
    let approver = store
        .current_component(&reader.tenant_id, record_id)?
        .and_then(|component| {
            component
                .attributes
                .get(medulla_persistence_records::decision_record::APPROVAL_ATTRIBUTE)
                .cloned()
        })
        .and_then(|receipt| {
            serde_json::from_str::<medulla_decision_types::decision::approval::RecommendationApproval>(&receipt)
                .ok()
        })
        .map(|approval| approval.reviewer_principal);
    Ok(DecisionProvenance {
        record_id: record.record_id.clone(),
        record_kind: RecordKind::Assembly,
        record_digest: record.record_digest.clone(),
        premises,
        facts,
        rules,
        approver,
        inputs: InputsState::Inline,
    })
}

/// The graphs a record's rows were read from, opened for the caller; empty when
/// the record is not visible. The async half of [`provenance`].
pub(super) async fn provenance_graphs(
    state: &super::SharedState,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    store: &AgentLibraryStore,
    reader: &LogReader,
    record_id: &str,
) -> Result<Vec<(String, Arc<GraphCore>)>, String> {
    let Some(entry) = visible_entry(store, reader, record_id)? else {
        return Ok(Vec::new());
    };
    let mut names: Vec<&str> = entry
        .record
        .inputs
        .graph_rows
        .as_slice()
        .iter()
        .map(|row| row.graph.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    let mut opened = Vec::new();
    for name in names {
        let (core, _) = super::stat_support::readable_graph(
            state,
            verified.agent_id(),
            name,
            medulla_decision_types::decision::statistical::StatisticalErrorCode::ParameterInvalid,
        )
        .await?;
        opened.push((name.to_string(), core));
    }
    Ok(opened)
}

fn properties_of(core: &GraphCore, node: &str) -> Option<Value> {
    rmp_serde::from_slice(&core.get_node_properties(node)?).ok()
}

/// Whether the edge `source -> target` carries `relationship`.
fn has_edge(core: &GraphCore, source: &str, target: &str, relationship: &str) -> bool {
    core.get_edge_properties(source, target).iter().any(|bytes| {
        rmp_serde::from_slice::<Value>(bytes)
            .ok()
            .is_some_and(|edge| edge.get("relationship").and_then(Value::as_str) == Some(relationship))
    })
}

/// The facts and rules of the `:ActionRecord`s approved by a gate decision among
/// the record's graph rows. The gate decision is followed along its
/// `decisionRequest` edge (MEDULLA-DECISION-ENGINE-R869.2); an action is one
/// whose `approvedBy` edge reaches the gate. `None` when no such action exists.
fn action_links(
    rows: &[medulla_decision_types::decision::statistical::graph_rows::GraphRowProvenance],
    graphs: &[(String, Arc<GraphCore>)],
) -> Option<(Vec<ProvenanceFact>, Vec<ProvenanceRule>)> {
    let (mut facts, mut rules, mut found) = (Vec::new(), Vec::new(), false);
    for row in rows {
        let Some((_, core)) = graphs.iter().find(|(name, _)| *name == row.graph) else {
            continue;
        };
        let is_gate = properties_of(core, &row.node_id)
            .is_some_and(|node| node.get("type").and_then(Value::as_str) == Some("GateDecision"));
        let decides_a_request = is_gate
            && core.get_successors(&row.node_id).ok().is_some_and(|targets| {
                targets
                    .iter()
                    .any(|target| has_edge(core, &row.node_id, target, "decisionRequest"))
            });
        if !decides_a_request {
            continue;
        }
        let mut actions = core.get_predecessors(&row.node_id).unwrap_or_default();
        actions.sort();
        for action in actions.iter().filter(|action| {
            has_edge(core, action, &row.node_id, "approvedBy")
                && properties_of(core, action)
                    .is_some_and(|node| node.get("type").and_then(Value::as_str) == Some("ActionRecord"))
        }) {
            found = true;
            let mut used = core.get_successors(action).unwrap_or_default();
            used.sort();
            for target in used {
                let Some(node) = properties_of(core, &target) else {
                    continue;
                };
                let text = |key: &str| node.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
                let number = |key: &str| node.get(key).and_then(Value::as_u64).unwrap_or_default();
                if has_edge(core, action, &target, "usedFact") {
                    facts.push(ProvenanceFact {
                        node_id: text("factNode"),
                        revision: number("factRevision"),
                        digest: text("factDigest"),
                    });
                } else if has_edge(core, action, &target, "usedRule") {
                    rules.push(ProvenanceRule {
                        rule_id: text("ruleNode"),
                        version: number("ruleVersion"),
                        digest: text("ruleDigest"),
                    });
                }
            }
        }
    }
    found.then_some((facts, rules))
}

/// Why one option was not chosen by a committed assembly record: the stored
/// inputs are re-derived (the digest must match the record's, else
/// `DECISION_REPLAY_MISMATCH`), then the option is forced in under the bounded
/// budget. Reads the tenant's own record only; changes nothing.
pub(super) fn why_not(
    store: &AgentLibraryStore,
    tenant_id: &str,
    record_id: &str,
    option_id: &str,
    budget_ms: Option<u32>,
) -> Result<DecisionWhyNot, String> {
    let not_found = || "DECISION_NOT_FOUND: no committed decision with this id".to_string();
    let body = store
        .decision_record_body(tenant_id, record_id)?
        .ok_or_else(not_found)?;
    let record: DecisionRecord = serde_json::from_slice(&body)
        .map_err(|error| format!("DECISION_INPUTS_RETIRED: the stored inputs cannot rebuild the problem: {error}"))?;
    let replayed = medulla_compute::assemble::assemble(
        record.inputs.clone(),
        medulla_compute::assemble::RecordIdentity::of(&record),
    )
    .map_err(|error| match error.code {
        medulla_decision_types::decision::DecisionErrorCode::AssemblyInputsInvalid => {
            format!("DECISION_INPUTS_RETIRED: {error}")
        }
        _ => error.to_string(),
    })?;
    if replayed.record.record_digest != record.record_digest {
        return Err(format!(
            "{}: re-deriving the record from its stored inputs reached a different record",
            medulla_decision_types::decision::DecisionErrorCode::DecisionReplayMismatch.as_str()
        ));
    }
    let budget = medulla_decision_types::decision::knob_families::decision_reads::why_not_budget_ms(budget_ms);
    let clock = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64)
    };
    let explained = medulla_compute::assemble::explain_option(&record.inputs, option_id, budget, &clock)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "OPTION_NOT_VISIBLE: the option is not among the record's candidates".to_string())?;
    Ok(DecisionWhyNot {
        record_id: record.record_id,
        option_id: option_id.to_string(),
        verdict: explained.verdict,
        reasons: explained.reasons,
        budget_used_ms: explained.budget_used_ms,
        budget_exhausted: explained.budget_exhausted,
    })
}
