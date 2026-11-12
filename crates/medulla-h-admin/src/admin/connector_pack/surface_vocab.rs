//! The connector-surface terms import and projection share
//! (MEDULLA-TYPED-PACKS-R102.2, R102.5).
//!
//! A pack ontology states its surface with the `cp:` link predicates below.
//! Import checks those triples; projection writes them onto the surface nodes
//! it stamps. Both read the same table, so what import accepts is what
//! projection writes.

/// The connector-pack vocabulary namespace.
pub(super) const CP: &str = medulla_types::ontology_iri!("connector-pack#");
/// The connector node namespace the contract mapping also uses.
pub(super) const CONNECTOR_BASE: &str = medulla_types::ontology_iri!("connector-pack/connector/");
/// `rdf:type`.
pub(super) const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// What a surface link's object must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Object {
    /// A class of the composed schema.
    DomainClass,
    /// A member of this pack.
    Member,
    /// Anything (a feature, a core capability).
    Open,
}

/// The surface link predicates a pack ontology may declare, with what each
/// one's object is. One entry per line.
pub(super) const SURFACE_LINKS: [(&str, Object); 8] = [
    ("appliesTo", Object::DomainClass),
    ("executes", Object::DomainClass),
    ("hasContract", Object::Open),
    ("operatesOn", Object::DomainClass),
    ("produces", Object::DomainClass),
    ("providesCapability", Object::Open),
    ("providesFeature", Object::Open),
    ("realizedBy", Object::Member),
];

/// What a link predicate's object must be, or `None` when `predicate` is not a
/// surface link.
pub(super) fn link_object(predicate: &str) -> Option<Object> {
    let local = predicate.strip_prefix(CP)?;
    SURFACE_LINKS
        .iter()
        .find(|(name, _)| *name == local)
        .map(|(_, object)| *object)
}

/// The URI schemes of the member kinds a surface node is projected for.
pub(super) const MEMBER_SCHEMES: [&str; 4] = ["mcp-server://", "prompt://", "skill://", "tool://"];

/// The schemes whose first path segment names the connector
/// (`<scheme><connector>/<name>`); a skill or prompt URI names the entry only.
const CONNECTOR_SCHEMES: [&str; 2] = ["mcp-server://", "tool://"];

/// Whether `iri` is the URI of a pack member.
pub(super) fn is_member_uri(iri: &str) -> bool {
    MEMBER_SCHEMES.iter().any(|scheme| iri.starts_with(scheme))
}

/// The connector a tool or server member URI names, or `None` when `iri` does
/// not carry one.
pub(super) fn member_connector(iri: &str) -> Option<&str> {
    let rest = CONNECTOR_SCHEMES
        .iter()
        .find_map(|scheme| iri.strip_prefix(scheme))?;
    Some(rest.split('/').next().unwrap_or(rest))
}
