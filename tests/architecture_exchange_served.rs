//! MEDULLA-CQ-ORCH-R705.6: the served ArchiMate exchange through the real
//! `dispatch` shell over a redb-backed `ServerState`.
//!
//! An import commits its nodes and edges as one batch; the export reads them
//! back through the caller's graph view and names the document digest and the
//! canonical model digest.

#![cfg(all(feature = "redb", feature = "rdf"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_core::architecture_exchange::read::parse_exchange;
use medulla_core::process_exchange::xml_tree::XmlLimits;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::archimate_exchange::{
    sha256_hex, ArchitectureModelExported, ArchitectureModelImported,
};

const SECRET: &str = "architecture-exchange-served-secret";
const ENCRYPTION_KEY: &str = "architecture-exchange-served-key-0123456789";
const GRAPH: &str = "architecture";

const DOC: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://www.opengroup.org/xsd/archimate/3.0/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" identifier="id-model">
  <name xml:lang="en">Estate</name>
  <elements>
    <element identifier="id-proc" xsi:type="BusinessProcess"><name xml:lang="en">Order to cash</name></element>
    <element identifier="id-app" xsi:type="ApplicationComponent"><name xml:lang="en">Billing</name></element>
    <element identifier="id-cap" xsi:type="Capability"><name xml:lang="en">Invoicing</name></element>
  </elements>
  <relationships>
    <relationship identifier="id-r1" source="id-app" target="id-proc" xsi:type="Serving"/>
  </relationships>
  <views><diagrams><view identifier="id-view" xsi:type="Diagram"><name xml:lang="en">Main</name>
    <node identifier="n1" elementRef="id-proc" xsi:type="Element" x="10" y="20" w="120" h="55"/>
    <node identifier="n2" elementRef="id-app" xsi:type="Element" x="200" y="20" w="120" h="55"/>
    <connection identifier="c1" relationshipRef="id-r1" xsi:type="Relationship" source="n2" target="n1"/>
  </view></diagrams></views>
</model>"##;

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

fn payload<T: serde::de::DeserializeOwned>(response: Response) -> T {
    assert!(response.error.is_none(), "refused: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed answer"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed answer"),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn core_version(state: &test_support::SharedState) -> u64 {
    state
        .try_read()
        .unwrap()
        .registry
        .get(GRAPH)
        .unwrap()
        .core
        .version()
}

// spec: MEDULLA-CQ-ORCH-R705.6
#[tokio::test(flavor = "multi_thread")]
async fn import_writes_one_batch_and_export_reads_it() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "architecture-exchange-served",
        common::current_isolation(),
        GRAPH,
    )
    .await;

    let before = core_version(&state);
    let imported: ArchitectureModelImported = payload(
        send(
            &state,
            2,
            Method::ArchitectureModelImport {
                document: DOC.to_string(),
                expected_version: Some(1),
            },
        )
        .await,
    );
    assert_eq!(
        core_version(&state),
        before + 1,
        "one import commits one batch"
    );
    assert_eq!(imported.document_digest, sha256_hex(DOC.as_bytes()));
    // Three elements, one reified relationship and one view; one direct edge.
    assert_eq!((imported.nodes, imported.edges), (5, 1));

    let exported: ArchitectureModelExported =
        payload(send(&state, 3, Method::ArchitectureModelExport { scope: None }).await);
    assert_eq!(
        core_version(&state),
        before + 1,
        "an export commits nothing"
    );
    assert_eq!(
        exported.document_digest,
        sha256_hex(exported.document.as_bytes())
    );
    assert_eq!(exported.model_digest.len(), 64);
    assert_eq!(
        exported.model_digest, imported.model_digest,
        "the export of an import has the imported canonical model"
    );

    let back = parse_exchange(exported.document.as_bytes(), XmlLimits::default()).unwrap();
    let identifiers: Vec<&str> = back
        .elements
        .iter()
        .map(|e| e.identifier.as_str())
        .collect();
    assert_eq!(identifiers, vec!["id-app", "id-cap", "id-proc"]);
    assert_eq!(back.relationships.len(), 1);
    assert_eq!(back.relationships[0].identifier, "id-r1");
    assert_eq!(back.views.len(), 1);

    // A repeated import is idempotent: the canonical model does not change.
    let again: ArchitectureModelImported = payload(
        send(
            &state,
            4,
            Method::ArchitectureModelImport {
                document: DOC.to_string(),
                expected_version: None,
            },
        )
        .await,
    );
    assert_eq!(again.model_digest, imported.model_digest);
    let second: ArchitectureModelExported =
        payload(send(&state, 5, Method::ArchitectureModelExport { scope: None }).await);
    assert_eq!(second.model_digest, exported.model_digest);
}
