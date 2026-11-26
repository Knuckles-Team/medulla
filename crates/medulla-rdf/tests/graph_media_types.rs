//! MEDULLA-FEDERATED-QUERY-R740.2: a graph serializes in each of the six media types, and
//! the serialization parses back to the same triple set. The same `serialize_graph` builds
//! the `graph_body` of a served `Sparql` read and the graph-store route's answer.
// spec: MEDULLA-FEDERATED-QUERY-R740.2
#![cfg(feature = "rdf")]

use medulla_rdf::graph_media::{graph_media_type, serialize_graph, GRAPH_MEDIA_TYPES};
use medulla_rdf::mapping::{
    parse_nquads, parse_ntriples, parse_trig, parse_turtle, triple_set_key,
};
use medulla_rdf::oxrdf::Triple;

/// A graph with an IRI object, a plain literal, a language-tagged literal, a typed literal
/// and a blank-node-free structure, so every serializer has each term kind to carry.
const GRAPH: &str = "\
<http://ex/a> <http://ex/knows> <http://ex/b> .
<http://ex/a> <http://ex/name> \"Alice\" .
<http://ex/a> <http://ex/label> \"Alicia\"@es .
<http://ex/b> <http://ex/age> \"41\"^^<http://www.w3.org/2001/XMLSchema#integer> .
";

fn parse_back(media_type: &str, body: &str) -> Vec<Triple> {
    match media_type {
        "application/n-triples" => parse_ntriples(body).unwrap(),
        "text/turtle" => parse_turtle(body).unwrap(),
        "application/n-quads" => quads(parse_nquads(body).unwrap()),
        "application/trig" => quads(parse_trig(body).unwrap()),
        "application/ld+json" => quads(medulla_rdf::jsonld::from_jsonld(body).unwrap()),
        #[cfg(feature = "rdf-xml")]
        "application/rdf+xml" => medulla_rdf::mapping::parse_rdfxml(body).unwrap(),
        other => panic!("no parser for {other}"),
    }
}

fn quads(quads: Vec<medulla_rdf::oxrdf::Quad>) -> Vec<Triple> {
    quads
        .into_iter()
        .map(|quad| Triple::new(quad.subject, quad.predicate, quad.object))
        .collect()
}

#[test]
fn construct_result_serializes_in_each_type() {
    let triples = parse_ntriples(GRAPH).unwrap();
    let expected = triple_set_key(&triples);
    assert_eq!(expected.len(), 4);

    // The list is the six types, N-Triples first; RDF/XML joins with its feature.
    assert_eq!(GRAPH_MEDIA_TYPES[0], "application/n-triples");
    assert_eq!(
        GRAPH_MEDIA_TYPES.len(),
        if cfg!(feature = "rdf-xml") { 6 } else { 5 }
    );

    for media_type in GRAPH_MEDIA_TYPES {
        let body = serialize_graph(media_type, &triples).unwrap();
        let back = parse_back(media_type, &body);
        assert_eq!(triple_set_key(&back), expected, "{media_type}");
    }
}

#[test]
fn only_the_six_types_name_a_graph_media_type() {
    for media_type in GRAPH_MEDIA_TYPES {
        assert_eq!(graph_media_type(media_type), Some(*media_type));
    }
    // Parameters and case do not change the type.
    assert_eq!(
        graph_media_type("Text/Turtle; charset=utf-8"),
        Some("text/turtle")
    );
    for other in ["text/plain", "application/json", "", "turtle", "ttl"] {
        assert_eq!(graph_media_type(other), None, "{other:?}");
    }
}
