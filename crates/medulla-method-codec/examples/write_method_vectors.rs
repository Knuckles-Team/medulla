//! Write the method-body vectors for the Go and JavaScript clients.
//!
//! `scripts/build_method_codec_wasm.py` runs this example on every build that writes the
//! codec (MEDULLA-CONTRACT-R061, R004) and places the output beside both client copies of
//! the module. The vectors are never committed. The builder is
//! `examples/support/method_vectors.rs`, shared with `tests/emit_method_body_vectors.rs`.

use std::env;
use std::path::Path;
use std::process::ExitCode;

use medulla_method_codec::CODEC;

#[path = "support/method_vectors.rs"]
mod method_vectors;

fn main() -> ExitCode {
    let Some(output) = env::args().nth(1) else {
        eprintln!("usage: write_method_vectors <output.json>");
        return ExitCode::FAILURE;
    };
    let (vectors, omitted) = method_vectors::method_body_vectors();
    for (name, reason) in &omitted {
        eprintln!("omitted {name}: {reason}");
    }
    if vectors.is_empty() {
        eprintln!("REFUSED: no vector qualified");
        return ExitCode::FAILURE;
    }
    let count = vectors.len();
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
        "wrote {output}: {count} vectors ({} omitted)",
        omitted.len()
    );
    ExitCode::SUCCESS
}
