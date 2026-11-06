//! Rust round-trip vectors for every wire method (MEDULLA-CONTRACT-R001.3).
//!
//! No sample is hand-written: `sample_builder::build_sample` derives each request from the
//! `Method` variant's schema and each result from the schema of the body that
//! `medulla_protocol_types::result_contract` registers for the method. The committed vectors
//! live in `tests/fixtures/wire_vectors/<domain>.json`. They are test data, not a generated
//! contract: `MEDULLA_WRITE_VECTORS=1` rewrites them for a deliberate wire change.
//!
//! Run: `cargo test -p medulla-capabilities --test wire_vectors`.
#![cfg(feature = "contract-schema")]

#[path = "wire_vectors/sample_builder.rs"]
mod sample_builder;

use std::collections::BTreeMap;
use std::path::PathBuf;

use medulla_capabilities::method_descriptors;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::result_contract::{visit_all, MethodResult, ResultVisitor};
use medulla_types::contract::Digest256;
use serde_json::{json, Map, Value};

use sample_builder::{build_node, build_sample};

/// The result bodies of each method, keyed by method id then op tag (empty for one body).
#[derive(Default)]
struct ResultSchemas(BTreeMap<&'static str, BTreeMap<&'static str, schemars::Schema>>);

impl ResultVisitor for ResultSchemas {
    fn visit<M: MethodResult>(&mut self) {
        self.0
            .entry(M::METHOD)
            .or_default()
            .entry(M::OP)
            .or_insert_with(|| {
                schemars::SchemaGenerator::default().into_root_schema_for::<M::Body>()
            });
    }
}

/// One wire-callable method: published to a consumer profile and not internal.
fn wire_methods() -> Vec<(&'static str, &'static str)> {
    let mut methods: Vec<_> = method_descriptors()
        .filter(|d| {
            d.stability != medulla_capabilities::Stability::Internal
                && !d.consumer_profiles.is_empty()
        })
        .map(|d| (d.domain, d.id.as_str()))
        .collect();
    methods.sort();
    methods
}

/// Recursively rebuild `value` with object keys in sorted order.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), canonical(&map[key])))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The `Method` variant whose serde tag equals `method`.
fn request_variant<'a>(root: &'a Value, method: &str) -> Option<&'a Value> {
    ["oneOf", "anyOf"]
        .iter()
        .filter_map(|keyword| root.get(*keyword).and_then(Value::as_array))
        .flatten()
        .find(|variant| {
            variant
                .pointer("/properties/method/const")
                .and_then(Value::as_str)
                == Some(method)
                || variant
                    .pointer("/properties/method/enum/0")
                    .and_then(Value::as_str)
                    == Some(method)
        })
}

/// One vector row: the method id, its sample request and result, and the canonical-bytes digest.
fn vector(
    method_schema: &schemars::Schema,
    results: &ResultSchemas,
    method: &str,
) -> Result<Value, String> {
    let root = method_schema.as_value();
    let variant = request_variant(root, method)
        .ok_or_else(|| format!("{method}: no Method schema variant carries this tag"))?;
    let request = build_node(root, variant, method).map_err(|error| error.to_string())?;

    // The generated request must deserialize back to an equal `Method`.
    let decoded: Method = serde_json::from_value(request.clone())
        .map_err(|error| format!("{method}: request does not deserialize: {error}"))?;
    let reencoded = serde_json::to_value(&decoded)
        .map_err(|error| format!("{method}: request does not serialize: {error}"))?;
    if canonical(&reencoded) != canonical(&request) {
        return Err(format!(
            "{method}: request does not round-trip\n  built:   {request}\n  decoded: {reencoded}"
        ));
    }

    let bodies = results
        .0
        .get(method)
        .ok_or_else(|| format!("{method}: no result marker is registered"))?;
    let mut samples = BTreeMap::new();
    for (op, schema) in bodies {
        let sample = build_sample(schema, method).map_err(|error| error.to_string())?;
        samples.insert(*op, canonical(&sample));
    }
    let result = match samples.len() {
        1 => samples.into_values().next().expect("one sample"),
        _ => json!(samples),
    };

    let request = canonical(&request);
    let digest = Digest256::sha256(
        json!({"method": method, "request": request, "result": result})
            .to_string()
            .as_bytes(),
    )
    .to_hex();
    Ok(json!({"method": method, "request": request, "result": result, "sha256": digest}))
}

fn fixture_path(domain: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/wire_vectors")
        .join(format!("{domain}.json"))
}

/// The vectors of every wire method, grouped by domain and sorted by method id.
fn all_vectors() -> &'static BTreeMap<&'static str, Vec<Value>> {
    static VECTORS: std::sync::OnceLock<BTreeMap<&'static str, Vec<Value>>> =
        std::sync::OnceLock::new();
    VECTORS.get_or_init(build_vectors)
}

fn build_vectors() -> BTreeMap<&'static str, Vec<Value>> {
    let method_schema = schemars::schema_for!(Method);
    let mut results = ResultSchemas::default();
    visit_all(&mut results);
    let mut failures = Vec::new();
    let mut by_domain: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    for (domain, method) in wire_methods() {
        match vector(&method_schema, &results, method) {
            Ok(row) => by_domain.entry(domain).or_default().push(row),
            Err(error) => failures.push(error),
        }
    }
    assert!(
        failures.is_empty(),
        "{} wire method(s) have no round-trip vector:\n{}",
        failures.len(),
        failures.join("\n")
    );
    by_domain
}

fn render(rows: &[Value]) -> String {
    let mut text =
        serde_json::to_string_pretty(&Value::Array(rows.to_vec())).expect("vectors serialize");
    text.push('\n');
    text
}

/// Compare one domain's generated rows with its committed file.
fn check_domain(domain: &str) {
    let vectors = all_vectors();
    let rows = vectors
        .get(domain)
        .unwrap_or_else(|| panic!("domain {domain} has no wire-callable method"));
    check_rows(domain, rows);
}

fn check_rows(domain: &str, rows: &[Value]) {
    let path = fixture_path(domain);
    let expected = render(rows);
    if std::env::var_os("MEDULLA_WRITE_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().expect("fixture dir")).expect("create fixture dir");
        std::fs::write(&path, expected).expect("write vectors");
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{}: {error}; rerun with MEDULLA_WRITE_VECTORS=1 to write the vectors",
            path.display()
        )
    });
    assert!(
        !committed.contains('\r'),
        "{} must use LF line endings",
        path.display()
    );
    assert!(
        committed == expected,
        "{} differs from the generated vectors; a deliberate wire change reruns with \
         MEDULLA_WRITE_VECTORS=1",
        path.display()
    );
}

// spec: MEDULLA-CONTRACT-R001.3
#[test]
fn vectors_cover_every_wire_method_and_round_trip() {
    let vectors = all_vectors();
    let covered: usize = vectors.values().map(Vec::len).sum();
    assert_eq!(
        covered,
        wire_methods().len(),
        "every wire method has a vector"
    );
    for (domain, rows) in &vectors {
        let methods: Vec<&str> = rows
            .iter()
            .filter_map(|row| row["method"].as_str())
            .collect();
        let mut sorted = methods.clone();
        sorted.sort();
        assert_eq!(methods, sorted, "{domain}: rows are sorted by method id");
        check_rows(domain, rows);
    }
}

// spec: MEDULLA-CONTRACT-R001.3.1
#[test]
fn domain_cluster_vectors_round_trip() {
    check_domain("cluster");
}

// spec: MEDULLA-CONTRACT-R001.3.2
#[test]
fn domain_compute_vectors_round_trip() {
    check_domain("compute");
}

// spec: MEDULLA-CONTRACT-R001.3.3
#[test]
fn domain_coordination_vectors_round_trip() {
    check_domain("coordination");
}

// spec: MEDULLA-CONTRACT-R001.3.4
#[test]
fn domain_graph_vectors_round_trip() {
    check_domain("graph");
}

// spec: MEDULLA-CONTRACT-R001.3.5
#[test]
fn domain_ingestion_vectors_round_trip() {
    check_domain("ingestion");
}

// spec: MEDULLA-CONTRACT-R001.3.6
#[test]
fn domain_messaging_vectors_round_trip() {
    check_domain("messaging");
}

// spec: MEDULLA-CONTRACT-R001.3.7
#[test]
fn domain_query_vectors_round_trip() {
    check_domain("query");
}

// spec: MEDULLA-CONTRACT-R001.3.8
#[test]
fn domain_reasoning_vectors_round_trip() {
    check_domain("reasoning");
}

// spec: MEDULLA-CONTRACT-R001.3.9
#[test]
fn domain_security_vectors_round_trip() {
    check_domain("security");
}

// spec: MEDULLA-CONTRACT-R001.3.10
#[test]
fn domain_storage_vectors_round_trip() {
    check_domain("storage");
}

// spec: MEDULLA-CONTRACT-R001.3.11
#[test]
fn domain_transactions_vectors_round_trip() {
    check_domain("transactions");
}
