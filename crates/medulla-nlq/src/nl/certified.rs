//! Certified-definition check of a natural-language plan
//! (MEDULLA-FEDERATED-QUERY-R058.6).
//!
//! A `:GlossaryTerm` node with status `certified` holds the one approved
//! definition of a metric or entity: its `name`, `definition`, `version` and the
//! `lineage` (the stored labels, properties and relationships the definition is
//! computed from). Before a candidate UQL executes, [`check_certified_definitions`]
//! finds the terms the question uses and checks that the candidate references the
//! whole lineage of each. A term whose lineage is not covered is a typed mismatch
//! that goes back to the planner inside the bounded correction loop of
//! `plan_grounded`; the answer lists the terms it used.
//!
//! A certifier also records a correction (MEDULLA-FEDERATED-QUERY-R058.7): the
//! question, the rejected UQL and the approved UQL, stored as one `NlCorrection`
//! node by the served `NlQueryCorrect` (`medulla-h-graph`). [`matching_corrections`] feeds the approved pairs of the same question
//! into later planning context; a correction never changes a certified term.

use std::collections::BTreeSet;

use medulla_graph_core::graph::GraphView;
use medulla_query_types::nl_query::{CertifiedTermUse, NlCorrection, NL_CORRECTION_LABEL};

use super::context::question_terms;

/// The node label every glossary term carries.
pub const GLOSSARY_TERM_LABEL: &str = "GlossaryTerm";
/// The `status` value of a certified term.
pub const CERTIFIED_STATUS: &str = "certified";

/// One certified glossary term read off the caller's snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertifiedTerm {
    pub name: String,
    pub definition: String,
    pub version: String,
    pub lineage: Vec<String>,
}

/// A certified term the candidate uses without its whole lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionMismatch {
    pub term: String,
    pub definition: String,
    /// Lineage names the candidate never references.
    pub missing: Vec<String>,
}

impl std::fmt::Display for DefinitionMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NL_DEFINITION_CONFLICT: certified term `{}` means \"{}\"; the query must use {}",
            self.term,
            self.definition,
            self.missing.join(", ")
        )
    }
}

/// What the check found for one candidate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CertifiedCheck {
    /// Every certified term the question uses, in name order.
    pub used: Vec<CertifiedTermUse>,
    /// The used terms whose lineage the candidate does not cover.
    pub mismatches: Vec<DefinitionMismatch>,
}

fn norm(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

impl CertifiedTerm {
    /// The term a node holds, `None` unless it is a certified glossary term with
    /// a name, a version and a lineage.
    pub fn from_properties(props: &serde_json::Map<String, serde_json::Value>) -> Option<Self> {
        let text = |key: &str| {
            props
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let is_term = ["type", "node_type", "label"]
            .iter()
            .any(|k| props.get(*k).and_then(|v| v.as_str()) == Some(GLOSSARY_TERM_LABEL));
        if !is_term || text("status").as_deref() != Some(CERTIFIED_STATUS) {
            return None;
        }
        let lineage: Vec<String> = props
            .get("lineage")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        Some(Self {
            name: text("name")?,
            definition: text("definition").unwrap_or_default(),
            version: text("version")?,
            lineage,
        })
    }

    fn used_by(&self, question_tokens: &BTreeSet<String>) -> bool {
        let words: Vec<String> = question_terms(&self.name);
        !words.is_empty()
            && words
                .iter()
                .all(|w| question_tokens.contains(w) || question_tokens.contains(&format!("{w}s")))
    }
}

/// The node label of a recorded conflict between two sources' definitions of one term
/// (MEDULLA-FEDERATED-QUERY-R058.9).
pub const TERM_CONFLICT_LABEL: &str = "TermConflict";
/// The `status` of a conflict no certifier has resolved.
pub const CONFLICT_OPEN: &str = "open";

/// Two sources that define one glossary term with different meanings. The fact links
/// both definition nodes; it merges nothing (MEDULLA-FEDERATED-QUERY-R058.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermConflict {
    pub term: String,
    pub left_id: String,
    pub right_id: String,
    pub left_source: String,
    pub right_source: String,
    pub left_definition: String,
    pub right_definition: String,
}

impl TermConflict {
    /// The one node id of this pair, so recording it twice is one fact.
    pub fn node_id(&self) -> String {
        format!("term-conflict:{}:{}:{}", norm(&self.term), self.left_id, self.right_id)
    }

    /// The properties of the conflict fact node, status `open`.
    pub fn properties(&self) -> serde_json::Value {
        serde_json::json!({
            "type": TERM_CONFLICT_LABEL,
            "term": self.term,
            "left": self.left_id,
            "right": self.right_id,
            "left_source": self.left_source,
            "right_source": self.right_source,
            "status": CONFLICT_OPEN,
        })
    }
}

fn node_props(
    blob: &[u8],
) -> Option<serde_json::Map<String, serde_json::Value>> {
    match medulla_types::msgpack::decode_property_value(blob) {
        Ok(serde_json::Value::Object(props)) => Some(props),
        _ => None,
    }
}

fn prop_text(props: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    props
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn same_meaning(a: &str, b: &str) -> bool {
    let words = |text: &str| -> Vec<String> {
        text.split_whitespace().map(str::to_lowercase).collect()
    };
    words(a) == words(b)
}

/// Find the glossary terms two different sources define with the same name and different
/// meanings and return one [`TermConflict`] per pair not yet recorded. Definitions are
/// reported, never merged: neither node changes. A term from the same source, or a pair
/// with equal definitions, is no conflict.
pub fn reconcile_term_definitions(view: &GraphView) -> Vec<TermConflict> {
    // (normalized name) -> terms, each (node id, name, source, definition).
    let mut by_name: std::collections::BTreeMap<String, Vec<(String, String, String, String)>> =
        Default::default();
    for (id, blob) in view.node_properties.iter() {
        let Some(props) = node_props(blob.as_slice()) else {
            continue;
        };
        let is_term = ["type", "node_type", "label"]
            .iter()
            .any(|k| props.get(*k).and_then(|v| v.as_str()) == Some(GLOSSARY_TERM_LABEL));
        let name = prop_text(&props, "name");
        if !is_term || name.is_empty() {
            continue;
        }
        by_name.entry(norm(&name)).or_default().push((
            id.to_string(),
            name,
            prop_text(&props, "source"),
            prop_text(&props, "definition"),
        ));
    }
    let mut found = Vec::new();
    for terms in by_name.values_mut() {
        terms.sort();
        for (i, left) in terms.iter().enumerate() {
            for right in &terms[i + 1..] {
                if left.2 == right.2 || same_meaning(&left.3, &right.3) {
                    continue;
                }
                let conflict = TermConflict {
                    term: left.1.clone(),
                    left_id: left.0.clone(),
                    right_id: right.0.clone(),
                    left_source: left.2.clone(),
                    right_source: right.2.clone(),
                    left_definition: left.3.clone(),
                    right_definition: right.3.clone(),
                };
                if !view.node_properties.contains_key(conflict.node_id().as_str()) {
                    found.push(conflict);
                }
            }
        }
    }
    found.sort_by_key(TermConflict::node_id);
    found
}

/// The normalized names of terms with an open conflict fact.
fn open_conflict_terms(view: &GraphView) -> BTreeSet<String> {
    view.node_properties
        .values()
        .filter_map(|blob| node_props(blob.as_slice()))
        .filter(|props| {
            prop_text(props, "type") == TERM_CONFLICT_LABEL
                && prop_text(props, "status") == CONFLICT_OPEN
        })
        .map(|props| norm(&prop_text(&props, "term")))
        .collect()
}

/// The certified terms of `view`, in name order, one per name (the highest
/// version wins when a name repeats). A term with an open conflict is uncertified until
/// a certifier resolves it (MEDULLA-FEDERATED-QUERY-R058.9).
pub fn certified_terms(view: &GraphView) -> Vec<CertifiedTerm> {
    let conflicted = open_conflict_terms(view);
    let mut terms: std::collections::BTreeMap<String, CertifiedTerm> = Default::default();
    for blob in view.node_properties.values() {
        let Ok(serde_json::Value::Object(props)) =
            medulla_types::msgpack::decode_property_value(blob.as_slice())
        else {
            continue;
        };
        let Some(term) = CertifiedTerm::from_properties(&props) else {
            continue;
        };
        if conflicted.contains(&norm(&term.name)) {
            continue;
        }
        match terms.get(&term.name) {
            Some(held) if held.version >= term.version => {}
            _ => {
                terms.insert(term.name.clone(), term);
            }
        }
    }
    terms.into_values().collect()
}

/// Compare each certified term the `question` uses with the `candidate` UQL: the
/// candidate must reference every name of the term's lineage.
pub fn check_certified_definitions(
    terms: &[CertifiedTerm],
    question: &str,
    candidate: &str,
) -> CertifiedCheck {
    let tokens: BTreeSet<String> = question_terms(question).into_iter().collect();
    let identifiers: BTreeSet<String> = candidate
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
        .map(norm)
        .collect();
    let mut check = CertifiedCheck::default();
    for term in terms.iter().filter(|t| t.used_by(&tokens)) {
        check.used.push(CertifiedTermUse {
            term: term.name.clone(),
            version: term.version.clone(),
            lineage: term.lineage.clone(),
        });
        let missing: Vec<String> = term
            .lineage
            .iter()
            .filter(|name| !identifiers.contains(&norm(name)))
            .cloned()
            .collect();
        if !missing.is_empty() {
            check.mismatches.push(DefinitionMismatch {
                term: term.name.clone(),
                definition: term.definition.clone(),
                missing,
            });
        }
    }
    check
}

/// The planner hint lines for the certified terms the question uses.
pub fn certified_hint(terms: &[CertifiedTerm], question: &str) -> String {
    let tokens: BTreeSet<String> = question_terms(question).into_iter().collect();
    let mut out = String::new();
    for term in terms.iter().filter(|t| t.used_by(&tokens)) {
        out.push_str(&format!(
            "\nCertified term `{}` (version {}): {}. Lineage: {}.",
            term.name,
            term.version,
            term.definition,
            term.lineage.join(", ")
        ));
    }
    out
}

/// The correction records of `view`, newest first, then by question and UQL.
pub fn nl_corrections(view: &GraphView) -> Vec<NlCorrection> {
    let mut found: Vec<NlCorrection> = Vec::new();
    for blob in view.node_properties.values() {
        let Ok(serde_json::Value::Object(props)) =
            medulla_types::msgpack::decode_property_value(blob.as_slice())
        else {
            continue;
        };
        if props.get("type").and_then(|v| v.as_str()) != Some(NL_CORRECTION_LABEL) {
            continue;
        }
        let Some(json) = props.get("json").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Ok(correction) = serde_json::from_str::<NlCorrection>(json) {
            found.push(correction);
        }
    }
    found.sort_by(|a, b| {
        b.certified_at_ms
            .cmp(&a.certified_at_ms)
            .then_with(|| a.question.cmp(&b.question))
            .then_with(|| a.approved_uql.cmp(&b.approved_uql))
    });
    found
}

/// The corrections recorded for the same question: equal sets of question words,
/// so wording order, case and punctuation do not matter.
pub fn matching_corrections<'a>(
    corrections: &'a [NlCorrection],
    question: &str,
) -> Vec<&'a NlCorrection> {
    let wanted: BTreeSet<String> = question_terms(question).into_iter().collect();
    corrections
        .iter()
        .filter(|c| {
            question_terms(&c.question)
                .into_iter()
                .collect::<BTreeSet<_>>()
                == wanted
        })
        .collect()
}

/// The planner hint lines for the approved pairs recorded for `question`.
pub fn corrections_hint(corrections: &[NlCorrection], question: &str) -> String {
    let mut out = String::new();
    for c in matching_corrections(corrections, question) {
        out.push_str(&format!(
            "\nApproved correction (certified by {}): for this question use `{}`, not `{}`.",
            c.certifier, c.approved_uql, c.rejected_uql
        ));
    }
    out
}

#[cfg(test)]
#[path = "certified_tests.rs"]
mod tests;
