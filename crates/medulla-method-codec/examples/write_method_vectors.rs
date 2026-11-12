//! Write the committed method-body vectors for the Go and JavaScript clients.
//!
//! `scripts/build_method_codec_wasm.py --write-vectors` runs this example
//! (MEDULLA-CONTRACT-R004). Each vector pairs the exact request frame a
//! connector sends with the canonical body [`canonical_body`] re-derives from
//! it: the Go test replays the frame bytes through the embedded module, the
//! JavaScript test replays the `params` object through the same module, so a
//! client that restates canonicalisation instead of delegating to this crate
//! fails by name.
//!
//! A vector is omitted when its `params` is not a JSON object or carries a
//! value `JSON.parse` cannot hold exactly (unsigned 64-bit integers at or
//! above 2**53): the JavaScript test re-encodes `params`.

use std::env;
use std::path::Path;
use std::process::ExitCode;

use medulla_method_codec::{canonical_body, CODEC};
use medulla_protocol_types::protocol_fixtures::method_bodies::{
    typed_request_frame, typed_samples,
};

fn main() -> ExitCode {
    let Some(output) = env::args().nth(1) else {
        eprintln!("usage: write_method_vectors <output.json>");
        return ExitCode::FAILURE;
    };
    let mut vectors = Vec::new();
    let mut omitted = 0u32;
    for (name, method) in typed_samples() {
        let frame = typed_request_frame(method);
        let Ok(body) = canonical_body(&frame) else {
            eprintln!("omitted {name}: the frame does not decode");
            omitted += 1;
            continue;
        };
        // The frame the transport carries is plain MessagePack; mirror it as
        // JSON so the clients replay the same `params` value.
        let Ok(mirror) = rmp_serde::from_slice::<serde_json::Value>(&frame) else {
            eprintln!("omitted {name}: the frame is not JSON-compatible");
            omitted += 1;
            continue;
        };
        let Some(method_name) = mirror.get("method").and_then(|value| value.as_str()) else {
            eprintln!("omitted {name}: the frame lacks its method name");
            omitted += 1;
            continue;
        };
        let Some(params) = mirror.get("params").filter(|value| value.is_object()) else {
            eprintln!("omitted {name}: the method carries no replayable params");
            omitted += 1;
            continue;
        };
        if !json_safe(params) {
            eprintln!("omitted {name}: params exceed JavaScript number precision");
            omitted += 1;
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
    if vectors.is_empty() {
        eprintln!("REFUSED: no vector qualified");
        return ExitCode::FAILURE;
    }
    let file = serde_json::json!({
        "codec": CODEC,
        "vectors": vectors,
    });
    if let Some(parent) = Path::new(&output).parent() {
        std::fs::create_dir_all(parent).expect("the vector directory creates");
    }
    std::fs::write(
        &output,
        serde_json::to_string_pretty(&file).expect("the vectors serialise") + "\n",
    )
    .expect("the vector file writes");
    println!(
        "wrote {output}: {} vectors ({} omitted)",
        vectors.len(),
        omitted
    );
    ExitCode::SUCCESS
}

/// `JSON.parse` holds only integers inside 2**53 and finite floats exactly.
fn json_safe(value: &serde_json::Value) -> bool {
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
