//! Rust round-trip vectors for every wire method (MEDULLA-CONTRACT-R001.3).
//!
//! No sample is hand-written: `medulla_capabilities::wire_vectors` derives each request from
//! the `Method` variant's schema and each result from the schema of the body that
//! `medulla_protocol_types::result_contract` registers for the method. No vector file is
//! committed and none is written (owner rule: no committed generated artifacts, and no
//! rewrite switch): each domain test rebuilds its samples in-process and checks them against
//! an independent second build, the domain's method set and each row's canonical digest. The
//! `emit_wire_vectors` example prints the same rows for an external client.
//!
//! Run: `cargo test -p medulla-capabilities --test wire_vectors`.
#![cfg(feature = "contract-schema")]

use std::collections::BTreeMap;

use medulla_capabilities::wire_vectors::{build_vectors, served_methods};
use medulla_types::contract::Digest256;
use serde_json::{json, Value};

/// The vectors of every wire method, grouped by domain and sorted by method id.
fn all_vectors() -> &'static BTreeMap<&'static str, Vec<Value>> {
    static VECTORS: std::sync::OnceLock<BTreeMap<&'static str, Vec<Value>>> =
        std::sync::OnceLock::new();
    VECTORS.get_or_init(|| {
        build_vectors().unwrap_or_else(|failures| {
            panic!(
                "{} wire method(s) have no round-trip vector:\n{}",
                failures.len(),
                failures.join("\n")
            )
        })
    })
}

/// One wire-callable method: published to a consumer profile and not internal.
fn wire_methods() -> Vec<(&'static str, &'static str)> {
    served_methods()
}

/// The method ids of `rows`, in row order.
fn row_methods(rows: &[Value]) -> Vec<&str> {
    rows.iter()
        .filter_map(|row| row["method"].as_str())
        .collect()
}

/// The digest a row carries must be the digest of its own method, request and result.
fn digest_mismatch(row: &Value) -> Option<String> {
    let body = json!({"method": row["method"], "request": row["request"], "result": row["result"]});
    let digest = Digest256::sha256(body.to_string().as_bytes()).to_hex();
    (row["sha256"].as_str() != Some(digest.as_str()))
        .then(|| format!("{}: digest does not match its row", row["method"]))
}

/// Check one domain's rows in-process: the domain's method set, a second independent build,
/// and each row's digest. Every failure names the method id.
fn check_domain(domain: &str) {
    let vectors = all_vectors();
    let rows = vectors
        .get(domain)
        .unwrap_or_else(|| panic!("domain {domain} has no wire-callable method"));
    let expected: Vec<&str> = wire_methods()
        .into_iter()
        .filter(|(d, _)| *d == domain)
        .map(|(_, method)| method)
        .collect();
    let built = row_methods(rows);
    let mut failures: Vec<String> = expected
        .iter()
        .filter(|method| !built.contains(method))
        .map(|method| format!("{method}: no vector for this {domain} method"))
        .collect();
    failures.extend(
        built
            .iter()
            .filter(|method| !expected.contains(method))
            .map(|method| format!("{method}: vector names no {domain} method")),
    );
    let fresh = build_vectors().unwrap_or_else(|failures| {
        panic!("{domain}: second build failed:\n{}", failures.join("\n"))
    });
    let again = fresh.get(domain).map(Vec::as_slice).unwrap_or_default();
    failures.extend(
        rows.iter()
            .filter(|row| !again.contains(row))
            .map(|row| format!("{}: vector differs between two builds", row["method"])),
    );
    failures.extend(rows.iter().filter_map(digest_mismatch));
    assert!(failures.is_empty(), "{domain}:\n{}", failures.join("\n"));
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
    for (domain, rows) in vectors.iter() {
        let methods: Vec<&str> = rows
            .iter()
            .filter_map(|row| row["method"].as_str())
            .collect();
        let mut sorted = methods.clone();
        sorted.sort();
        assert_eq!(methods, sorted, "{domain}: rows are sorted by method id");
        check_domain(domain);
    }
}

/// Verifies MEDULLA-CONTRACT-R001.3.1: Wire vectors for the cluster method domain
// spec: MEDULLA-CONTRACT-R001.3.1
#[test]
fn domain_cluster_vectors_round_trip() {
    check_domain("cluster");
}

/// Verifies MEDULLA-CONTRACT-R001.3.2: Wire vectors for the compute method domain
// spec: MEDULLA-CONTRACT-R001.3.2
#[test]
fn domain_compute_vectors_round_trip() {
    check_domain("compute");
}

/// Verifies MEDULLA-CONTRACT-R001.3.3: Wire vectors for the coordination method domain
// spec: MEDULLA-CONTRACT-R001.3.3
#[test]
fn domain_coordination_vectors_round_trip() {
    check_domain("coordination");
}

/// Verifies MEDULLA-CONTRACT-R001.3.4: Wire vectors for the graph method domain
// spec: MEDULLA-CONTRACT-R001.3.4
#[test]
fn domain_graph_vectors_round_trip() {
    check_domain("graph");
}

/// Verifies MEDULLA-CONTRACT-R001.3.5: Wire vectors for the ingestion method domain
// spec: MEDULLA-CONTRACT-R001.3.5
#[test]
fn domain_ingestion_vectors_round_trip() {
    check_domain("ingestion");
}

/// Verifies MEDULLA-CONTRACT-R001.3.6: Wire vectors for the messaging method domain
// spec: MEDULLA-CONTRACT-R001.3.6
#[test]
fn domain_messaging_vectors_round_trip() {
    check_domain("messaging");
}

/// Verifies MEDULLA-CONTRACT-R001.3.7: Wire vectors for the query method domain
// spec: MEDULLA-CONTRACT-R001.3.7
#[test]
fn domain_query_vectors_round_trip() {
    check_domain("query");
}

/// Verifies MEDULLA-CONTRACT-R001.3.8: Wire vectors for the reasoning method domain
// spec: MEDULLA-CONTRACT-R001.3.8
#[test]
fn domain_reasoning_vectors_round_trip() {
    check_domain("reasoning");
}

/// Verifies MEDULLA-CONTRACT-R001.3.9: Wire vectors for the security method domain
// spec: MEDULLA-CONTRACT-R001.3.9
#[test]
fn domain_security_vectors_round_trip() {
    check_domain("security");
}

/// Verifies MEDULLA-CONTRACT-R001.3.10: Wire vectors for the storage method domain
// spec: MEDULLA-CONTRACT-R001.3.10
#[test]
fn domain_storage_vectors_round_trip() {
    check_domain("storage");
}

/// Verifies MEDULLA-CONTRACT-R001.3.11: Wire vectors for the transactions method domain
// spec: MEDULLA-CONTRACT-R001.3.11
#[test]
fn domain_transactions_vectors_round_trip() {
    check_domain("transactions");
}
