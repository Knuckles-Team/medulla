//! The one Rust producer of wire vectors (MEDULLA-CONTRACT-R001.3, R001.4.1).
//!
//! No sample is hand-written: `sample_builder::build_sample` derives each request from the
//! `Method` variant's schema and each result from the schema of the body that
//! `medulla_protocol_types::result_contract` registers for the method. Nothing writes a
//! vector file into the repository and no rewrite switch exists: a consumer asks
//! [`build_vectors`] (the integration test, the `emit_wire_vectors` example) and
//! [`served_methods`] for the served set.

pub mod pattern_sample;
pub mod sample_builder;
pub mod sample_repair;

use std::collections::BTreeMap;

use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::result_contract::{visit_all, MethodResult, ResultVisitor};
use medulla_types::contract::Digest256;
use serde_json::{json, Map, Value};

use crate::method_descriptors;
use sample_builder::{build_node_with, build_sample};

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
            d.stability != crate::Stability::Internal && !d.consumer_profiles.is_empty()
        })
        .map(|d| (d.domain, d.id.as_str()))
        .collect();
    methods.sort();
    methods
}

/// `(domain, method id)` of every served wire method: every descriptor that is not
/// internal and is published to at least one consumer profile, sorted.
pub fn served_methods() -> Vec<(&'static str, &'static str)> {
    wire_methods()
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

/// Per-type sample hooks: the types whose decoder re-derives a digest over the value's own
/// fields. Each sample comes from the type's own constructor, never from hand-written JSON.
fn sample_hook(reference: &str) -> Option<Value> {
    let sample = match reference.strip_prefix("#/$defs/")? {
        "MutationScopeIdentity" => serde_json::to_value(
            medulla_mutation_types::mutation_scope::MutationScopeIdentity::fixed_native(
                "tenant-a",
                medulla_mutation_types::mutation_scope::DurabilityDomain::ControlPlane,
                "agent-library",
                "agent-library:v1",
            )
            .expect("a fixed native identity"),
        ),
        "SqlSourceJson" => serde_json::to_value(
            medulla_protocol_types::storage_wire::source_batch::SqlSourceJson::new(json!({}))
                .expect("an empty JSON object"),
        ),
        _ => return None,
    };
    Some(sample.expect("a constructed sample serializes"))
}

/// The request of a method whose decoder checks a rule spanning its fields (a tagged op
/// pair, a digest, a descriptor shape): the typed sample its own fixture constructs.
fn typed_request(method: &str) -> Option<Value> {
    medulla_protocol_types::protocol_fixtures::method_bodies::typed_samples()
        .into_iter()
        .find(|(name, _)| *name == method)
        .map(|(_, sample)| serde_json::to_value(sample).expect("a typed sample serializes"))
}

/// The request built from the `Method` variant's schema, repaired against the decoder.
fn schema_request(root: &Value, variant: &Value, method: &str) -> Result<Value, String> {
    let request =
        build_node_with(root, variant, method, &sample_hook).map_err(|error| error.to_string())?;
    sample_repair::repair(request, |value| {
        serde_json::from_value::<Method>(value.clone())
            .map(drop)
            .map_err(|error| error.to_string())
    })
    .map_err(|error| format!("{method}: request does not deserialize: {error}"))
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
    let request = match typed_request(method) {
        Some(typed) => typed,
        None => schema_request(root, variant, method)?,
    };

    // The generated request must deserialize, and its encoding must be a fixed point:
    // serde fills omitted defaulted fields on decode, so the vector carries the decoded
    // encoding, which must decode back to the same encoding.
    let decoded: Method = serde_json::from_value(request)
        .map_err(|error| format!("{method}: request does not deserialize: {error}"))?;
    let request = serde_json::to_value(&decoded)
        .map_err(|error| format!("{method}: request does not serialize: {error}"))?;
    let again: Method = serde_json::from_value(request.clone())
        .map_err(|error| format!("{method}: encoded request does not deserialize: {error}"))?;
    let reencoded = serde_json::to_value(&again)
        .map_err(|error| format!("{method}: request does not serialize: {error}"))?;
    if canonical(&reencoded) != canonical(&request) {
        return Err(format!(
            "{method}: request does not round-trip\n  encoded: {request}\n  again:   {reencoded}"
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

/// The vectors of the wire methods whose domain passes `keep`, from a fresh build: the rows
/// by domain, or one failure line per method id.
fn build_vectors_where(
    keep: impl Fn(&str) -> bool,
) -> Result<BTreeMap<&'static str, Vec<Value>>, Vec<String>> {
    let method_schema = schemars::schema_for!(Method);
    let mut results = ResultSchemas::default();
    visit_all(&mut results);
    let mut failures = Vec::new();
    let mut by_domain: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    for (domain, method) in wire_methods().into_iter().filter(|(d, _)| keep(d)) {
        match vector(&method_schema, &results, method) {
            Ok(row) => by_domain.entry(domain).or_default().push(row),
            Err(error) => failures.push(error),
        }
    }
    if failures.is_empty() {
        Ok(by_domain)
    } else {
        Err(failures)
    }
}

/// The vectors of every served wire method, grouped by domain and sorted by method id, from
/// a fresh build, or one failure line per method id.
pub fn build_vectors() -> Result<BTreeMap<&'static str, Vec<Value>>, Vec<String>> {
    build_vectors_where(|_| true)
}
