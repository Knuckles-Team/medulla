//! MEDULLA-SRC-AUTH-R012.5 to R012.8: the served `SourceSync` through the real
//! signed dispatch, over a durable graph that holds an imported ArchiMate model.
//!
//! The host opens, fills and completes one approved foreign read per sync
//! (the same four-phase route as `source_approval.rs`), pinned to the sync plan
//! digest of the connection and class; `SourceSync` takes its rows exactly
//! once, plans the whole sync and commits one batch. Outbound changes come back
//! as drafts that the router queues as write-back proposals.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Map, Value};

use medulla_catalog_types::connector_pack::{ConnectorPackSourceApprovalResult, PackEntryKind};
use medulla_catalog_types::foreign_read::{
    ForeignReadChunkRequest, ForeignReadChunkResult, ForeignReadCompleteRequest,
    ForeignReadCompleteResult, ForeignReadExecutionRef, ForeignReadLimits, ForeignReadOpenRequest,
    ForeignReadOpenResult, ForeignReadRow, FOREIGN_READ_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_decision_types::decision::statistical::writeback::{WritebackOp, WritebackOutcome};
use medulla_graph_core::graph::{GraphSchemaSource, GraphSchemaSources, SchemaSourceOrigin};
use medulla_protocol_types::protocol::{GraphType, Method, Response};
use medulla_source_types::source_sync::SyncReport;
use medulla_types::contract::Digest256;
use medulla_virtual_graph::architecture_sync::ArchitectureSyncAdapter;
use medulla_virtual_graph::source_sync::{
    sync_plan_digest, SyncAdapter, SYNC_CONFLICT_CLASS, SYNC_STATE_CLASS,
};

use super::{
    approve_source, bind, build_pack, head_of, imported, ok, refused, tool, Content, Served, ADMIN,
    TENANT,
};

const CONNECTOR: &str = "r012-sync";
const GRAPH: &str = "architecture-sync";
const MODEL: &str = "id-model";
const CORE: &str = "https://knuckles-team.github.io/ontology/core#";
const CONNECTION: &str = "urn:eg:fixture:sync:archimate-export";
const OTHER_CONNECTION: &str = "urn:eg:fixture:sync:other-export";

/// Both ways, the graph wins a two-sided change.
const APP: &str = "https://knuckles-team.github.io/ontology/core#ApplicationComponent";
/// Both ways, a two-sided change waits for a person.
const PROCESS: &str = "https://knuckles-team.github.io/ontology/core#BusinessProcess";
/// Outbound only.
const CAPABILITY: &str = "https://knuckles-team.github.io/ontology/core#BusinessCapability";
/// Inbound only.
const ACTOR: &str = "https://knuckles-team.github.io/ontology/core#BusinessActor";
/// A secondary row with no system of record behind it.
const SERVICE: &str = "https://knuckles-team.github.io/ontology/core#Service";
/// Rows exist, but no adapter serves the class.
const INCIDENT: &str = "https://knuckles-team.github.io/ontology/core#Incident";

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
</model>"##;

fn authority_turtle() -> String {
    let mut ttl = format!(
        r#"@prefix vg: <https://knuckles-team.github.io/ontology/virtual-graph#> .
@prefix kg: <{CORE}> .
@prefix ex: <urn:eg:fixture:sync:> .
ex:archimate-export a vg:SourceConnection ; vg:sourceKind "mcp" ; vg:declaredCapability "read" .
ex:other-export a vg:SourceConnection ; vg:sourceKind "mcp" ; vg:declaredCapability "read" .
"#
    );
    // (class label, role rows, direction, rule); a system of record row is
    // written for every class but SERVICE.
    let classes = [
        ("ApplicationComponent", "both", "system_of_record_wins"),
        ("BusinessProcess", "both", "hold_for_review"),
        ("BusinessCapability", "outbound", "system_of_record_wins"),
        ("BusinessActor", "inbound", "system_of_record_wins"),
        ("Service", "both", "system_of_record_wins"),
        ("Incident", "both", "system_of_record_wins"),
    ];
    for (label, direction, rule) in classes {
        if label != "Service" {
            ttl.push_str(&format!(
                "ex:{label}-sor a vg:SourceAuthority ; vg:authoritativeSource vg:NativeGraph ; \
                 vg:authorityFor kg:{label} ; vg:authorityRank 1 ; vg:authorityContract \"virtual-graph\" ; \
                 vg:recordRole \"system_of_record\" .\n"
            ));
        }
        ttl.push_str(&format!(
            "ex:{label}-secondary a vg:SourceAuthority ; vg:authoritativeSource ex:archimate-export ; \
             vg:authorityFor kg:{label} ; vg:authorityRank 2 ; vg:authorityContract \"virtual-graph\" ; \
             vg:recordRole \"federated_secondary\" ; vg:syncDirection \"{direction}\" ; vg:conflictRule \"{rule}\" .\n"
        ));
    }
    ttl
}

fn element(id: &str, element_type: &str, name: &str, relationships: Vec<Value>) -> Value {
    json!({
        "identifier": id,
        "element_type": element_type,
        "name": name,
        "properties": {},
        "relationships": relationships,
    })
}

/// The three elements the imported model holds, as the source serves them.
fn model_records() -> Vec<Value> {
    vec![
        element(
            "id-app",
            "ApplicationComponent",
            "Billing",
            vec![json!({
                "identifier": "id-r1",
                "relationship_type": "Serving",
                "source": "id-app",
                "target": "id-proc",
            })],
        ),
        element("id-proc", "BusinessProcess", "Order to cash", vec![]),
        element("id-cap", "Capability", "Invoicing", vec![]),
    ]
}

fn without(records: &[Value], key: &str) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record["identifier"] != key)
        .cloned()
        .collect()
}

fn renamed(records: &[Value], key: &str, name: &str) -> Vec<Value> {
    records
        .iter()
        .cloned()
        .map(|mut record| {
            if record["identifier"] == key {
                record["name"] = json!(name);
            }
            record
        })
        .collect()
}

fn adapter() -> ArchitectureSyncAdapter {
    ArchitectureSyncAdapter {
        model_id: MODEL.to_string(),
    }
}

fn unique(label: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("r012:{label}:{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// One served engine, an approved pack source, and a durable graph holding the
/// imported model with the authority rows installed.
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
<data://{CONNECTOR}/elements#access> a ac:AccessContract ;
 ac:classIri "urn:ArchitectureElement" ; ac:sourceId "{CONNECTOR}/elements" ;
 ac:entity "elements" ; ac:keyField "identifier" ; ac:accessKind "mcp_tool" ;
 ac:operation "export_elements" ; ac:pagination "none" ."#
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
                            tool: "export_elements".to_string(),
                            capabilities: Default::default(),
                        },
                        quota: None,
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
        let harness = Self {
            served,
            _env: env,
            approved,
            source_name,
        };
        let imported: Value = ok(
            "import the model",
            harness
                .served
                .call_on(
                    GRAPH,
                    &unique("import"),
                    Method::ArchitectureModelImport {
                        document: DOC.to_string(),
                        expected_version: None,
                    },
                )
                .await,
        );
        assert!(imported.get("model_digest").is_some(), "{imported}");
        harness
    }

    /// Open, fill and (when `complete`) complete one approved read pinned to
    /// the sync plan digest of `connection` and `class`.
    async fn read(
        &self,
        connection: &str,
        class: &str,
        records: &[Value],
        complete: bool,
    ) -> ForeignReadExecutionRef {
        let digest = sync_plan_digest(connection, class);
        let opened: ForeignReadOpenResult = ok(
            "open the sync read",
            self.served
                .call(
                    &unique("open"),
                    Method::ForeignReadOpen {
                        request: ForeignReadOpenRequest {
                            schema_version: FOREIGN_READ_SCHEMA_VERSION,
                            submission: None,
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
                        },
                    },
                )
                .await,
        );
        let rows: Vec<ForeignReadRow> = records
            .iter()
            .map(|record| ForeignReadRow {
                id: record["identifier"].as_str().unwrap().to_string(),
                score: None,
                columns: record
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
                provenance: vec![format!("fixture:{}", record["identifier"])],
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
        if complete {
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
        }
        opened.execution
    }

    async fn sync_execution(
        &self,
        connection: &str,
        class: &str,
        execution: ForeignReadExecutionRef,
    ) -> Response {
        self.served
            .call_on(
                GRAPH,
                &unique("sync"),
                Method::SourceSync {
                    connection_iri: connection.to_string(),
                    class_iri: class.to_string(),
                    foreign_execution: execution,
                    scope: Some(MODEL.to_string()),
                },
            )
            .await
    }

    async fn sync_response(&self, connection: &str, class: &str, records: &[Value]) -> Response {
        let execution = self.read(connection, class, records, true).await;
        self.sync_execution(connection, class, execution).await
    }

    async fn sync(&self, class: &str, records: &[Value]) -> SyncReport {
        ok(
            "SourceSync",
            self.sync_response(CONNECTION, class, records).await,
        )
    }

    async fn version(&self) -> u64 {
        self.served
            .state
            .read()
            .await
            .registry
            .get(GRAPH)
            .unwrap()
            .core
            .version()
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

    async fn nodes_of(&self, class: &str) -> Vec<Map<String, Value>> {
        self.nodes()
            .await
            .into_values()
            .filter(|props| props.get("type").and_then(Value::as_str) == Some(class))
            .collect()
    }

    async fn writeback(&self, op: WritebackOp) -> WritebackOutcome {
        ok(
            "Writeback",
            self.served
                .call(
                    &unique("writeback"),
                    Method::DecisionLog {
                        op: Box::new(DecisionLogOp::Writeback {
                            tenant_id: TENANT.to_string(),
                            computation: op,
                        }),
                    },
                )
                .await,
        )
    }
}

// spec: MEDULLA-SRC-AUTH-R012.5
#[tokio::test]
async fn outbound_change_queues_one_proposal() {
    let h = Harness::new().await;
    // The source no longer lists id-cap: the graph holds a record the source
    // lacks, so the change goes outbound.
    let records = without(&model_records(), "id-cap");
    let report = h.sync(APP, &records).await;
    assert_eq!(
        (report.unchanged, report.applied, report.proposed),
        (2, 0, 1)
    );
    assert!(report.conflicts.is_empty());
    assert_eq!(report.outbound.len(), 1);
    let draft = &report.outbound[0];
    assert_eq!(draft.record_key, "id-cap");
    let graph_record = model_records().remove(2);
    let payload = adapter().outbound_payload("id-cap", &graph_record);
    assert_eq!(
        draft.payload_digest,
        Digest256::sha256(payload.to_string().as_bytes()).to_hex(),
        "the draft carries the digest of the outbound payload"
    );

    // One proposal, queued under the queue id the report names.
    let queue_id = report.queue_id.clone().expect("the router names the queue");
    assert_eq!(queue_id.len(), 64);
    assert_eq!(
        report.queued,
        vec![format!("sync:{}:id-cap", &queue_id[..16])]
    );

    // A re-sync of the unchanged graph proposes the same queue: no duplicate.
    let again = h.sync(APP, &records).await;
    assert_eq!(again.queue_id.as_deref(), Some(queue_id.as_str()));
    assert_eq!(again.queued, report.queued);

    // The queue holds exactly that proposal for the graph as system of record.
    let approved = h
        .writeback(WritebackOp::Approve {
            queue_id: queue_id.clone(),
            proposal_id: report.queued[0].clone(),
        })
        .await;
    assert!(matches!(approved, WritebackOutcome::Approved { .. }));
    let WritebackOutcome::Released { change_set } = h
        .writeback(WritebackOp::Release {
            queue_id: queue_id.clone(),
        })
        .await
    else {
        panic!("the approved queue releases");
    };
    assert_eq!(change_set.proposals.len(), 1);
    let proposal = &change_set.proposals[0];
    assert_eq!(proposal.id, report.queued[0]);
    assert_eq!(proposal.external_id, "id-cap");
    assert_eq!(proposal.system_of_record, GRAPH);
    assert_eq!(change_set.approvals.len(), 1);

    // Sync wrote no inbound change: the model's nodes are as imported.
    assert_eq!(
        h.nodes()
            .await
            .get("archimate:id-model:id-cap")
            .and_then(|props| props.get("name"))
            .and_then(Value::as_str),
        Some("Invoicing")
    );
}

// spec: MEDULLA-SRC-AUTH-R012.6
#[tokio::test]
async fn sync_applies_inbound_and_writes_state_once() {
    let h = Harness::new().await;
    let new_record = element("id-new", "ApplicationComponent", "Ledger", vec![]);
    let mut records = model_records();
    records.push(new_record.clone());

    let before = h.version().await;
    let report = h.sync(APP, &records).await;
    assert_eq!(
        (report.unchanged, report.applied, report.proposed),
        (3, 1, 0)
    );
    assert!(report.queued.is_empty() && report.queue_id.is_none());
    assert_eq!(h.version().await, before + 1, "one sync commits one batch");
    let nodes = h.nodes().await;
    let node = &nodes["archimate:id-model:id-new"];
    assert_eq!(node["name"], json!("Ledger"));
    assert_eq!(node["archimateElementType"], json!("ApplicationComponent"));
    let states = h.nodes_of(SYNC_STATE_CLASS).await;
    assert_eq!(states.len(), 1);
    assert_eq!(states[0]["syncRecordKey"], json!("id-new"));
    assert_eq!(states[0]["syncConnection"], json!(CONNECTION));
    assert_eq!(states[0]["syncClass"], json!(APP));
    assert_eq!(
        states[0]["baseDigest"],
        json!(adapter().record_digest(&new_record)),
        "an applied record's base is the source digest it took"
    );

    // The same source again: nothing changed, nothing is written.
    let version = h.version().await;
    let repeat = h.sync(APP, &records).await;
    assert_eq!(
        (repeat.unchanged, repeat.applied, repeat.proposed),
        (4, 0, 0)
    );
    assert_eq!(
        h.version().await,
        version,
        "an unchanged sync commits nothing"
    );
    assert_eq!(h.nodes_of(SYNC_STATE_CLASS).await.len(), 1);

    // The source renames the record: it is applied, and the state is updated in
    // place by the same batch.
    let edited = renamed(&records, "id-new", "Ledger 2");
    let edited_digest = adapter().record_digest(&edited[3]);
    let second = h.sync(APP, &edited).await;
    assert_eq!(
        (second.unchanged, second.applied, second.proposed),
        (3, 1, 0)
    );
    assert_eq!(h.version().await, version + 1);
    assert_eq!(
        h.nodes().await["archimate:id-model:id-new"]["name"],
        json!("Ledger 2")
    );
    let states = h.nodes_of(SYNC_STATE_CLASS).await;
    assert_eq!(
        states.len(),
        1,
        "the state node is keyed by connection, class and key"
    );
    assert_eq!(states[0]["baseDigest"], json!(edited_digest));
}

// spec: MEDULLA-SRC-AUTH-R012.7
#[tokio::test]
async fn sync_refusals_leave_state_unchanged() {
    let h = Harness::new().await;
    let version = h.version().await;
    let node_ids: BTreeSet<String> = h.nodes().await.into_keys().collect();
    let records = model_records();
    let mut with_new = records.clone();
    with_new.push(element("id-new", "Capability", "New", vec![]));

    let cases = [
        (
            "SYNC_SOURCE_NOT_SECONDARY",
            OTHER_CONNECTION,
            APP,
            records.clone(),
        ),
        (
            "SYNC_NO_SYSTEM_OF_RECORD",
            CONNECTION,
            SERVICE,
            records.clone(),
        ),
        (
            "SYNC_ADAPTER_MISSING",
            CONNECTION,
            INCIDENT,
            records.clone(),
        ),
        // An outbound-only secondary: a source change is refused whole.
        ("SYNC_DIRECTION_REFUSED", CONNECTION, CAPABILITY, with_new),
        // An inbound-only secondary: a graph change is refused whole.
        (
            "SYNC_DIRECTION_REFUSED",
            CONNECTION,
            ACTOR,
            without(&records, "id-cap"),
        ),
    ];
    for (code, connection, class, source) in cases {
        let response = h.sync_response(connection, class, &source).await;
        let refusal = refused(code, response);
        assert!(refusal.starts_with(code), "{code}: {refusal}");
        assert_eq!(h.version().await, version, "{code} wrote to the graph");
        let after: BTreeSet<String> = h.nodes().await.into_keys().collect();
        assert_eq!(after, node_ids, "{code} changed the graph's nodes");
    }

    // A read the host never completed passes its refusal through.
    let open = h.read(CONNECTION, APP, &records, false).await;
    let refusal = refused(
        "an incomplete read",
        h.sync_execution(CONNECTION, APP, open).await,
    );
    assert!(
        refusal.starts_with("FOREIGN_READ_NOT_COMPLETE"),
        "{refusal}"
    );
    assert_eq!(h.version().await, version);
    assert!(h.nodes_of(SYNC_STATE_CLASS).await.is_empty());
}

// spec: MEDULLA-SRC-AUTH-R012.8
#[tokio::test]
async fn report_lists_conflicts_with_both_digests() {
    let h = Harness::new().await;
    let base = model_records();
    let graph_digest = adapter().record_digest(&base[2]);

    // Under system_of_record_wins the graph value stands and goes outbound.
    let edited = renamed(&base, "id-cap", "Invoicing (source)");
    let source_digest = adapter().record_digest(&edited[2]);
    let before = h.version().await;
    let report = h.sync(APP, &edited).await;
    assert_eq!(
        (report.unchanged, report.applied, report.proposed),
        (2, 0, 1)
    );
    assert_eq!(report.conflicts.len(), 1);
    let conflict = &report.conflicts[0];
    assert_eq!(conflict.record_key, "id-cap");
    assert_eq!(
        conflict.graph_digest.as_deref(),
        Some(graph_digest.as_str())
    );
    assert_eq!(
        conflict.source_digest.as_deref(),
        Some(source_digest.as_str())
    );
    assert_eq!(conflict.rule, "system_of_record_wins");
    assert_eq!(report.outbound[0].record_key, "id-cap");
    assert_eq!(report.queued.len(), 1);
    assert_eq!(
        h.version().await,
        before + 1,
        "the conflict node rides the one batch"
    );
    let nodes = h.nodes_of(SYNC_CONFLICT_CLASS).await;
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["syncRecordKey"], json!("id-cap"));
    assert_eq!(nodes[0]["graphDigest"], json!(graph_digest));
    assert_eq!(nodes[0]["sourceDigest"], json!(source_digest));
    assert_eq!(
        nodes[0]["conflictRuleApplied"],
        json!("system_of_record_wins")
    );
    assert_eq!(
        h.nodes().await["archimate:id-model:id-cap"]["name"],
        json!("Invoicing"),
        "the graph value stands"
    );

    // Under hold_for_review the conflict is recorded and nothing is proposed.
    let held = renamed(&base, "id-proc", "Order to cash (source)");
    let held_digest = adapter().record_digest(&held[1]);
    let report = h.sync(PROCESS, &held).await;
    assert_eq!(report.proposed, 0);
    assert!(report.queued.is_empty() && report.queue_id.is_none());
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].rule, "hold_for_review");
    assert_eq!(
        report.conflicts[0].source_digest.as_deref(),
        Some(held_digest.as_str())
    );
    let conflicts = h.nodes_of(SYNC_CONFLICT_CLASS).await;
    assert_eq!(conflicts.len(), 2);
    assert!(conflicts.iter().any(|node| {
        node["syncRecordKey"] == json!("id-proc")
            && node["conflictRuleApplied"] == json!("hold_for_review")
            && node["sourceDigest"] == json!(held_digest)
    }));
}
