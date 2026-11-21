//! Agent Library entries as membership triples (MEDULLA-ORCH-VOCAB-R003.6).
//!
//! [`project_entry`] is the one mapping the projection worker
//! (MEDULLA-ORCH-VOCAB-R003.6.1) commits. Every triple it returns has the
//! entry's own agent as its subject, so replacing or removing one entry's
//! triples never touches a triple another entry projected. A retired or
//! withdrawn revision projects nothing.

use medulla_agent_types::agent_component::{
    AgentComponentEntry, AgentComponentFacts, AgentComponentKind, A2A_AGENT_KIND_ATTRIBUTE,
};
use medulla_agent_types::agent_guardrail::GuardrailRule;
use medulla_agent_types::agent_library::{AgentLibraryEntry, AgentLibraryLifecycle};
use medulla_rdf::oxrdf::vocab::xsd;
use medulla_rdf::oxrdf::{Literal, NamedNode, Triple};

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

/// The IRI of one tenant's agent component.
pub fn component_iri(tenant: &str, component_id: &str) -> String {
    format!("{CORE}component/{}/{}", segment(tenant), segment(component_id))
}

/// The triples of one published component revision (MEDULLA-ORCH-VOCAB-R003.6). Only a
/// published component of a projected kind projects; every other kind and every revision that is
/// retired or withdrawn projects nothing. For an A2A agent card, in this order: `a :A2AAgentCard`,
/// `:memberOfLibrary` its tenant's library, one `:providesCapability` per declared capability in
/// stored order and `:hasAgentKind` when the card carries one. A value that is not an IRI cannot
/// be stored (publish refuses it) and writes no triple.
pub fn project_component(entry: &AgentComponentEntry) -> Vec<Triple> {
    if entry.lifecycle != AgentLibraryLifecycle::Published {
        return Vec::new();
    }
    let subject = NamedNode::new_unchecked(component_iri(&entry.tenant_id, &entry.component_id));
    match entry.kind {
        AgentComponentKind::A2aAgentCard => project_a2a_card(&subject, entry),
        AgentComponentKind::GuardrailRule => project_guardrail_rule(&subject, entry),
        AgentComponentKind::ModelProfile => project_model_profile(&subject, entry),
        _ => Vec::new(),
    }
}

fn project_a2a_card(subject: &NamedNode, entry: &AgentComponentEntry) -> Vec<Triple> {
    let mut triples = vec![
        Triple::new(
            subject.clone(),
            NamedNode::new_unchecked(RDF_TYPE),
            term("A2AAgentCard"),
        ),
        Triple::new(
            subject.clone(),
            term("memberOfLibrary"),
            NamedNode::new_unchecked(library_iri(&entry.tenant_id)),
        ),
    ];
    for capability in &entry.declared_capabilities {
        if let Ok(capability) = NamedNode::new(capability.as_str()) {
            triples.push(Triple::new(
                subject.clone(),
                term("providesCapability"),
                capability,
            ));
        }
    }
    if let Some(kind) = entry.attributes.get(A2A_AGENT_KIND_ATTRIBUTE) {
        if let Ok(kind) = NamedNode::new(kind.as_str()) {
            triples.push(Triple::new(subject.clone(), term("hasAgentKind"), kind));
        }
    }
    triples
}

/// A published guardrail rule as a best practice (MEDULLA-ORCH-VOCAB-R004.4), in this order:
/// `a :BestPractice`, `:memberOfLibrary` its tenant's library, `:appliesToTaskClass` the rule's
/// class. Publish refuses a rule whose class is missing or not an IRI, so a rule that does not
/// decode (or whose class is not a valid IRI) is unreachable and writes no triple.
fn project_guardrail_rule(subject: &NamedNode, entry: &AgentComponentEntry) -> Vec<Triple> {
    let Ok(rule) = GuardrailRule::from_attributes(&entry.component_id, &entry.attributes) else {
        return Vec::new();
    };
    let Ok(class) = NamedNode::new(rule.applies_to_class.as_str()) else {
        return Vec::new();
    };
    vec![
        Triple::new(
            subject.clone(),
            NamedNode::new_unchecked(RDF_TYPE),
            term("BestPractice"),
        ),
        Triple::new(
            subject.clone(),
            term("memberOfLibrary"),
            NamedNode::new_unchecked(library_iri(&entry.tenant_id)),
        ),
        Triple::new(subject.clone(), term("appliesToTaskClass"), class),
    ]
}

/// A published model profile with its context window (MEDULLA-ORCH-VOCAB-R005.5), in this
/// order: `a :ModelProfile`, `:memberOfLibrary` its tenant's library, `:contextWindow` as an
/// `xsd:integer`. The window is the typed fact, not an attribute; facts of another variant
/// (unreachable, publish binds facts to kind) write no triple.
fn project_model_profile(subject: &NamedNode, entry: &AgentComponentEntry) -> Vec<Triple> {
    let AgentComponentFacts::ModelProfile {
        context_window_tokens,
        ..
    } = &entry.facts
    else {
        return Vec::new();
    };
    vec![
        Triple::new(
            subject.clone(),
            NamedNode::new_unchecked(RDF_TYPE),
            term("ModelProfile"),
        ),
        Triple::new(
            subject.clone(),
            term("memberOfLibrary"),
            NamedNode::new_unchecked(library_iri(&entry.tenant_id)),
        ),
        Triple::new(
            subject.clone(),
            term("contextWindow"),
            Literal::new_typed_literal(context_window_tokens.to_string(), xsd::INTEGER),
        ),
    ]
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

    fn card(
        kind: AgentComponentKind,
        capabilities: &[&str],
        attributes: &[(&str, &str)],
    ) -> AgentComponentEntry {
        use medulla_agent_types::agent_component::{
            AgentComponentDraft, DraftPublication, DraftSubject,
        };
        let digest = format!("sha256:{}", "1".repeat(64));
        let mut draft = AgentComponentDraft::bare(
            DraftSubject {
                component_id: "card-a",
                kind,
                version: "1.0.0",
                content_digest: &digest,
                summary: "card",
            },
            DraftPublication {
                tenant_id: "tenant-a",
                actor_scope: "agent-builder",
                purpose_id: "agent-construction",
                policy_digest: &digest,
                source_revision: "rev-1",
                source_revision_digest: &digest,
            },
        );
        draft.declared_capabilities = capabilities.iter().map(|c| c.to_string()).collect();
        draft.attributes = attributes
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        AgentComponentEntry::publish(draft, 1, 1).expect("the component publishes")
    }

    // spec: MEDULLA-ORCH-VOCAB-R003.6
    #[test]
    fn a2a_card_maps_to_member_with_capabilities_and_kind() {
        let kind = format!("{CORE}agentKindVerifier");
        let published = card(
            AgentComponentKind::A2aAgentCard,
            &["urn:vendor:summarize", "urn:vendor:translate"],
            &[("agent_kind", &kind)],
        );
        let subject = component_iri("tenant-a", "card-a");
        let triples = project_component(&published);
        let rendered: Vec<String> = triples.iter().map(|t| t.to_string()).collect();
        assert_eq!(
            rendered,
            vec![
                format!("<{subject}> <{RDF_TYPE}> <{CORE}A2AAgentCard>"),
                format!("<{subject}> <{CORE}memberOfLibrary> <{}>", library_iri("tenant-a")),
                format!("<{subject}> <{CORE}providesCapability> <urn:vendor:summarize>"),
                format!("<{subject}> <{CORE}providesCapability> <urn:vendor:translate>"),
                format!("<{subject}> <{CORE}hasAgentKind> <{kind}>"),
            ]
        );
        // No agent kind attribute, no :hasAgentKind.
        let plain = card(AgentComponentKind::A2aAgentCard, &[], &[]);
        assert_eq!(project_component(&plain).len(), 2);
        // A retired card and another kind project nothing.
        let mut retired = published.clone();
        retired.lifecycle = AgentLibraryLifecycle::Retired;
        assert!(project_component(&retired).is_empty());
        assert!(project_component(&card(AgentComponentKind::Tool, &[], &[])).is_empty());
    }

    // spec: MEDULLA-ORCH-VOCAB-R004.4
    #[test]
    fn guardrail_rule_maps_to_best_practice_with_scope() {
        let class = format!("{CORE}TaskClassRefactor");
        let rule = card(
            AgentComponentKind::GuardrailRule,
            &[],
            &[("applies_to_class", &class), ("source_generation", "gen-1")],
        );
        let subject = component_iri("tenant-a", "card-a");
        let rendered: Vec<String> = project_component(&rule)
            .iter()
            .map(|t| t.to_string())
            .collect();
        assert_eq!(
            rendered,
            vec![
                format!("<{subject}> <{RDF_TYPE}> <{CORE}BestPractice>"),
                format!("<{subject}> <{CORE}memberOfLibrary> <{}>", library_iri("tenant-a")),
                format!("<{subject}> <{CORE}appliesToTaskClass> <{class}>"),
            ]
        );
        let mut retired = rule.clone();
        retired.lifecycle = AgentLibraryLifecycle::Retired;
        assert!(project_component(&retired).is_empty());
        assert!(project_component(&card(AgentComponentKind::Tool, &[], &[])).is_empty());
    }

    // spec: MEDULLA-ORCH-VOCAB-R005.5
    #[test]
    fn model_profile_maps_with_context_window() {
        // Published as an opaque card, then re-kinded: the mapping reads only the stored fields.
        let mut published = card(AgentComponentKind::A2aAgentCard, &[], &[]);
        published.kind = AgentComponentKind::ModelProfile;
        published.facts = AgentComponentFacts::ModelProfile {
            provider: "vendor".into(),
            model_identity: "model-a".into(),
            context_window_tokens: 200_000,
            max_output_tokens: 8_192,
            supports_tools: true,
            supports_structured_output: true,
            supports_vision: false,
            input_capabilities: None,
            modalities: Default::default(),
            cost: None,
            latency_declared: None,
            latency_observed_ref: None,
            shell_proficiency: None,
            stopping_behaviour: None,
        };
        let subject = component_iri("tenant-a", "card-a");
        let rendered: Vec<String> = project_component(&published)
            .iter()
            .map(|t| t.to_string())
            .collect();
        assert_eq!(
            rendered,
            vec![
                format!("<{subject}> <{RDF_TYPE}> <{CORE}ModelProfile>"),
                format!("<{subject}> <{CORE}memberOfLibrary> <{}>", library_iri("tenant-a")),
                format!(
                    "<{subject}> <{CORE}contextWindow> \"200000\"^^<http://www.w3.org/2001/XMLSchema#integer>"
                ),
            ]
        );
        let mut retired = published.clone();
        retired.lifecycle = AgentLibraryLifecycle::Retired;
        assert!(project_component(&retired).is_empty());
    }
}
