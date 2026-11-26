//! MEDULLA-FEDERATED-QUERY-R740.1: a graph write is parsed by its exact media type, and a
//! refusal names its code and leaves the store unchanged. The parse runs before any update
//! is planned, so a refusal is an `Err` of the one update builder: nothing reaches a store.
// spec: MEDULLA-FEDERATED-QUERY-R740.1
#![cfg(feature = "sparql-http")]

use medulla_server_blob::sparql_http::{
    parse_rdf_body, rdf_graph_write_event_update, rdf_graph_write_update, RdfBodyRefusal,
    RdfWriteMode, RDF_BODY_MAX_TRIPLES,
};

const GRAPH: &str = "http://ex/g";

/// One triple in every media type: `<http://ex/a> <http://ex/p> "x"`.
const BODIES: &[(&str, &str)] = &[
    (
        "application/n-triples",
        "<http://ex/a> <http://ex/p> \"x\" .\n",
    ),
    (
        "text/turtle",
        "@prefix ex: <http://ex/> .\nex:a ex:p \"x\" .\n",
    ),
    (
        "application/n-quads",
        "<http://ex/a> <http://ex/p> \"x\" .\n",
    ),
    (
        "application/trig",
        "@prefix ex: <http://ex/> .\n<http://ex/g> { ex:a ex:p \"x\" . }\n",
    ),
    (
        "application/ld+json",
        "{\"@id\": \"http://ex/a\", \"http://ex/p\": \"x\"}",
    ),
];

#[cfg(feature = "rdf-xml")]
const RDF_XML: &str = "<?xml version=\"1.0\"?>\
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
<rdf:Description rdf:about=\"http://ex/a\"><p xmlns=\"http://ex/\">x</p></rdf:Description>\
</rdf:RDF>";

fn key(triples: &[medulla_rdf::oxrdf::Triple]) -> Vec<String> {
    let mut keys: Vec<String> = triples.iter().map(ToString::to_string).collect();
    keys.sort();
    keys
}

#[test]
fn each_media_type_parses_and_each_refusal_leaves_store_unchanged() {
    // The six types parse to the same one triple.
    let expected = key(&parse_rdf_body("application/n-triples", BODIES[0].1, GRAPH).unwrap());
    assert_eq!(expected.len(), 1);
    for (media_type, body) in BODIES {
        let parsed = parse_rdf_body(media_type, body, GRAPH)
            .unwrap_or_else(|refusal| panic!("{media_type}: {}", refusal.message()));
        assert_eq!(key(&parsed), expected, "{media_type}");
    }
    #[cfg(feature = "rdf-xml")]
    assert_eq!(
        key(&parse_rdf_body("application/rdf+xml", RDF_XML, GRAPH).unwrap()),
        expected
    );

    // Parameters and case are removed before the parser is chosen.
    assert!(parse_rdf_body("Text/Turtle; charset=utf-8", BODIES[1].1, GRAPH).is_ok());

    // RDF_MEDIA_TYPE_UNSUPPORTED (415): a type outside the six, and no type at all.
    for media_type in ["text/plain", "application/json", ""] {
        let refusal = parse_rdf_body(media_type, BODIES[0].1, GRAPH).unwrap_err();
        assert_eq!(refusal.code(), "RDF_MEDIA_TYPE_UNSUPPORTED", "{media_type:?}");
        assert!(refusal.http_status().starts_with("415"));
    }

    // RDF_PARSE_FAILED (400).
    let refusal = parse_rdf_body("text/turtle", "ex:a ex:p", GRAPH).unwrap_err();
    assert_eq!(refusal.code(), "RDF_PARSE_FAILED");
    assert!(refusal.http_status().starts_with("400"));

    // RDF_DATASET_GRAPH_MISMATCH (422): a quad of another graph, in both quad syntaxes and
    // JSON-LD; the target graph and the default graph are admitted.
    let other = "<http://ex/a> <http://ex/p> \"x\" <http://ex/other> .\n";
    let refusal = parse_rdf_body("application/n-quads", other, GRAPH).unwrap_err();
    assert_eq!(refusal, RdfBodyRefusal::DatasetGraphMismatch);
    assert_eq!(refusal.code(), "RDF_DATASET_GRAPH_MISMATCH");
    assert!(refusal.http_status().starts_with("422"));
    let trig = "<http://ex/other> { <http://ex/a> <http://ex/p> \"x\" . }";
    assert_eq!(
        parse_rdf_body("application/trig", trig, GRAPH).unwrap_err().code(),
        "RDF_DATASET_GRAPH_MISMATCH"
    );
    let jsonld = "{\"@id\": \"http://ex/other\", \"@graph\": [{\"@id\": \"http://ex/a\", \
                  \"http://ex/p\": \"x\"}]}";
    assert_eq!(
        parse_rdf_body("application/ld+json", jsonld, GRAPH).unwrap_err().code(),
        "RDF_DATASET_GRAPH_MISMATCH"
    );
    let named = "<http://ex/a> <http://ex/p> \"x\" <http://ex/g> .\n";
    assert!(parse_rdf_body("application/n-quads", named, GRAPH).is_ok());

    // RDF_BODY_TOO_LARGE (413): one triple over the limit.
    let big: String = (0..=RDF_BODY_MAX_TRIPLES)
        .map(|i| format!("<http://e/s{i}> <http://e/p> <http://e/o> .\n"))
        .collect();
    let refusal = parse_rdf_body("application/n-triples", &big, GRAPH).unwrap_err();
    assert_eq!(refusal, RdfBodyRefusal::BodyTooLarge);
    assert_eq!(refusal.code(), "RDF_BODY_TOO_LARGE");
    assert!(refusal.http_status().starts_with("413"));

    // The refusal text names no triple of the body.
    assert!(!parse_rdf_body("application/n-quads", other, GRAPH)
        .unwrap_err()
        .message()
        .contains("ex/other"));

    // A refused body builds no update: the store has nothing to change.
    assert!(rdf_graph_write_update("text/plain", "x", GRAPH, RdfWriteMode::Replace).is_err());
    let replace = rdf_graph_write_update("text/turtle", BODIES[1].1, GRAPH, RdfWriteMode::Replace)
        .unwrap();
    assert!(replace.starts_with("CLEAR DEFAULT; INSERT DATA {"), "{replace}");
    let merge = rdf_graph_write_update("text/turtle", BODIES[1].1, GRAPH, RdfWriteMode::Merge)
        .unwrap();
    assert!(merge.starts_with("INSERT DATA {"), "{merge}");

    // The served event carries the same body in one JSON envelope.
    let envelope = serde_json::json!({
        "media_type": "text/turtle", "mode": "replace", "body": BODIES[1].1,
    })
    .to_string();
    assert_eq!(rdf_graph_write_event_update(&envelope, GRAPH).unwrap(), replace);
    for bad in [
        "not json".to_string(),
        serde_json::json!({"media_type": "text/turtle", "mode": "append", "body": ""})
            .to_string(),
        serde_json::json!({"media_type": "text/turtle", "mode": "merge"}).to_string(),
    ] {
        let refusal = rdf_graph_write_event_update(&bad, GRAPH).unwrap_err();
        assert_eq!(refusal.code(), "RDF_PARSE_FAILED", "{bad}");
    }
}
