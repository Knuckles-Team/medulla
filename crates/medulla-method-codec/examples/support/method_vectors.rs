//! The method-body vector builder shared by the `write_method_vectors` example and the
//! `emit_method_body_vectors` test (MEDULLA-CONTRACT-R061). Both include it through `#[path]`.
//!
//! One vector per entry of `typed_samples()`, keyed by its sample `name`, in that order. Each
//! vector pairs the exact request frame a connector sends with the canonical body
//! [`canonical_body`] re-derives from it: the Go test replays the frame bytes through the
//! embedded module, the JavaScript test replays the `params` object through the same module,
//! so a client that restates canonicalisation instead of delegating to this crate fails by
//! name. A vector is omitted, with its reason, when its frame does not decode, its `params` is
//! not a JSON object, or it carries a value `JSON.parse` cannot hold exactly (unsigned 64-bit
//! integers at or above 2**53). The output is a pure function of the codec and the samples.
#![allow(dead_code)]

use medulla_method_codec::canonical_body;
use medulla_protocol_types::protocol_fixtures::method_bodies::{
    typed_request_frame, typed_samples,
};

/// The vectors, and the omitted sample names with the reason each was omitted.
pub fn method_body_vectors() -> (Vec<serde_json::Value>, Vec<(String, &'static str)>) {
    let mut vectors = Vec::new();
    let mut omitted: Vec<(String, &'static str)> = Vec::new();
    for (name, method) in typed_samples() {
        let name = name.to_string();
        let frame = typed_request_frame(method);
        let Ok(body) = canonical_body(&frame) else {
            omitted.push((name, "the frame does not decode"));
            continue;
        };
        // The frame the transport carries is plain MessagePack; mirror it as
        // JSON so the clients replay the same `params` value.
        let Ok(mirror) = rmp_serde::from_slice::<serde_json::Value>(&frame) else {
            omitted.push((name, "the frame is not JSON-compatible"));
            continue;
        };
        let Some(method_name) = mirror.get("method").and_then(|value| value.as_str()) else {
            omitted.push((name, "the frame lacks its method name"));
            continue;
        };
        let Some(params) = mirror.get("params").filter(|value| value.is_object()) else {
            omitted.push((name, "the method carries no replayable params"));
            continue;
        };
        if !json_safe(params) {
            omitted.push((name, "params exceed JavaScript number precision"));
            continue;
        }
        vectors.push(serde_json::json!({
            "name": name,
            "method": method_name,
            "params": params,
            "frame": hex(&frame),
            "body": hex(&body),
        }));
    }
    (vectors, omitted)
}

/// `JSON.parse` holds only integers inside 2**53 and finite floats exactly.
pub fn json_safe(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(_) | serde_json::Value::String(_) | serde_json::Value::Null => true,
        serde_json::Value::Number(number) => {
            number.as_u64().is_some_and(|integer| integer < 1u64 << 53)
                || number
                    .as_i64()
                    .is_some_and(|integer| integer >= -(1i64 << 53))
                || number.as_f64().is_some_and(f64::is_finite)
        }
        serde_json::Value::Array(items) => items.iter().all(json_safe),
        serde_json::Value::Object(map) => map.values().all(json_safe),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
