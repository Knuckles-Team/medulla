//! The committed-decision reads of `DecisionLog` (MEDULLA-DECISION-ENGINE-R870.1,
//! R870.2): `List` pages a tenant's records, `Provenance` states what one record
//! rested on. Both read the record index `Get` reads, filter by each record's own
//! visibility and write nothing.

use medulla_decision_types::decision::knob_families::decision_reads::list_page_max;
use medulla_decision_types::decision::record::{PremiseProvenance, ResolutionKind};
use medulla_decision_types::decision::statistical::log::{
    DecisionListItem, DecisionLogList, DecisionProvenance, InputsState, ProvenanceFact,
    ProvenancePremise, ProvenanceRule,
};
use medulla_decision_types::decision::statistical::QuestionKind;
use medulla_persistence_records::agent_library::AgentLibraryStore;

use super::stat_log::{visible_entries, visible_entry, LogReader};

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
    let mut entries: Vec<_> = visible_entries(store, reader)?
        .into_iter()
        .filter(|entry| {
            let record = &entry.record;
            filter.question_kind.is_none_or(|k| k == record.question.kind)
                && filter.resolution_kind.is_none_or(|k| k == record.resolution_kind)
                && filter.from_ms.is_none_or(|from| record.created_at_ms >= from)
                && filter.to_ms.is_none_or(|to| record.created_at_ms <= to)
        })
        .collect();
    entries.sort_by(|a, b| {
        b.record
            .created_at_ms
            .cmp(&a.record.created_at_ms)
            .then_with(|| a.record.record_id.cmp(&b.record.record_id))
    });
    if let Some((at, id)) = &after {
        entries.retain(|entry| {
            let key = (entry.record.created_at_ms, entry.record.record_id.as_str());
            key.0 < *at || (key.0 == *at && key.1 > id.as_str())
        });
    }
    let limit = filter.limit as usize;
    let more = entries.len() > limit;
    entries.truncate(limit);
    let next_cursor = more
        .then(|| entries.last())
        .flatten()
        .map(|last| format!("{}:{}", last.record.created_at_ms, last.record.record_id));
    let records = entries
        .into_iter()
        .map(|entry| DecisionListItem {
            record_id: entry.record.record_id.clone(),
            question_kind: entry.record.question.kind,
            resolution_kind: entry.record.resolution_kind,
            outcome: entry.record.outcome.clone(),
            decided_at_ms: entry.record.created_at_ms,
            record_digest: entry.record.record_digest.clone(),
            inputs: InputsState::from(&entry.inputs),
        })
        .collect();
    Ok(DecisionLogList {
        records,
        next_cursor,
    })
}

/// What one committed record rested on. An unknown id and another tenant's id
/// answer with the same refusal, so existence is not disclosed.
pub(super) fn provenance(
    store: &AgentLibraryStore,
    reader: &LogReader,
    record_id: &str,
) -> Result<DecisionProvenance, String> {
    let Some(entry) = visible_entry(store, reader, record_id)? else {
        return Err("DECISION_NOT_FOUND: no committed decision with this id".to_string());
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
    let facts = record
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
    Ok(DecisionProvenance {
        record_id: record.record_id.clone(),
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
