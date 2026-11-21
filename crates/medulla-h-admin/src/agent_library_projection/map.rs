//! Agent Library entries as membership triples (MEDULLA-ORCH-VOCAB-R003.6).
//!
//! [`project_entry`] is the one mapping the projection worker
//! (MEDULLA-ORCH-VOCAB-R003.6.1) commits. Every triple it returns has the
//! entry's own agent as its subject, so replacing or removing one entry's
//! triples never touches a triple another entry projected. A retired or
//! withdrawn revision projects nothing.

use medulla_agent_types::agent_library::{AgentLibraryEntry, AgentLibraryLifecycle};
use medulla_rdf::oxrdf::{NamedNode, Triple};

/// The core ontology namespace the projected terms belong to.
pub const CORE: &str = "https://knuckles-team.github.io/ontology/core#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// One IRI path segment: unreserved bytes kept, every other byte
/// percent-encoded, so any valid tenant or agent id is a valid IRI segment.
pub(super) fn segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn term(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{CORE}{local}"))
}

/// The IRI of one tenant's agent.
pub fn agent_iri(tenant: &str, agent_id: &str) -> String {
    format!("{CORE}agent/{}/{}", segment(tenant), segment(agent_id))
}

/// The IRI of one tenant's agent template.
pub fn template_iri(tenant: &str, template_id: &str) -> String {
    format!(
        "{CORE}agentTemplate/{}/{}",
        segment(tenant),
        segment(template_id)
    )
}

/// The IRI of one tenant's Agent Library.
pub fn library_iri(tenant: &str) -> String {
    format!("{CORE}agentLibrary/{}", segment(tenant))
}

/// The membership triples of one entry revision: `a :Agent`,
/// `:memberOfLibrary` its tenant's library and, for an instance,
/// `:instanceOfTemplate` its template. A revision that is not published
/// projects nothing.
pub fn project_entry(entry: &AgentLibraryEntry) -> Vec<Triple> {
    if !matches!(entry.lifecycle, AgentLibraryLifecycle::Published) {
        return Vec::new();
    }
    let agent = NamedNode::new_unchecked(agent_iri(&entry.tenant_id, &entry.agent_id));
    let mut triples = vec![
        Triple::new(
            agent.clone(),
            NamedNode::new_unchecked(RDF_TYPE),
            term("Agent"),
        ),
        Triple::new(
            agent.clone(),
            term("memberOfLibrary"),
            NamedNode::new_unchecked(library_iri(&entry.tenant_id)),
        ),
    ];
    if let Some(template) = &entry.instantiated_from {
        triples.push(Triple::new(
            agent,
            term("instanceOfTemplate"),
            NamedNode::new_unchecked(template_iri(&entry.tenant_id, &template.template_id)),
        ));
    }
    triples
}

/// `triples` as one N-Triples document, one statement per line.
pub fn ntriples(triples: &[Triple]) -> String {
    triples
        .iter()
        .map(|triple| format!("{triple} .\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_percent_encode_reserved_bytes() {
        assert_eq!(segment("agent-a_1.~"), "agent-a_1.~");
        assert_eq!(segment("a/b c"), "a%2Fb%20c");
        assert_eq!(
            agent_iri("tenant-a", "x/y"),
            format!("{CORE}agent/tenant-a/x%2Fy")
        );
    }
}
