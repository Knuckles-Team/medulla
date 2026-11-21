//! Print the wire vectors of every served method as one JSON document
//! (MEDULLA-CONTRACT-R001.4.1).
//!
//! ```text
//! {"domains": {<domain>: [<row>, ...]}, "served": [<method id>, ...]}
//! ```
//!
//! Keys are sorted and the document ends with one line feed. This is the only
//! producer of wire vectors: nothing is written into the repository. The process
//! exits 1, with one failure line per method on stderr, when a vector cannot be
//! built.
//!
//! Run: `cargo run --locked --quiet -p medulla-capabilities --features
//! contract-schema,canonical-ledger --example emit_wire_vectors`.

use std::collections::BTreeMap;
use std::process::ExitCode;

use medulla_capabilities::wire_vectors::{build_vectors, served_methods};

fn main() -> ExitCode {
    let domains = match build_vectors() {
        Ok(domains) => domains,
        Err(failures) => {
            for failure in failures {
                eprintln!("{failure}");
            }
            return ExitCode::FAILURE;
        }
    };
    let mut served: Vec<&str> = served_methods().into_iter().map(|(_, id)| id).collect();
    served.sort_unstable();
    let mut document = BTreeMap::new();
    document.insert("domains", serde_json::json!(domains));
    document.insert("served", serde_json::json!(served));
    match serde_json::to_string(&document) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("the vector document does not serialize: {error}");
            ExitCode::FAILURE
        }
    }
}
