//! The one served `SourceSync` harness of the connector-pack tests.
//!
//! One served engine, one approved pack source and one durable graph with the authority rows
//! installed. The host opens, fills and completes one approved foreign read per sync, pinned to
//! the sync plan digest of the connection and class; `SourceSync` takes its rows exactly once.
//! Every served sync test (architecture, open metadata, process model) builds its engine here.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{Map, Value};

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
use medulla_virtual_graph::source_sync::sync_plan_digest;

use super::{
    approve_source, bind, build_pack, head_of, imported, ok, tool, Content, Served, ADMIN, TENANT,
};

/// What distinguishes one sync fixture from another.
pub(in crate::admin) struct SyncSpec {
    /// The connector name of the pack.
    pub connector: &'static str,
    /// The graph the sync writes.
    pub graph: &'static str,
    /// The connection IRI the authority rows declare.
    pub connection: &'static str,
    /// The contract entity (also the `<entity>` of the source id) and its key field.
    pub entity: &'static str,
    pub key_field: &'static str,
    /// The contract class IRI and the connector operation the source calls.
    pub contract_class: &'static str,
    pub operation: &'static str,
    /// The authority rows installed as the operator schema source.
    pub authority: String,
    /// The `scope` a sync names, when the adapter needs one.
    pub scope: Option<&'static str>,
    /// The row id of one source record.
    pub row_id: fn(&Value) -> String,
}

/// A process-wide unique idempotency key.
pub(in crate::admin) fn unique(label: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("sync:{label}:{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// One served engine, an approved pack source and a durable graph with the authority rows
/// installed.
pub(in crate::admin) struct SyncHarness {
    pub(in crate::admin) served: Served,
    _env: medulla_kernel::crypto::TestEnvReadGuard,
    approved: ConnectorPackSourceApprovalResult,
    source_name: String,
    spec: SyncSpec,
}

impl SyncHarness {
    pub(in crate::admin) async fn new(spec: SyncSpec) -> Self {
        let connector = spec.connector;
        let served = Served::new();
        let env = served.with_graph_persistence().await;
        bind(&served, connector, ADMIN).await;
        let pack = build_pack(
            connector,
            &[
                tool(connector, "a", "A."),
                Content {
                    kind: PackEntryKind::Resource,
                    uri: format!("contract://{connector}"),
                    name: "source-contract".to_string(),
                    media_type: "text/turtle",
                    body: format!(
                        r#"@prefix ac: <https://knuckles-team.github.io/ontology/access#> .
<data://{connector}/{entity}#access> a ac:AccessContract ;
 ac:classIri "{class}" ; ac:sourceId "{connector}/{entity}" ;
 ac:entity "{entity}" ; ac:keyField "{key}" ; ac:accessKind "mcp_tool" ;
 ac:operation "{operation}" ; ac:pagination "none" ."#,
                        entity = spec.entity,
                        class = spec.contract_class,
                        key = spec.key_field,
                        operation = spec.operation,
                    )
                    .into_bytes(),
                    input_schema: None,
                    annotations: Default::default(),
                    references: Vec::new(),
                },
            ],
        );
        let first = imported(&served, &pack, None).await;
        let connector_id = medulla_types::contract::ResourceId::new(connector).unwrap();
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
                connector,
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
                            server: connector.to_string(),
                            tool: spec.operation.to_string(),
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
                spec.graph,
                &unique("create"),
                Method::CreateGraph {
                    graph_name: spec.graph.to_string(),
                    graph_type: GraphType::Global,
                },
            )
            .await;
        assert!(created.error.is_none(), "{created:?}");
        {
            let state = served.state.read().await;
            let core = state
                .registry
                .get(spec.graph)
                .expect("resident")
                .core
                .clone();
            let mut sources = GraphSchemaSources::with_dynamic(BTreeMap::new()).unwrap();
            let source = GraphSchemaSource::new(
                SchemaSourceOrigin::Operator,
                None,
                Some(Arc::from(spec.authority.as_str())),
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
            spec,
        }
    }

    /// The connection IRI of the fixture.
    pub(in crate::admin) fn connection(&self) -> &'static str {
        self.spec.connection
    }

    /// Open, fill and (when `complete`) complete one approved read pinned to the sync plan
    /// digest of `connection` and `class`.
    pub(in crate::admin) async fn read(
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
            .map(|record| {
                let id = (self.spec.row_id)(record);
                ForeignReadRow {
                    provenance: vec![format!("fixture:{id}")],
                    id,
                    score: None,
                    columns: record
                        .as_object()
                        .unwrap()
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect(),
                }
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

    pub(in crate::admin) async fn sync_execution(
        &self,
        connection: &str,
        class: &str,
        execution: ForeignReadExecutionRef,
    ) -> Response {
        self.served
            .call_on(
                self.spec.graph,
                &unique("sync"),
                Method::SourceSync {
                    connection_iri: connection.to_string(),
                    class_iri: class.to_string(),
                    foreign_execution: execution,
                    scope: self.spec.scope.map(str::to_string),
                },
            )
            .await
    }

    pub(in crate::admin) async fn sync_response(
        &self,
        connection: &str,
        class: &str,
        records: &[Value],
    ) -> Response {
        let execution = self.read(connection, class, records, true).await;
        self.sync_execution(connection, class, execution).await
    }

    /// One complete sync of `records` of `class` against the fixture's own connection.
    pub(in crate::admin) async fn sync(&self, class: &str, records: &[Value]) -> SyncReport {
        ok(
            "SourceSync",
            self.sync_response(self.spec.connection, class, records)
                .await,
        )
    }

    pub(in crate::admin) async fn version(&self) -> u64 {
        self.served
            .state
            .read()
            .await
            .registry
            .get(self.spec.graph)
            .unwrap()
            .core
            .version()
    }

    pub(in crate::admin) async fn nodes(&self) -> BTreeMap<String, Map<String, Value>> {
        let state = self.served.state.read().await;
        let core = state.registry.get(self.spec.graph).unwrap().core.clone();
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

    pub(in crate::admin) async fn nodes_of(&self, class: &str) -> Vec<Map<String, Value>> {
        self.nodes()
            .await
            .into_values()
            .filter(|props| props.get("type").and_then(Value::as_str) == Some(class))
            .collect()
    }

    /// Send one write-back op. An approval comes from a second principal: the principal that
    /// queued the change set never approves it (MEDULLA-DECISION-ENGINE-R615.3).
    pub(in crate::admin) async fn writeback(&self, op: WritebackOp) -> WritebackOutcome {
        let approver = matches!(op, WritebackOp::Approve { .. });
        let method = Method::DecisionLog {
            op: Box::new(DecisionLogOp::Writeback {
                tenant_id: TENANT.to_string(),
                computation: op,
            }),
        };
        let key = unique("writeback");
        let response = if approver {
            self.served
                .call_as("pack-served-second-approver", &["*"], &key, method)
                .await
        } else {
            self.served.call(&key, method).await
        };
        ok("Writeback", response)
    }

    /// The edges of the graph as `(source, target)` pairs.
    pub(in crate::admin) async fn edge_pairs(&self) -> BTreeSet<(String, String)> {
        let state = self.served.state.read().await;
        let core = state.registry.get(self.spec.graph).unwrap().core.clone();
        core.get_edges()
            .into_iter()
            .map(|(source, target, _)| (source, target))
            .collect()
    }
}
