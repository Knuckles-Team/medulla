//! MEDULLA-TYPED-PACKS-R110.5: the served `SourceSync` of glossary terms
//! through the real signed dispatch, with the open metadata adapter.
//!
//! The host opens, fills and completes one approved foreign read pinned to the
//! sync plan digest of the connection and class; `SourceSync` takes the rows
//! exactly once, the adapter maps each open metadata record and the engine
//! commits the catalog nodes and the sync state as one batch.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use medulla_catalog_types::open_metadata::{EXTERNAL_GUID_PROPERTY, EXTERNAL_VERSION_PROPERTY};
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_sync::SyncConflictReason;
use medulla_virtual_graph::source_sync::SYNCED_FROM_PROPERTY;

use super::sync_harness::{unique, SyncHarness, SyncSpec};

const CONNECTOR: &str = "r110-sync";
const GRAPH: &str = "open-metadata-sync";
const CATALOG: &str = "https://knuckles-team.github.io/ontology/catalog#";
const CONNECTION: &str = "urn:eg:fixture:sync:glossary-export";

fn term_class() -> String {
    format!("{CATALOG}CatalogGlossaryTerm")
}

fn authority_turtle() -> String {
    format!(
        r#"@prefix vg: <https://knuckles-team.github.io/ontology/virtual-graph#> .
@prefix cat: <{CATALOG}> .
@prefix ex: <urn:eg:fixture:sync:> .
ex:glossary-export a vg:SourceConnection ; vg:sourceKind "mcp" ; vg:declaredCapability "read" .
ex:term-sor a vg:SourceAuthority ; vg:authoritativeSource vg:NativeGraph ;
 vg:authorityFor cat:CatalogGlossaryTerm ; vg:authorityRank 1 ; vg:authorityContract "virtual-graph" ;
 vg:recordRole "system_of_record" .
ex:term-secondary a vg:SourceAuthority ; vg:authoritativeSource ex:glossary-export ;
 vg:authorityFor cat:CatalogGlossaryTerm ; vg:authorityRank 2 ; vg:authorityContract "virtual-graph" ;
 vg:recordRole "federated_secondary" ; vg:syncDirection "both" ; vg:conflictRule "system_of_record_wins" .
"#
    )
}

/// The glossary terms the source serves: one with a mapped classification and
/// an unmapped one (left out of the writes), one plain.
fn terms() -> Vec<Value> {
    vec![
        json!({
            "guid": "term-1",
            "type_name": "GlossaryTerm",
            "version": 3,
            "update_time_ms": 1_700_000_000_000_i64,
            "properties": {"displayName": "Customer"},
            "classifications": [
                {"type_name": "Confidentiality", "properties": {"level": "high"}},
                {"type_name": "NotInTheCatalog", "properties": {"x": 1}}
            ],
            "relationships": [],
        }),
        json!({
            "guid": "term-2",
            "type_name": "GlossaryTerm",
            "version": 1,
            "update_time_ms": 1_700_000_001_000_i64,
            "properties": {"displayName": "Order"},
            "classifications": [],
            "relationships": [],
        }),
    ]
}

// proves R110.7: Open metadata evidence on every synced node
// spec: MEDULLA-TYPED-PACKS-R110.7
#[tokio::test]
async fn nodes_keep_guid_version_and_source() {
    let h = SyncHarness::new(spec()).await;
    let records = terms();

    let first = h.sync(&term_class(), &records).await;
    assert_eq!((first.unchanged, first.applied, first.proposed), (0, 2, 0));
    assert_eq!(
        first.by_type,
        BTreeMap::from([("GlossaryTerm".to_string(), 2)])
    );

    // Every term node holds the GUID, the version and the connection it came from.
    let nodes = h.nodes().await;
    for (guid, version) in [("term-1", 3), ("term-2", 1)] {
        let term = nodes.get(guid).expect("the term node");
        assert_eq!(term[EXTERNAL_GUID_PROPERTY], guid);
        assert_eq!(term[EXTERNAL_VERSION_PROPERTY], version);
        assert_eq!(term[SYNCED_FROM_PROPERTY], CONNECTION);
    }

    // The stamp never enters a graph digest: a second sync of the same records is unchanged.
    let again = h.sync(&term_class(), &records).await;
    assert_eq!((again.unchanged, again.applied, again.proposed), (2, 0, 0));
}

// proves R110.8: Open metadata conflict proof: a store version raised under a catalog edit
// spec: MEDULLA-TYPED-PACKS-R110.8
#[tokio::test]
async fn concurrent_term_edit_is_a_conflict_and_catalog_wins() {
    let h = SyncHarness::new(spec()).await;
    let records = terms();
    let first = h.sync(&term_class(), &records).await;
    assert_eq!(first.applied, 2);

    // A catalog edit of term-1 through the served batch.
    let operations = vec![json!({
        "op": "upsert_node",
        "id": "term-1",
        "properties": {"displayName": "Client"},
    })];
    let edited = h
        .served
        .call_on(
            GRAPH,
            &unique("edit"),
            Method::BatchUpdate {
                operations_msgpack: rmp_serde::to_vec_named(&operations).unwrap(),
            },
        )
        .await;
    assert!(
        edited.refusal_text().is_none(),
        "{:?}",
        edited.refusal_text()
    );

    // The source keeps term-1's properties and raises its version from 3 to 4.
    let mut raised = records.clone();
    raised[0]["version"] = json!(4);
    let report = h.sync(&term_class(), &raised).await;

    assert_eq!(report.conflicts.len(), 1, "{:?}", report.conflicts);
    let conflict = &report.conflicts[0];
    assert_eq!(conflict.record_key, "term-1");
    assert_eq!(conflict.reason, SyncConflictReason::SourceVersionAhead);
    assert_eq!(conflict.rule, "system_of_record_wins");
    assert_eq!(report.proposed, 1);
    assert_eq!(report.outbound.len(), 1);
    assert_eq!(report.outbound[0].record_key, "term-1");

    // The catalog value stays on the node and the stored version is still the old one,
    // so the outbound write (expected version 3) is refused by the store until the next
    // inbound sync.
    let nodes = h.nodes().await;
    assert_eq!(nodes["term-1"]["displayName"], "Client");
    assert_eq!(nodes["term-1"][EXTERNAL_VERSION_PROPERTY], 3);
}

fn spec() -> SyncSpec {
    SyncSpec {
        connector: CONNECTOR,
        graph: GRAPH,
        connection: CONNECTION,
        entity: "terms",
        key_field: "guid",
        contract_class: "urn:GlossaryTerm",
        operation: "export_terms",
        authority: authority_turtle(),
        scope: None,
        row_id: |record| record["guid"].as_str().unwrap().to_string(),
    }
}

// spec: MEDULLA-TYPED-PACKS-R110.5
#[tokio::test]
async fn sync_reads_terms_and_writes_catalog_nodes() {
    let h = SyncHarness::new(spec()).await;
    let records = terms();

    // The first sync applies both terms inbound and proposes nothing.
    let report = h.sync(&term_class(), &records).await;
    assert_eq!(
        (report.unchanged, report.applied, report.proposed),
        (0, 2, 0)
    );
    assert!(report.conflicts.is_empty());

    // Each term is a GlossaryTerm node keyed by its GUID with the sync stamps.
    let nodes = h.nodes().await;
    let term = nodes.get("term-1").expect("the term node");
    assert_eq!(term["type"], "GlossaryTerm");
    assert_eq!(term["displayName"], "Customer");
    assert_eq!(term[EXTERNAL_GUID_PROPERTY], "term-1");
    assert_eq!(term[EXTERNAL_VERSION_PROPERTY], 3);
    assert_eq!(nodes["term-2"][EXTERNAL_VERSION_PROPERTY], 1);

    // The mapped classification is a node of its class; the unmapped one is
    // not written.
    let classification = nodes.get("term-1#Confidentiality").expect("classification");
    assert_eq!(classification["type"], "ConfidentialityClassification");
    assert_eq!(classification["level"], "high");
    assert!(!nodes.contains_key("term-1#NotInTheCatalog"));

    // The same source again is unchanged: the graph side rebuilds the same
    // digest from the nodes it wrote.
    let again = h.sync(&term_class(), &records).await;
    assert_eq!((again.unchanged, again.applied, again.proposed), (2, 0, 0));

    // A changed term applies inbound and moves the stored version.
    let mut changed = records.clone();
    changed[1]["properties"]["displayName"] = json!("Purchase order");
    changed[1]["version"] = json!(2);
    let third = h.sync(&term_class(), &changed).await;
    assert_eq!((third.unchanged, third.applied, third.proposed), (1, 1, 0));
    let nodes = h.nodes().await;
    assert_eq!(nodes["term-2"]["displayName"], "Purchase order");
    assert_eq!(nodes["term-2"][EXTERNAL_VERSION_PROPERTY], 2);
}

// spec: MEDULLA-TYPED-PACKS-R110.6
#[tokio::test]
async fn unmapped_types_are_listed_and_skipped() {
    let h = SyncHarness::new(spec()).await;
    let mut records = terms();
    // A third term with the same unmapped classification, and two records of a
    // type no catalog class carries.
    records.push(json!({
        "guid": "term-3",
        "type_name": "GlossaryTerm",
        "version": 1,
        "update_time_ms": 1_700_000_002_000_i64,
        "properties": {"displayName": "Invoice"},
        "classifications": [{"type_name": "NotInTheCatalog", "properties": {"x": 2}}],
        "relationships": [],
    }));
    for guid in ["other-1", "other-2"] {
        records.push(json!({
            "guid": guid,
            "type_name": "UnmappedAsset",
            "version": 1,
            "update_time_ms": 1_700_000_003_000_i64,
            "properties": {"displayName": guid},
            "classifications": [],
            "relationships": [],
        }));
    }

    let report = h.sync(&term_class(), &records).await;

    // The other terms apply; the records of the other type are neither applied
    // nor counted.
    assert_eq!(
        (report.unchanged, report.applied, report.proposed),
        (0, 3, 0)
    );
    assert!(report.conflicts.is_empty());
    assert_eq!(
        report.by_type,
        BTreeMap::from([("GlossaryTerm".to_string(), 3)])
    );

    // Each unmapped type is listed once, with its first key and its count.
    let listed: Vec<(&str, &str, u64)> = report
        .skipped
        .iter()
        .map(|skip| (skip.type_name.as_str(), skip.first_key.as_str(), skip.count))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("NotInTheCatalog", "term-1", 2),
            ("UnmappedAsset", "other-1", 2),
        ]
    );

    let nodes = h.nodes().await;
    assert!(nodes.contains_key("term-1"));
    assert!(nodes.contains_key("term-3"));
    assert!(!nodes.contains_key("other-1"));
    assert!(!nodes.contains_key("other-2"));
}
