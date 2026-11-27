//! The six RDF media types the engine reads and writes for one graph
//! (MEDULLA-FEDERATED-QUERY-R740): N-Triples, Turtle, N-Quads, TriG, JSON-LD and, with the
//! `rdf-xml` feature, RDF/XML.
//!
//! One list and one serializer serve the graph-store route's content negotiation and the
//! `graph_media_type` of a served `Sparql` read, so the two never disagree on what the six
//! types are.

use oxrdf::Triple;

/// The graph media types, N-Triples (the default) first.
#[cfg(feature = "rdf-xml")]
pub const GRAPH_MEDIA_TYPES: &[&str] = &[
    "application/n-triples",
    "text/turtle",
    "application/n-quads",
    "application/trig",
    "application/ld+json",
    "application/rdf+xml",
];

/// The graph media types, N-Triples (the default) first.
#[cfg(not(feature = "rdf-xml"))]
pub const GRAPH_MEDIA_TYPES: &[&str] = &[
    "application/n-triples",
    "text/turtle",
    "application/n-quads",
    "application/trig",
    "application/ld+json",
];

/// The member of [`GRAPH_MEDIA_TYPES`] that `media_type` names, parameters removed and case
/// ignored; `None` when it names none.
pub fn graph_media_type(media_type: &str) -> Option<&'static str> {
    let essence = media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    GRAPH_MEDIA_TYPES
        .iter()
        .copied()
        .find(|known| *known == essence)
}

/// Serialize `triples` in the media type `ct`; a type outside [`GRAPH_MEDIA_TYPES`] is
/// serialized as N-Triples.
pub fn serialize_graph(ct: &str, triples: &[Triple]) -> Result<String, String> {
    match ct {
        "text/turtle" => crate::mapping::to_turtle(triples),
        "application/n-quads" => crate::mapping::to_nquads(triples, None),
        "application/trig" => crate::mapping::to_trig(triples, None),
        "application/ld+json" => crate::jsonld::to_jsonld(triples, None, None),
        #[cfg(feature = "rdf-xml")]
        "application/rdf+xml" => crate::mapping::to_rdfxml(triples),
        _ => crate::mapping::to_ntriples(triples),
    }
}
