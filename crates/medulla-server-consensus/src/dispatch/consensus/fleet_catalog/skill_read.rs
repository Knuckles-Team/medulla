//! The graph reads one skill rendering needs (MEDULLA-TYPED-PACKS-R100.3).
//!
//! A catalog read loads each listed skill's engine-held body and resolves the
//! ontology terms it references BEFORE the pure projection runs, so
//! `skill_row` stays a pure function of what was read and nothing is cached
//! between reads.

use medulla_agent_types::agent_component::AgentComponentEntry;

use super::rendered::{CodeDeltaRecord, OntologyTermRef, SkillGraphContent};

/// The pack attribute carrying the ontology references a skill declares,
/// whitespace-separated component ids, recorded by the pack importer from the
/// `SKILL.md` front-matter key `ontology_refs`.
pub const ATTR_SKILL_ONTOLOGY_REFS: &str = "skill.ontology_refs";

/// One skill's graph content at the revision it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillGraphRead {
    pub revision: u64,
    pub body: Option<Vec<u8>>,
    pub terms: Vec<OntologyTermRef>,
    pub code_delta: Vec<CodeDeltaRecord>,
}

impl SkillGraphRead {
    /// The content `entry` declares, with `body` as read from the engine body
    /// store and `defined` answering whether a referenced term is a published
    /// ontology component of the tenant.
    pub fn from_entry(
        entry: &AgentComponentEntry,
        body: Option<Vec<u8>>,
        defined: impl Fn(&str) -> bool,
    ) -> Self {
        let terms = declared_refs(entry)
            .map(|iri| OntologyTermRef {
                defined: defined(iri),
                iri: iri.to_string(),
            })
            .collect();
        Self {
            revision: entry.entry_revision,
            body,
            terms,
            code_delta: vec![CodeDeltaRecord {
                path: entry.source_revision.clone(),
                change_digest: entry.source_revision_digest.clone(),
            }],
        }
    }

    /// A skill whose body could not be read.
    pub fn without_body(entry: &AgentComponentEntry) -> Self {
        Self::from_entry(entry, None, |_| false)
    }
}

/// The ontology references `entry` declares.
pub fn declared_refs(entry: &AgentComponentEntry) -> impl Iterator<Item = &str> {
    entry
        .attributes
        .get(ATTR_SKILL_ONTOLOGY_REFS)
        .map(String::as_str)
        .unwrap_or_default()
        .split_whitespace()
}

impl SkillGraphContent for SkillGraphRead {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn skill_body(&self, _component_id: &str) -> Option<Vec<u8>> {
        self.body.clone()
    }

    fn referenced_terms(&self, _component_id: &str) -> Vec<OntologyTermRef> {
        self.terms.clone()
    }

    fn code_delta(&self, _component_id: &str) -> Vec<CodeDeltaRecord> {
        self.code_delta.clone()
    }
}
