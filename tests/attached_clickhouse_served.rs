//! A live ClickHouse source read through the served path (MEDULLA-UNIFIED-DATA-PLANE-R017.2.2).
//!
//! The source is registered with `Method::RegisterForeignSource` (a `ClickHouse` spec) and read
//! with `Method::QueryForeignColumns`, both through the real `dispatch` surface. The ClickHouse
//! server is an in-process loopback HTTP endpoint that speaks the `JSONCompact` interface: it
//! answers `system.columns`, `system.tables` and a `SELECT` whose `{pN:Type}` placeholders bind
//! through `param_pN` query parameters, over one fixed table. The served rows are compared with
//! the table's own rows. A capture request (a `watermark`) refuses `CAPABILITY_UNSUPPORTED` with
//! the reason `clickhouse_no_cdc`, and a column the table lacks is `SCHEMA_DRIFT`.
#![cfg(all(feature = "federation", feature = "server", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_query_types::wire::{
    ForeignColumnComparison, ForeignColumnPredicate, ForeignColumnRow, ForeignSourceSpec,
};

const SECRET: &str = "medulla-unified-data-plane-r017-2-2-clickhouse-secret";

/// The fixture table `events (id UInt64, name String, amount Int64)`.
const ROWS: [(u64, &str, i64); 4] = [(1, "ada", 10), (2, "bea", 20), (3, "cam", 30), (4, "ada", 40)];

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn cell(column: &str, row: &(u64, &str, i64)) -> serde_json::Value {
    match column {
        "id" => serde_json::json!(row.0.to_string()),
        "name" => serde_json::json!(row.1),
        _ => serde_json::json!(row.2),
    }
}

fn type_of(column: &str) -> &'static str {
    match column {
        "id" => "UInt64",
        "name" => "String",
        _ => "Int64",
    }
}

fn compare(left: &serde_json::Value, op: &str, right: &str) -> bool {
    let ordering = match (left.as_str(), right.parse::<f64>()) {
        (Some(text), Err(_)) => text.cmp(right),
        _ => {
            let l = left
                .as_i64()
                .map(|n| n as f64)
                .or_else(|| left.as_str().and_then(|t| t.parse().ok()))
                .unwrap_or(f64::NAN);
            let r: f64 = right.parse().unwrap_or(f64::NAN);
            match l.partial_cmp(&r) {
                Some(o) => o,
                None => return false,
            }
        }
    };
    match op {
        "=" => ordering.is_eq(),
        "!=" => ordering.is_ne(),
        "<" => ordering.is_lt(),
        "<=" => ordering.is_le(),
        ">" => ordering.is_gt(),
        _ => ordering.is_ge(),
    }
}

fn answer(sql: &str, params: &BTreeMap<String, String>) -> String {
    if sql.contains("FROM system.columns") {
        let meta = r#"[{"name":"name","type":"String"},{"name":"type","type":"String"}]"#;
        let data: Vec<_> = ["id", "name", "amount"]
            .iter()
            .map(|c| serde_json::json!([c, type_of(c)]))
            .collect();
        return format!(r#"{{"meta":{meta},"data":{},"rows":3}}"#, serde_json::json!(data));
    }
    if sql.contains("FROM system.tables") {
        return r#"{"meta":[{"name":"engine","type":"String"}],"data":[["MergeTree"]],"rows":1}"#
            .to_string();
    }
    let projection = sql
        .strip_prefix("SELECT ")
        .and_then(|rest| rest.split(" FROM ").next())
        .unwrap_or("");
    let columns: Vec<String> = projection
        .split(", ")
        .map(|c| c.trim_matches('`').to_string())
        .collect();
    let mut conditions = Vec::new();
    if let Some(clause) = sql.split(" WHERE ").nth(1) {
        for part in clause.split(" AND ") {
            let mut pieces = part.splitn(3, ' ');
            let column = pieces.next().unwrap_or("").trim_matches('`').to_string();
            let op = pieces.next().unwrap_or("=").to_string();
            let placeholder = pieces.next().unwrap_or("");
            let name = placeholder.trim_matches(|c| c == '{' || c == '}');
            let name = name.split(':').next().unwrap_or("");
            let value = params
                .get(&format!("param_{name}"))
                .cloned()
                .unwrap_or_default();
            conditions.push((column, op, value));
        }
    }
    let data: Vec<_> = ROWS
        .iter()
        .filter(|row| {
            conditions
                .iter()
                .all(|(column, op, value)| compare(&cell(column, row), op, value))
        })
        .map(|row| {
            serde_json::Value::Array(columns.iter().map(|c| cell(c, row)).collect())
        })
        .collect();
    let meta: Vec<_> = columns
        .iter()
        .map(|c| serde_json::json!({"name": c, "type": type_of(c)}))
        .collect();
    serde_json::json!({"meta": meta, "data": data, "rows": data.len()}).to_string()
}

fn handle(mut stream: TcpStream) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let length: usize = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap_or(0))
        })
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
    let sql = String::from_utf8_lossy(&buffer[header_end..]).to_string();
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or("/");
    let query = target.split_once('?').map_or("", |(_, q)| q);
    let params: BTreeMap<String, String> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (percent_decode(k), percent_decode(v)))
        .collect();
    let body = answer(&sql, &params);
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Serve the fixture on a loopback port; returns the port.
fn serve() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fixture server");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || handle(stream));
        }
    });
    port
}

fn clickhouse(port: u16, columns: &[&str], watermark: Option<&str>) -> ForeignSourceSpec {
    ForeignSourceSpec::ClickHouse {
        host: "127.0.0.1".to_string(),
        port,
        database: "analytics".to_string(),
        tls: false,
        auth_ref: None,
        table: "events".to_string(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
        id_column: "id".to_string(),
        watermark: watermark.map(str::to_string),
    }
}

async fn register(
    state: &test_support::SharedState,
    id: u64,
    name: &str,
    spec: ForeignSourceSpec,
) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::RegisterForeignSource {
                name: name.to_string(),
                source: spec,
                quota: None,
                acceleration: None,
            },
        ),
    )
    .await
}

async fn read(
    state: &test_support::SharedState,
    id: u64,
    name: &str,
    columns: &[&str],
    predicates: Vec<ForeignColumnPredicate>,
) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::QueryForeignColumns {
                name: name.to_string(),
                columns: columns.iter().map(|c| c.to_string()).collect(),
                predicates,
            },
        ),
    )
    .await
}

fn rows(response: &Response) -> Vec<(String, BTreeMap<String, serde_json::Value>)> {
    assert!(
        response.error.is_none(),
        "read refused: {:?}",
        response.error
    );
    let Some(ResultPayload::Raw(bytes)) = &response.result else {
        panic!("unexpected read payload: {:?}", response.result);
    };
    let mut rows: Vec<ForeignColumnRow> = rmp_serde::from_slice(bytes).expect("msgpack rows");
    rows.sort_by_key(|row| row.id.parse::<i64>().unwrap_or(i64::MAX));
    rows.into_iter().map(|row| (row.id, row.columns)).collect()
}

fn refusal(response: &Response) -> &str {
    response.error.as_deref().unwrap_or_else(|| {
        panic!("expected a refusal, got {:?}", response.result);
    })
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn federated_reads_match_native_and_capture_is_refused() {
    common::configure_authority();
    medulla_plan::clickhouse_connection::register();
    let port = serve();
    std::env::set_var(
        medulla_federation::federation_ssrf::HTTP_JSON_FEDERATION_ALLOW_ENV,
        format!("127.0.0.1:{port}"),
    );
    let state = test_support::durable_state(SECRET, common::current_isolation());

    let registered = register(&state, 1, "ch_events", clickhouse(port, &["id", "name", "amount"], None)).await;
    assert!(registered.error.is_none(), "register: {:?}", registered.error);

    // A pushed ordered filter and a projection equal the table's own rows.
    let served = rows(
        &read(
            &state,
            2,
            "ch_events",
            &["name", "amount"],
            vec![ForeignColumnPredicate {
                column: "amount".to_string(),
                comparison: ForeignColumnComparison::Ge,
                value: serde_json::json!(20),
            }],
        )
        .await,
    );
    let served: Vec<(String, i64)> = served
        .into_iter()
        .map(|(_, columns)| {
            (
                columns["name"].as_str().unwrap().to_string(),
                columns["amount"].as_i64().unwrap(),
            )
        })
        .collect();
    let native: Vec<(String, i64)> = ROWS
        .iter()
        .filter(|(_, _, amount)| *amount >= 20)
        .map(|(_, name, amount)| (name.to_string(), *amount))
        .collect();
    assert_eq!(served, native);

    // A string equality binds through a query parameter.
    let ada = rows(
        &read(
            &state,
            3,
            "ch_events",
            &["id"],
            vec![ForeignColumnPredicate {
                column: "name".to_string(),
                comparison: ForeignColumnComparison::Eq,
                value: serde_json::json!("ada"),
            }],
        )
        .await,
    );
    let ids: Vec<String> = ada.into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, vec!["1".to_string(), "4".to_string()]);

    // A column the table lacks is schema drift, not an empty column.
    let registered = register(&state, 4, "ch_drift", clickhouse(port, &["id", "missing"], None)).await;
    assert!(registered.error.is_none(), "register: {:?}", registered.error);
    let drift = read(&state, 5, "ch_drift", &["missing"], vec![]).await;
    assert!(refusal(&drift).starts_with("SCHEMA_DRIFT"), "{:?}", drift.error);

    // A capture request is refused: ClickHouse has no change capture.
    let capture = register(&state, 6, "ch_capture", clickhouse(port, &["id"], Some("updated_at"))).await;
    let message = refusal(&capture);
    assert!(message.starts_with("CAPABILITY_UNSUPPORTED"), "{message}");
    assert!(message.contains("clickhouse_no_cdc"), "{message}");

    // A server that is not there is SOURCE_UNAVAILABLE, never an empty result.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = dead.local_addr().unwrap().port();
    drop(dead);
    std::env::set_var(
        medulla_federation::federation_ssrf::HTTP_JSON_FEDERATION_ALLOW_ENV,
        format!("127.0.0.1:{port},127.0.0.1:{dead_port}"),
    );
    let registered = register(&state, 7, "ch_gone", clickhouse(dead_port, &["id"], None)).await;
    assert!(registered.error.is_none(), "register: {:?}", registered.error);
    let gone = read(&state, 8, "ch_gone", &["id"], vec![]).await;
    assert!(refusal(&gone).starts_with("SOURCE_UNAVAILABLE"), "{:?}", gone.error);
}
