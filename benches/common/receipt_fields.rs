//! The fields and the refusal every cross-modal and shape receipt shares. Included by
//! `#[path]` next to the receipt shape that reports them.

use std::io;

/// Refusal code: a receipt with an empty digest, case or sample set is never written.
pub const RECEIPT_FIELD_MISSING: &str = "BENCH_RECEIPT_FIELD_MISSING";

pub fn refuse(code: &str, what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{code}: {what}"))
}

/// Hardware and build identity every receipt carries.
pub fn hardware_identity() -> serde_json::Value {
    serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "logical_cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
    })
}
