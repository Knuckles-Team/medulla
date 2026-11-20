//! Skill packages rendered from graph content (MEDULLA-TYPED-PACKS-R100.2).
//!
//! Pure: every graph read goes through [`SkillGraphContent`], so the same graph
//! revision always yields the same package and nothing is written or cached.

use medulla_agent_types::agent_component::AgentComponentEntry;
use medulla_catalog_types::fleet_catalog::{
    RenderedSkillPackage, RENDERED_SKILL_PACKAGE_SCHEMA_VERSION,
};
use medulla_types::contract::{Digest256, ServerErrorCode};

/// One ontology term a skill references, and whether the graph defines it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OntologyTermRef {
    pub iri: String,
    pub defined: bool,
}

/// One change to the code a skill references since the last render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeDeltaRecord {
    pub path: String,
    pub change_digest: String,
}

/// The graph reads one rendering needs, at one revision.
pub trait SkillGraphContent {
    /// The graph revision every read below answers at.
    fn revision(&self) -> u64;
    /// The skill body, or `None` when the graph holds none for the component.
    fn skill_body(&self, component_id: &str) -> Option<Vec<u8>>;
    /// The ontology terms the skill references.
    fn referenced_terms(&self, component_id: &str) -> Vec<OntologyTermRef>;
    /// The code changes under the skill since the last render.
    fn code_delta(&self, component_id: &str) -> Vec<CodeDeltaRecord>;
}

/// Why a package could not be rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderRefusal {
    BodyMissing { component_id: String },
    DanglingOntologyRef { component_id: String, term: String },
}

impl RenderRefusal {
    /// The registered wire code of this refusal.
    pub fn code(&self) -> ServerErrorCode {
        match self {
            Self::BodyMissing { .. } => ServerErrorCode::RenderBodyMissing,
            Self::DanglingOntologyRef { .. } => ServerErrorCode::RenderDanglingOntologyRef,
        }
    }
}

impl std::fmt::Display for RenderRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code().as_str();
        match self {
            Self::BodyMissing { component_id } => {
                write!(f, "{code}: the graph holds no body for skill {component_id}")
            }
            Self::DanglingOntologyRef { component_id, term } => write!(
                f,
                "{code}: skill {component_id} references undefined term {term}"
            ),
        }
    }
}

fn section(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// Render `entry`'s skill package from the graph.
///
/// The content digest is SHA-256 over the canonical encoding of the skill body,
/// the referenced ontology terms (sorted) and the code delta (sorted), in that
/// order, each section length-prefixed.
pub fn render_skill_package(
    entry: &AgentComponentEntry,
    uri: &str,
    graph: &dyn SkillGraphContent,
) -> Result<RenderedSkillPackage, RenderRefusal> {
    let id = entry.component_id.as_str();
    let body = graph
        .skill_body(id)
        .ok_or_else(|| RenderRefusal::BodyMissing {
            component_id: id.to_string(),
        })?;
    let mut terms = graph.referenced_terms(id);
    terms.sort_by(|a, b| a.iri.cmp(&b.iri));
    terms.dedup_by(|a, b| a.iri == b.iri);
    if let Some(missing) = terms.iter().find(|term| !term.defined) {
        return Err(RenderRefusal::DanglingOntologyRef {
            component_id: id.to_string(),
            term: missing.iri.clone(),
        });
    }
    let mut delta = graph.code_delta(id);
    delta.sort_by(|a, b| (&a.path, &a.change_digest).cmp(&(&b.path, &b.change_digest)));

    let mut canonical = Vec::new();
    section(&mut canonical, &body);
    section(&mut canonical, &(terms.len() as u64).to_be_bytes());
    for term in &terms {
        section(&mut canonical, term.iri.as_bytes());
    }
    section(&mut canonical, &(delta.len() as u64).to_be_bytes());
    for change in &delta {
        section(&mut canonical, change.path.as_bytes());
        section(&mut canonical, change.change_digest.as_bytes());
    }
    Ok(RenderedSkillPackage {
        schema_version: RENDERED_SKILL_PACKAGE_SCHEMA_VERSION,
        uri: uri.to_string(),
        digest: Digest256::sha256(&canonical).to_hex(),
        ontology_refs: terms.into_iter().map(|term| term.iri).collect(),
        graph_revision: graph.revision(),
    })
}

#[cfg(test)]
#[path = "rendered_tests.rs"]
mod tests;
