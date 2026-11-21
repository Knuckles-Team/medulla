//! MEDULLA-TYPED-PACKS-R110.5: the served `SourceSync` of glossary terms
//! through the real signed dispatch, with the open metadata adapter.
//!
//! The host opens, fills and completes one approved foreign read pinned to the
//! sync plan digest of the connection and class; `SourceSync` takes the rows
//! exactly once, the adapter maps each open metadata record and the engine
//! commits the catalog nodes and the sync state as one batch.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Map, Value};

use medulla_catalog_types::connector_pack::{ConnectorPackSourceApprovalResult, PackEntryKind};
use medulla_catalog_types::foreign_read::{
    ForeignReadChunkRequest, ForeignReadChunkResult, ForeignReadCompleteRequest,
    ForeignReadCompleteResult, ForeignReadExecutionRef, ForeignReadLimits, ForeignReadOpenRequest,
    ForeignReadOpenResult, ForeignReadRow, FOREIGN_READ_SCHEMA_VERSION,
};
use medulla_catalog_types::open_metadata::{EXTERNAL_GUID_PROPERTY, EXTERNAL_VERSION_PROPERTY};
use medulla_graph_core::graph::{GraphSchemaSource, GraphSchemaSources, SchemaSourceOrigin};
use medulla_protocol_types::protocol::{GraphType, Method};
use medulla_source_types::source_sync::SyncReport;
use medulla_virtual_graph::source_sync::sync_plan_digest;

use super::{
    approve_source, bind, build_pack, head_of, imported, ok, tool, Content, Served, ADMIN, TENANT,
};

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

fn unique(label: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("r110:{label}:{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// One served engine, an approved pack source and an empty durable graph with
/// the authority rows installed.
struct Harness {
    served: Served,
    _env: medulla_kernel::crypto::TestEnvReadGuard,
    approved: ConnectorPackSourceApprovalResult,
    source_name: String,
}

impl Harness {
    async fn new() -> Self {
        let served = Served::new();
        let env = served.with_graph_persistence().await;
        bind(&served, CONNECTOR, ADMIN).await;
        let pack = build_pack(
            CONNECTOR,
            &[
                tool(CONNECTOR, "a", "A."),
                Content {
                    kind: PackEntryKind::Resource,
                    uri: format!("contract://{CONNECTOR}"),
                    name: "source-contract".to_string(),
                    media_type: "text/turtle",
                    body: format!(
                        r#"@prefix ac: <https://knuckles-team.github.io/ontology/access#> .
<data://{CONNECTOR}/terms#access> a ac:AccessContract ;
 ac:classIri "urn:GlossaryTerm" ; ac:sourceId "{CONNECTOR}/terms" ;
 ac:entity "terms" ; ac:keyField "guid" ; ac:accessKind "mcp_tool" ;
 ac:operation "export_terms" ; ac:pagination "none" ."#
                    )
                    .into_bytes(),
                    input_schema: None,
                    annotations: Default::default(),
                    references: Vec::new(),
                },
            ],
        );
        let first = imported(&served, &pack, None).await;
        let connector_id = medulla_types::contract::ResourceId::new(CONNECTOR).unwrap();
        let store = {
            use medulla_persistence_records::agent_library::AgentLibraryStateExt;
            served.state.write().await.ensure_agent_library().unwrap()
        };
        let member = store
            .connector_pack_members(TENANT, &connector_id)
            .unwrap()
            .into_iter()
            .find(|member| !member.source_registrations.is_empty())
            .expect("source registration");
        let registration = member.source_registrations[0].clone();
        let approved: ConnectorPackSourceApprovalResult = ok(
            "approve the sync source",
            approve_source(
                &served,
                CONNECTOR,
                &registration.contract_iri,
                &registration.resource_uri,
                head_of(&first),
                member.body_sha256,
            )
            .await,
        );
        let source_name = registration.source_id.clone();
        let registered: String = ok(
            "register the foreign source",
            served
                .call(
                    &unique("register"),
                    Method::RegisterForeignSource {
                        name: source_name.clone(),
                        source: medulla_query_types::wire::ForeignSourceSpec::Mcp {
                            server: CONNECTOR.to_string(),
                            tool: "export_terms".to_string(),
                            capabilities: Default::default(),
                        },
                        quota: None,
                        acceleration: None,
                    },
                )
                .await,
        );
        assert_eq!(registered, source_name);

        let created = served
            .call_on(
                GRAPH,
                &unique("create"),
                Method::CreateGraph {
                    graph_name: GRAPH.to_string(),
                    graph_type: GraphType::Global,
                },
            )
            .await;
        assert!(created.error.is_none(), "{created:?}");
        {
            let state = served.state.read().await;
            let core = state.registry.get(GRAPH).expect("resident").core.clone();
            let mut sources = GraphSchemaSources::with_dynamic(BTreeMap::new()).unwrap();
            let source = GraphSchemaSource::new(
                SchemaSourceOrigin::Operator,
                None,
                Some(Arc::from(authority_turtle().as_str())),
                0,
            )
            .expect("a bounded operator source");
            sources
                .attach_dynamic("operator".to_string(), source)
                .expect("the operator source attaches");
            core.install_schema_sources(Arc::new(sources));
        }
        Self {
            served,
            _env: env,
            approved,
            source_name,
        }
    }

    /// Open, fill and complete one approved read pinned to the sync plan
    /// digest of the connection and class.
    async fn read(&self, records: &[Value]) -> ForeignReadExecutionRef {
        let digest = sync_plan_digest(CONNECTION, &term_class());
        let opened: ForeignReadOpenResult = ok(
            "open the sync read",
            self.served
                .call(
                    &unique("open"),
                    Method::ForeignReadOpen {
                        request: ForeignReadOpenRequest {
                            schema_version: FOREIGN_READ_SCHEMA_VERSION,
                            source_name: self.source_name.clone(),
                            approval: self.approved.clone(),
                            plan_digest: digest,
                            query_digest: digest,
                            limits: ForeignReadLimits {
                                max_rows: 1000,
                                max_bytes: 4 * 1024 * 1024,
                                chunk_rows: 1000,
                                chunk_bytes: 1024 * 1024,
                                deadline_ms: 60_000,
                            },
                            submission: None,
                        },
                    },
                )
                .await,
        );
        let rows: Vec<ForeignReadRow> = records
            .iter()
            .map(|record| ForeignReadRow {
                id: record["guid"].as_str().unwrap().to_string(),
                score: None,
                columns: record
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
                provenance: vec![format!("fixture:{}", record["guid"])],
            })
            .collect();
        let chunked: ForeignReadChunkResult = ok(
            "fill the sync read",
            self.served
                .call(
                    &unique("chunk"),
                    Method::ForeignReadChunk {
                        request: ForeignReadChunkRequest {
                            execution: opened.execution,
                            sequence: 0,
                            source_identity: opened.source_identity.clone(),
                            rows,
                        },
                    },
                )
                .await,
        );
        assert_eq!(chunked.accepted_rows as usize, records.len());
        let _: ForeignReadCompleteResult = ok(
            "complete the sync read",
            self.served
                .call(
                    &unique("complete"),
                    Method::ForeignReadComplete {
                        request: ForeignReadCompleteRequest {
                            execution: opened.execution,
                            sequence: 1,
                        },
                    },
                )
                .await,
        );
        opened.execution
    }

    async fn sync(&self, records: &[Value]) -> SyncReport {
        let execution = self.read(records).await;
        ok(
            "SourceSync",
            self.served
                .call_on(
                    GRAPH,
                    &unique("sync"),
                    Method::SourceSync {
                        connection_iri: CONNECTION.to_string(),
                        class_iri: term_class(),
                        foreign_execution: execution,
                        scope: None,
                    },
                )
                .await,
        )
    }

    async fn nodes(&self) -> BTreeMap<String, Map<String, Value>> {
        let state = self.served.state.read().await;
        let core = state.registry.get(GRAPH).unwrap().core.clone();
        core.get_nodes()
            .into_iter()
            .filter_map(
                |(id, bytes)| match medulla_types::msgpack::decode_property_value(&bytes) {
                    Ok(Value::Object(map)) => Some((id, map)),
                    _ => None,
                },
            )
            .collect()
    }
}

// spec: MEDULLA-TYPED-PACKS-R110.5
#[tokio::test]
async fn sync_reads_terms_and_writes_catalog_nodes() {
    let h = Harness::new().await;
    let records = terms();

    // The first sync applies both terms inbound and proposes nothing.
    let report = h.sync(&records).await;
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
    let again = h.sync(&records).await;
    assert_eq!((again.unchanged, again.applied, again.proposed), (2, 0, 0));

    // A changed term applies inbound and moves the stored version.
    let mut changed = records.clone();
    changed[1]["properties"]["displayName"] = json!("Purchase order");
    changed[1]["version"] = json!(2);
    let third = h.sync(&changed).await;
    assert_eq!((third.unchanged, third.applied, third.proposed), (1, 1, 0));
    let nodes = h.nodes().await;
    assert_eq!(nodes["term-2"]["displayName"], "Purchase order");
    assert_eq!(nodes["term-2"][EXTERNAL_VERSION_PROPERTY], 2);
}

// spec: MEDULLA-TYPED-PACKS-R110.6
#[tokio::test]
async fn unmapped_types_are_listed_and_skipped() {
    let h = Harness::new().await;
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

    let report = h.sync(&records).await;

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
        .map(|skip| {
            (
                skip.type_name.as_str(),
                skip.first_key.as_str(),
                skip.count,
            )
        })
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
