//! Served acceptance of the connector surface projection
//! (MEDULLA-TYPED-PACKS-R102.2, R102.4, R102.6): the outbox consumer writes one
//! node per tool of the pack head into the pack graph, stamped with the
//! component id, entry revision, pack digest and lifecycle.

use std::sync::atomic::AtomicBool;

use medulla_catalog_types::connector_pack::{ConnectorPackIndex, PackAnnotations, PackEntryKind};
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};
use medulla_types::contract::ResourceId;

use super::{bind, build_pack, head_of, imported, tool, Content, Served, ADMIN, TENANT};
use crate::connector_pack_projection::worker::sweep;

const CP: &str = "http://knuckles.team/kg/connector-pack#";

type Pack = (ConnectorPackIndex, Vec<u8>);

/// The ontology of a pack whose `tools` all operate on one domain class.
fn surface_ontology(connector: &str, tools: &[&str]) -> Content {
    let mut body = String::from(
        "@prefix cp: <http://knuckles.team/kg/connector-pack#> .\n\
         @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         <https://example.org/Order> a owl:Class ;\n\
         rdfs:subClassOf <http://knuckles.team/kg#Agreement> .\n",
    );
    for name in tools {
        body.push_str(&format!(
            "<tool://{connector}/{name}> cp:operatesOn <https://example.org/Order> .\n"
        ));
    }
    Content {
        kind: PackEntryKind::Ontology,
        uri: format!("ontology://{connector}/surface.ttl"),
        name: "surface.ttl".to_string(),
        media_type: "text/turtle",
        body: body.into_bytes(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    }
}

fn pack(connector: &str, tools: &[&str]) -> Pack {
    let mut entries: Vec<Content> = tools
        .iter()
        .map(|name| tool(connector, name, "A tool."))
        .collect();
    entries.push(surface_ontology(connector, tools));
    build_pack(connector, &entries)
}

/// Run the projection worker until the outbox is drained.
async fn project(served: &Served) {
    let subscribed = AtomicBool::new(false);
    while sweep(&served.state, &subscribed).await.unwrap() {}
}

/// Every triple of the connector's pack graph; empty before it exists.
async fn pack_triples(served: &Served, connector: &str) -> Vec<Triple> {
    let graph = crate::graph_schema::pack_projection_graph_name(
        TENANT,
        &ResourceId::new(connector).unwrap(),
    )
    .unwrap();
    let state = served.state.read().await;
    match state.registry.get(&graph) {
        Some(entry) => medulla_rdf::mapping::export_triples(&entry.core, &graph).unwrap(),
        None => Vec::new(),
    }
}

/// The values of `predicate` on the node `subject`, sorted.
fn values(triples: &[Triple], subject: &str, predicate: &str) -> Vec<String> {
    let mut found: Vec<String> = triples
        .iter()
        .filter(|triple| {
            matches!(&triple.subject, NamedOrBlankNode::NamedNode(node) if node.as_str() == subject)
                && triple.predicate.as_str() == predicate
        })
        .map(|triple| match &triple.object {
            Term::Literal(literal) => literal.value().to_string(),
            Term::NamedNode(node) => node.as_str().to_string(),
            other => other.to_string(),
        })
        .collect();
    found.sort();
    found
}

fn cp(local: &str) -> String {
    format!("{CP}{local}")
}

fn tool_node(connector: &str, name: &str) -> String {
    format!("tool://{connector}/{name}")
}

// spec: MEDULLA-TYPED-PACKS-R102.2
#[tokio::test]
async fn surface_nodes_follow_pack_head() {
    let connector = "r102-follow";
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    bind(&served, connector, ADMIN).await;
    let first = imported(&served, &pack(connector, &["a", "b"]), None).await;
    project(&served).await;
    let triples = pack_triples(&served, connector).await;
    for name in ["a", "b"] {
        let node = tool_node(connector, name);
        assert_eq!(values(&triples, &node, &cp("lifecycle")), ["active"]);
        assert_eq!(
            values(&triples, &node, &cp("componentId")),
            [format!("mcp:{connector}/tool/{name}")]
        );
        assert_eq!(
            values(&triples, &node, &cp("operatesOn")),
            ["https://example.org/Order"]
        );
    }

    // The next head withdraws tool b: its node stays, marked withdrawn, with
    // exactly one lifecycle value (the old one is replaced, not merged).
    let second = imported(&served, &pack(connector, &["a"]), Some(head_of(&first))).await;
    project(&served).await;
    let triples = pack_triples(&served, connector).await;
    let withdrawn = tool_node(connector, "b");
    assert_eq!(
        values(&triples, &withdrawn, &cp("lifecycle")),
        ["withdrawn"]
    );
    assert!(values(&triples, &withdrawn, &cp("operatesOn")).is_empty());
    assert_eq!(
        values(&triples, &tool_node(connector, "a"), &cp("lifecycle")),
        ["active"]
    );

    // A later import restores it.
    imported(
        &served,
        &pack(connector, &["a", "b"]),
        Some(head_of(&second)),
    )
    .await;
    project(&served).await;
    let triples = pack_triples(&served, connector).await;
    assert_eq!(values(&triples, &withdrawn, &cp("lifecycle")), ["active"]);
}

// spec: MEDULLA-TYPED-PACKS-R102.4
#[tokio::test]
async fn outbox_consumer_writes_surface_nodes() {
    let connector = "r102-outbox";
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    bind(&served, connector, ADMIN).await;
    imported(&served, &pack(connector, &["a"]), None).await;
    let node = tool_node(connector, "a");
    assert!(
        values(
            &pack_triples(&served, connector).await,
            &node,
            &cp("componentId")
        )
        .is_empty(),
        "nothing is projected before the consumer runs"
    );
    let subscribed = AtomicBool::new(false);
    assert!(sweep(&served.state, &subscribed).await.unwrap());
    let written = pack_triples(&served, connector).await;
    assert_eq!(values(&written, &node, &cp("componentId")).len(), 1);
    assert!(!sweep(&served.state, &subscribed).await.unwrap());
    assert_eq!(
        pack_triples(&served, connector).await.len(),
        written.len(),
        "a second sweep writes nothing"
    );
}

// spec: MEDULLA-TYPED-PACKS-R102.6
#[tokio::test]
async fn surface_nodes_carry_pack_digest() {
    let connector = "r102-digest";
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    bind(&served, connector, ADMIN).await;
    let receipt = imported(&served, &pack(connector, &["a", "b"]), None).await;
    project(&served).await;
    let triples = pack_triples(&served, connector).await;
    let digest = receipt.pack_digest.to_hex();
    assert_eq!(digest.len(), 64);
    assert!(digest
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    for name in ["a", "b"] {
        let node = tool_node(connector, name);
        assert_eq!(
            values(&triples, &node, &cp("packDigest")),
            std::slice::from_ref(&digest)
        );
        let revision = values(&triples, &node, &cp("entryRevision"));
        assert_eq!(revision.len(), 1, "{name}");
        assert!(revision[0].parse::<u64>().unwrap() >= 1, "{name}");
    }
}
