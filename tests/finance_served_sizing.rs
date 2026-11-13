//! MEDULLA-FINANCE-PRIMITIVES-R900.3: the served `FinancePositionSize` and
//! `FinanceDistributionShift` routes run the shared `medulla_finance_quant::sizing` kernels
//! through the real signed `dispatch` surface.
#![cfg(all(feature = "server", feature = "security", feature = "finance"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};

const SECRET: &str = "medulla-fin-r900-3-served-sizing-secret";

const GRAPH: &str = "fin-r900-3";

/// A served call: one signed dispatch against a fresh state holding the graph.
async fn call(method: Method) -> Response {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let create = Method::CreateGraph {
        graph_name: GRAPH.to_string(),
        graph_type: GraphType::Global,
    };
    let created =
        test_support::dispatch(&state, test_support::request(SECRET, 1, GRAPH, create)).await;
    assert!(
        created.error.is_none(),
        "CreateGraph: {:?}",
        created.error_detail
    );
    test_support::dispatch(&state, test_support::request(SECRET, 2, GRAPH, method)).await
}

fn position(
    accuracy: f64,
    win_loss_ratio: f64,
    fraction: Option<f64>,
    max_risk: Option<f64>,
) -> Method {
    Method::FinancePositionSize {
        accuracy,
        win_loss_ratio,
        fraction,
        max_risk,
        series_revision: None,
    }
}

fn float(response: &Response) -> f64 {
    match response.result.as_ref() {
        Some(ResultPayload::Float(value)) => *value,
        other => panic!(
            "expected a float result, got {other:?} / {:?} {:?}",
            response.error, response.error_detail
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R900.3
async fn position_size_runs_the_sizing_kernel_through_dispatch() {
    // p=0.6, b=2 -> full Kelly 0.4; quarter Kelly 0.1; capped at 0.05.
    let capped = call(position(0.6, 2.0, Some(0.25), Some(0.05))).await;
    assert!((float(&capped) - 0.05).abs() < 1e-12);
    // Defaults (half Kelly, 0.02 cap) apply when fraction/max_risk are omitted.
    let defaulted = call(position(0.6, 2.0, None, None)).await;
    assert!((float(&defaulted) - 0.02).abs() < 1e-12);
    // A losing edge sizes to zero.
    assert_eq!(
        float(&call(position(0.1, 1.0, Some(0.5), Some(0.5))).await),
        0.0
    );
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R900.3
async fn position_size_refuses_an_invalid_accuracy_with_the_kernel_code() {
    let refused = call(position(1.5, 2.0, None, None)).await;
    assert!(refused.result.is_none());
    assert_eq!(refused.error.as_deref(), Some("INVALID_ARGUMENT"));
    assert!(refused
        .error_detail
        .as_deref()
        .is_some_and(|d| d.contains("SIZING_INPUT_INVALID")));
}

#[tokio::test(flavor = "multi_thread")]
// spec: MEDULLA-FINANCE-PRIMITIVES-R900.3
async fn distribution_shift_runs_the_ks_kernel_through_dispatch() {
    let reference: Vec<f64> = (0..200).map(|i| i as f64 / 200.0).collect();
    let moved: Vec<f64> = reference.iter().map(|v| v + 5.0).collect();
    let shifted = call(Method::FinanceDistributionShift {
        reference: reference.clone(),
        current: moved,
        threshold: 0.5,
    })
    .await;
    let same = call(Method::FinanceDistributionShift {
        reference: reference.clone(),
        current: reference,
        threshold: 0.5,
    })
    .await;
    for (response, expect_shifted) in [(&shifted, true), (&same, false)] {
        let Some(ResultPayload::Raw(bytes)) = response.result.as_ref() else {
            panic!("expected a raw result, got {:?}", response.error);
        };
        let value: serde_json::Value = rmp_serde::from_slice(bytes).expect("msgpack result");
        assert_eq!(value["shifted"], expect_shifted);
    }
    let empty = call(Method::FinanceDistributionShift {
        reference: vec![],
        current: vec![1.0],
        threshold: 0.5,
    })
    .await;
    assert!(empty
        .error_detail
        .as_deref()
        .is_some_and(|d| d.contains("SHIFT_SAMPLE_EMPTY")));
}

#[tokio::test(flavor = "multi_thread")]
/// Verifies MEDULLA-FINANCE-PRIMITIVES-R900.6: Hot-path proof
// spec: MEDULLA-FINANCE-PRIMITIVES-R900.6
async fn distribution_shift_serves_100k_samples() {
    // Two 100k-value uniform grids, the second moved by 0.1: the two-sample KS
    // statistic is 0.1 up to the grid step (1e-5).
    const N: usize = 100_000;
    let reference: Vec<f64> = (0..N).map(|i| i as f64 / N as f64).collect();
    let current: Vec<f64> = reference.iter().map(|v| v + 0.1).collect();
    let response = call(Method::FinanceDistributionShift {
        reference,
        current,
        threshold: 0.05,
    })
    .await;
    let Some(ResultPayload::Raw(bytes)) = response.result.as_ref() else {
        panic!("expected a raw result, got {:?}", response.error_detail);
    };
    let value: serde_json::Value = rmp_serde::from_slice(bytes).expect("msgpack result");
    let statistic = value["statistic"].as_f64().expect("statistic");
    assert!((statistic - 0.1).abs() <= 2e-5, "statistic {statistic}");
    assert_eq!(value["shifted"], true);
}

// ── MEDULLA-FINANCE-PRIMITIVES-R902: probabilistic forecast evidence is never a kernel input ──

#[cfg(feature = "blob")]
mod forecast_served {
    //! A forecast sample enters through a served `SourceIngest` (an imported connector pack
    //! maps the `forecast` stream to `ForecastSample`), is read back through the served graph
    //! read, and is then named as the series revision of the served sizing and backtest
    //! requests.

    use std::sync::Arc;

    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    use medulla_catalog_types::connector_pack::{
        ConnectorPackBindRequest, ConnectorPackImportRequest, ConnectorPackIndex, ConnectorPackOp,
        McpCatalogSnapshotBinding, PackAnnotations, PackArchiveRef, PackEntry, PackEntryKind,
        PackImportResult, PackProducer, PackSection, CONNECTOR_PACK_SCHEMA_VERSION,
    };
    use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
    #[allow(unused_imports)]
    use medulla_server_blob::blob::store::{ChunkStore, RedbChunkStore};
    use medulla_server_blob::blob::BlobCursors;
    use medulla_source_types::source_ingestion::{SourceIngestionReceipt, SourceIngestionRequest};
    use medulla_types::contract::{BoundedVec, Digest256, ResourceId};

    use super::{common, test_support};

    const SECRET: &str = "finance-r902-forecast-served-secret";
    pub const GRAPH: &str = "fin-r902";
    const CONNECTOR: &str = "forecast-source";
    pub const FORECAST_STREAM: &str = "forecast";
    pub const SERIES_STREAM: &str = "series";
    const FORECAST_FIELDS: [(&str, &str); 8] = [
        ("series_id", "series_id"),
        ("model_revision", "model_revision"),
        ("seed", "seed"),
        ("temperature", "temperature"),
        ("top_p", "top_p"),
        ("sample_count", "sample_count"),
        ("values", "values"),
        ("probabilistic", "probabilistic"),
    ];

    pub type State = test_support::SharedState;

    pub async fn call(state: &State, key: &str, graph: &str, method: Method) -> Response {
        let _ = key;
        test_support::dispatch(
            state,
            test_support::request(SECRET, next_id(), graph, method),
        )
        .await
    }

    fn next_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(10);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    pub fn ok<T: serde::de::DeserializeOwned>(label: &str, response: Response) -> T {
        assert!(
            response.error.is_none(),
            "{label} was refused: {response:?}"
        );
        let value = match response.result {
            Some(ResultPayload::Raw(bytes)) => {
                return rmp_serde::from_slice(&bytes)
                    .unwrap_or_else(|error| panic!("{label} does not decode: {error}"));
            }
            Some(ResultPayload::Json(value)) => value,
            other => panic!("{label} answered an unexpected payload: {other:?}"),
        };
        serde_json::from_value(value)
            .unwrap_or_else(|error| panic!("{label} does not decode: {error}"))
    }

    fn context(key: &str) -> medulla_agent_types::agent_library::AgentLibraryMutationContext {
        medulla_agent_types::agent_library::AgentLibraryMutationContext {
            request_id: 1,
            principal: "bound-by-engine".to_string(),
            caller_principal: "bound-by-engine".to_string(),
            attempt_nonce: medulla_types::contract::Nonce::from_bytes([0; 32]),
            tenant_id: common::TEST_TENANT.to_string(),
            actor_scope: "bound-by-engine".to_string(),
            purpose_id: "bound-by-engine".to_string(),
            policy_revision: "bound-by-engine".to_string(),
            policy_digest: format!("sha256:{}", "0".repeat(64)),
            policy_decision_id: "bound-by-engine".to_string(),
            idempotency_key: key.to_string(),
            expected_revision: None,
            trace_id: None,
            created_at_ms: 1,
        }
    }

    fn section(archive: &mut Vec<u8>, bytes: &[u8]) -> PackSection {
        let section = PackSection {
            offset: archive.len() as u64,
            length: bytes.len() as u64,
            sha256: Digest256::from_bytes(Sha256::digest(bytes).into()),
        };
        archive.extend_from_slice(bytes);
        section
    }

    /// A digest-honest pack: `forecast` maps to `ForecastSample`, `series` to a plain `BarSeries`.
    fn pack() -> (ConnectorPackIndex, Vec<u8>) {
        let mut archive = Vec::new();
        let server_body =
            format!(r#"{{"contract_version":"1","instructions":"Test.","name":"{CONNECTOR}"}}"#);
        let entry = |kind, uri: String, name: &str, body: PackSection| PackEntry {
            kind,
            uri,
            name: name.to_string(),
            media_type: "application/json".to_string(),
            body,
            input_schema: None,
            output_schema: None,
            annotations: PackAnnotations::default(),
            references: BoundedVec::new(Vec::new()).unwrap(),
        };
        let server = entry(
            PackEntryKind::McpServer,
            format!("mcp-server://{CONNECTOR}"),
            CONNECTOR,
            section(&mut archive, server_body.as_bytes()),
        );
        let forecast_fields: serde_json::Map<String, Value> = FORECAST_FIELDS
            .iter()
            .map(|(source, target)| (source.to_string(), json!(target)))
            .collect();
        let mapping = json!({
            "connector": CONNECTOR,
            "schema_mappings": {
                "forecast": {"fields": forecast_fields, "ontology_class": "ForecastSample"},
                "series": {"fields": {"series_id": "seriesId"}, "ontology_class": "BarSeries"},
            },
            "sources": [],
        })
        .to_string();
        let resource = entry(
            PackEntryKind::Resource,
            "connector://resources".to_string(),
            "connector-resources",
            section(&mut archive, mapping.as_bytes()),
        );
        let mut index = ConnectorPackIndex {
            schema_version: CONNECTOR_PACK_SCHEMA_VERSION,
            connector: ResourceId::new(CONNECTOR).unwrap(),
            server,
            server_package_version: "1.0.0".to_string(),
            archive: PackArchiveRef {
                blob_digest: String::new(),
                length: archive.len() as u64,
                sha256: Digest256::from_bytes(Sha256::digest(&archive).into()),
            },
            entries: BoundedVec::new(vec![resource]).unwrap(),
            producer: PackProducer {
                name: "finance-r902-test".to_string(),
                version: "1".to_string(),
            },
            catalog: McpCatalogSnapshotBinding {
                configuration_revision: 1,
                catalog_generation: 1,
                snapshot_digest: Digest256::from_bytes([1; 32]),
                child_connection_generation: 1,
                authorization_scope_digest: Digest256::from_bytes([2; 32]),
            },
            pack_digest: Digest256::from_bytes([0; 32]),
        };
        index.pack_digest =
            medulla_catalog_types::connector_pack::digest::pack_digest(&index).unwrap();
        (index, archive)
    }

    async fn upload(state: &State, archive: &[u8]) -> String {
        let tag = format!("r902:{}", hex::encode(Sha256::digest(archive)));
        let cursor: u64 = ok(
            "BlobBegin",
            call(
                state,
                &tag,
                "__commons__",
                Method::BlobBegin {
                    chunk_size: 1 << 20,
                },
            )
            .await,
        );
        let _: Value = ok(
            "BlobChunkPut",
            call(
                state,
                &tag,
                "__commons__",
                Method::BlobChunkPut {
                    cursor,
                    data: archive.to_vec(),
                },
            )
            .await,
        );
        ok(
            "BlobCommit",
            call(state, &tag, "__commons__", Method::BlobCommit { cursor }).await,
        )
    }

    /// A durable store with the tenant graph, blob cursors and the imported pack.
    pub async fn harness() -> State {
        test_support::provision_encryption_key_once("finance-r902-served-encryption-key");
        let dir = test_support::fresh_dir("medulla-fin-r902");
        let dir_s = dir.to_string_lossy().to_string();
        let (_backend, state) =
            test_support::redb_state_at(SECRET, common::current_isolation(), &dir_s);
        let store = Arc::new(RedbChunkStore::open(&dir_s).unwrap());
        state
            .write()
            .await
            .set_blob_cursors(Some(Arc::new(BlobCursors::new(store))));
        let created = call(
            &state,
            "create",
            GRAPH,
            Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: GraphType::Global,
            },
        )
        .await;
        assert!(created.error.is_none(), "CreateGraph: {created:?}");
        let key = format!("bind:{CONNECTOR}");
        let _: Value = ok(
            "Bind",
            call(
                &state,
                &key,
                "__commons__",
                Method::ConnectorPack {
                    op: Box::new(ConnectorPackOp::Bind {
                        request: ConnectorPackBindRequest {
                            context: context(&key),
                            connector: ResourceId::new(CONNECTOR).unwrap(),
                            importer: format!(
                                "principal:sha256:{}",
                                hex::encode(Sha256::digest(common::TEST_AGENT.as_bytes()))
                            ),
                        },
                    }),
                },
            )
            .await,
        );
        let (mut index, archive) = pack();
        index.archive.blob_digest = upload(&state, &archive).await;
        let key = format!("connector-pack:{CONNECTOR}:import:{}:0", index.pack_digest);
        match ok::<PackImportResult>(
            "Import",
            call(
                &state,
                &key,
                "__commons__",
                Method::ConnectorPack {
                    op: Box::new(ConnectorPackOp::Import {
                        request: Box::new(ConnectorPackImportRequest {
                            context: context(&key),
                            index,
                            expected_head: None,
                            allow_mass_withdrawal: false,
                        }),
                    }),
                },
            )
            .await,
        ) {
            PackImportResult::Imported { .. } => {}
            other => panic!("the pack did not import: {other:?}"),
        }
        state
    }

    /// One `SourceIngest` batch of one record on `stream`.
    pub async fn ingest(
        state: &State,
        stream: &str,
        record_id: &str,
        payload: Value,
        page: u64,
    ) -> SourceIngestionReceipt {
        let value = json!({
            "connector": CONNECTOR, "kind": "records", "mode": "delta", "strict_schema": true,
            "records": [{
                "stream": stream, "record_id": record_id,
                "mapping_reference": format!("connector:{CONNECTOR}#mappings/{stream}"),
                "payload": payload,
                "updated_at": "2026-11-13T00:00:00Z",
                "provenance": {"connector": CONNECTOR, "adapter_kind": "mcp", "server": "forecast",
                                "tool": "sample", "tool_schema_sha256": "07".repeat(32),
                                "source_uri": format!("forecast://{record_id}")},
            }],
            "relationships": [], "withdrawals": [],
            "provider_checkpoint": {"stream": stream, "position": {"page": page},
                                      "watermark": "2026-11-13T00:00:00Z"},
            "expected_previous_checkpoint": Value::Null,
        });
        let request: SourceIngestionRequest =
            serde_json::from_value(value).expect("a valid source ingestion request");
        ok(
            "SourceIngest",
            call(
                state,
                &format!("ingest:{record_id}"),
                GRAPH,
                Method::SourceIngest { request },
            )
            .await,
        )
    }

    pub fn node_id(stream: &str, record_id: &str) -> String {
        medulla_h_ingest::source_ingestion::source_record_node_id(
            common::TEST_TENANT,
            CONNECTOR,
            stream,
            record_id,
        )
        .unwrap()
    }

    pub async fn properties(state: &State, id: &str) -> serde_json::Map<String, Value> {
        let value: Value = ok(
            "GetNodeProperties",
            call(
                state,
                id,
                GRAPH,
                Method::GetNodeProperties {
                    node_id: id.to_string(),
                },
            )
            .await,
        );
        value
            .as_object()
            .unwrap_or_else(|| panic!("node properties are an object: {value}"))
            .clone()
    }
}

#[cfg(feature = "blob")]
fn backtest_on(revision: &str) -> Method {
    use medulla_result_types::compute_result::market::{
        BacktestRunDraft, CostModel, DataRevisionRef, Direction, FillRule, FinanceMarketOp,
        TradeFill, UniverseMember, ValidationInputs,
    };
    const DAY: i64 = 86_400_000;
    let returns: Vec<f64> = (0..48)
        .map(|i| f64::from((i * 37 % 11) - 4) / 1_000.0)
        .collect();
    Method::FinanceMarket {
        op: Box::new(FinanceMarketOp::BacktestRun {
            draft: BacktestRunDraft {
                strategy: "super_trend@1 long-only".to_string(),
                signal_keys: vec!["sha256:00".to_string()],
                data_revisions: vec![DataRevisionRef {
                    series_id: "bars/binance/BTC-USDT/1D".to_string(),
                    source_revision: revision.to_string(),
                    known_as_of: 1_000 * DAY,
                }],
                universe: vec![UniverseMember {
                    listing_id: "binance:BTC/USDT:spot".to_string(),
                    from: 0,
                    until: Some(900 * DAY),
                }],
                costs: CostModel {
                    fee_bps: 10,
                    slippage_bps: 5,
                },
                fill_rule: FillRule::NextBarOpen,
                fills: vec![TradeFill {
                    listing_id: "binance:BTC/USDT:spot".to_string(),
                    known_at: 100 * DAY,
                    fill_at: 100 * DAY,
                    fill_price: 101_000,
                    direction: Direction::Bullish,
                }],
                returns,
                validation: ValidationInputs {
                    n_groups: 6,
                    n_test_groups: 2,
                    purge_window: 2,
                    embargo: 1,
                    n_trials: 5,
                    performance: vec![vec![1.0, 0.2, 0.1]; 48],
                },
                supersedes: None,
            },
        }),
    }
}

#[cfg(feature = "blob")]
fn assert_refused_as_probabilistic(label: &str, response: &Response) {
    assert!(
        response.result.is_none(),
        "{label} must not produce a result"
    );
    assert_eq!(
        response.error.as_deref(),
        Some("INVALID_ARGUMENT"),
        "{label}"
    );
    assert!(
        response
            .error_detail
            .as_deref()
            .is_some_and(|detail| detail.starts_with("PROBABILISTIC_INPUT_REFUSED")
                || detail.contains("PROBABILISTIC_INPUT_REFUSED")),
        "{label}: {:?}",
        response.error_detail
    );
}

#[cfg(feature = "blob")]
#[test]
// spec: MEDULLA-FINANCE-PRIMITIVES-R902
fn probabilistic_forecast_is_refused_as_kernel_input() {
    test_support::on_engine(probabilistic_forecast_flow);
}

#[cfg(feature = "blob")]
async fn probabilistic_forecast_flow() {
    use forecast_served::{
        call, harness, ingest, node_id, properties, FORECAST_STREAM, GRAPH, SERIES_STREAM,
    };
    use serde_json::json;
    let state = harness().await;

    // The forecast is ingested through the served SourceIngest, claiming probabilistic=false.
    let receipt = ingest(
        &state,
        FORECAST_STREAM,
        "sample-1",
        json!({"series_id": "SPY", "model_revision": "m-7", "seed": 42, "temperature": 0.7,
               "top_p": 0.9, "sample_count": 8, "values": [101.5, 102.25, 99.75],
               "probabilistic": false}),
        1,
    )
    .await;
    assert_eq!(receipt.raw_admissions.len(), 1);
    // A deterministic series record sits beside it.
    ingest(
        &state,
        SERIES_STREAM,
        "bars-1",
        json!({"series_id": "SPY"}),
        1,
    )
    .await;

    // Readback preserves the class, the flag (forced true), the seed and the controls.
    let forecast = node_id(FORECAST_STREAM, "sample-1");
    let stored = properties(&state, &forecast).await;
    assert_eq!(stored["type"], "ForecastSample");
    let sample = medulla_types::finance_forecast::ForecastSample::from_stored(&stored)
        .expect("the stored node reads back as a forecast sample");
    assert!(sample.probabilistic, "the flag is forced true at admission");
    assert_eq!(sample.seed, 42);
    assert_eq!((sample.temperature, sample.top_p), (0.7, 0.9));
    assert_eq!(sample.sample_count, 8);
    assert_eq!(sample.values, vec![101.5, 102.25, 99.75]);
    assert_eq!(
        sample.decision_evidence(),
        (
            medulla_types::evidence_class::EvidenceClass::Claim,
            "forecast:seed=42".to_string()
        )
    );

    // A forecast with an invalid control is refused at ingest and writes nothing.
    let bad = serde_json::from_value::<
        medulla_source_types::source_ingestion::SourceIngestionRequest,
    >(json!({
        "connector": "forecast-source", "kind": "records", "mode": "delta", "strict_schema": true,
        "records": [{"stream": FORECAST_STREAM, "record_id": "sample-bad",
            "mapping_reference": "connector:forecast-source#mappings/forecast",
            "payload": {"series_id": "SPY", "model_revision": "m-7", "seed": 1,
                        "temperature": 0.7, "top_p": 7.0, "sample_count": 8,
                        "values": [1.0], "probabilistic": true},
            "updated_at": "2026-11-13T00:00:00Z",
            "provenance": {"connector": "forecast-source", "adapter_kind": "mcp",
                "server": "forecast", "tool": "sample", "tool_schema_sha256": "07".repeat(32),
                "source_uri": "forecast://sample-bad"}}],
        "relationships": [], "withdrawals": [],
        "provider_checkpoint": {"stream": FORECAST_STREAM, "position": {"page": 2},
                                "watermark": "2026-11-13T00:00:00Z"},
        "expected_previous_checkpoint": {"stream": FORECAST_STREAM, "position": {"page": 1}},
    }))
    .unwrap();
    let refused = call(&state, "bad", GRAPH, Method::SourceIngest { request: bad }).await;
    assert!(
        format!("{refused:?}").contains("FORECAST_SAMPLE_INVALID"),
        "{refused:?}"
    );
    let present = call(
        &state,
        "has",
        GRAPH,
        Method::HasNode {
            node_id: node_id(FORECAST_STREAM, "sample-bad"),
        },
    )
    .await;
    assert!(
        matches!(present.result, Some(ResultPayload::Bool(false))),
        "a refused forecast wrote nothing: {present:?}"
    );

    // Sizing: the forecast revision is refused with the code; deterministic inputs are served.
    let sized = |revision: Option<String>| Method::FinancePositionSize {
        accuracy: 0.6,
        win_loss_ratio: 2.0,
        fraction: Some(0.25),
        max_risk: Some(0.05),
        series_revision: revision,
    };
    let refused = call(&state, "size-f", GRAPH, sized(Some(forecast.clone()))).await;
    assert_refused_as_probabilistic("sizing on a forecast revision", &refused);
    for (label, revision) in [
        ("no revision", None),
        (
            "a deterministic stored revision",
            Some(node_id(SERIES_STREAM, "bars-1")),
        ),
        ("an unknown revision", Some("sha256:unknown".to_string())),
    ] {
        let served = call(&state, "size-d", GRAPH, sized(revision)).await;
        assert!(served.error.is_none(), "{label}: {:?}", served.error_detail);
        let size = float(&served);
        assert!((size - 0.05).abs() < 1e-12, "{label}: {size}");
    }

    // Backtest: a draft that read the forecast revision is refused; one that read a
    // deterministic revision still seals.
    let refused = call(&state, "bt-f", GRAPH, backtest_on(&forecast)).await;
    assert_refused_as_probabilistic("backtest on a forecast revision", &refused);
    let sealed = call(
        &state,
        "bt-d",
        GRAPH,
        backtest_on(&node_id(SERIES_STREAM, "bars-1")),
    )
    .await;
    assert!(
        sealed.error.is_none(),
        "a deterministic revision seals: {:?} {:?}",
        sealed.error,
        sealed.error_detail
    );
}
