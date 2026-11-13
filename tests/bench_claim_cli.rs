//! MEDULLA-DURABLE-KERNEL-R042.100.2 -- `bench-claim admit` refuses each missing
//! or invalid measurement field with its stable code, through the real binary.

use medulla_types::benchmark_claim::{
    sha256_hex, BenchmarkMeasurement, ResourceKind, MEASUREMENT_CURRENCY_EXPONENT_INVALID,
    MEASUREMENT_ELAPSED_NOT_POSITIVE, MEASUREMENT_PRICE_TABLE_MISSING,
    MEASUREMENT_PROFILE_INCOMPARABLE, MEASUREMENT_SCHEMA_VERSION, MEASUREMENT_TOKENIZER_MISSING,
    MEASUREMENT_TOKENS_MISSING,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const TABLE: &str = r#"{"version":1,"currency":"USD","minor_unit_exponent":2,"prices":{"cpu_ms":{"unit":"ms","unit_price":500000}}}"#;

fn bin() -> std::path::PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_bench-claim") {
        return path.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .join("bench-claim")
}

fn good() -> BenchmarkMeasurement {
    let d = |c: char| c.to_string().repeat(64);
    BenchmarkMeasurement {
        schema_version: MEASUREMENT_SCHEMA_VERSION,
        benchmark_case: "ycsb-a".into(),
        workload: "mixed".into(),
        profile: "gpu-1".into(),
        configuration_digest: d('a'),
        dataset_digest: d('b'),
        query_digest: d('c'),
        release_commit: "0123abc".into(),
        hardware_digest: d('d'),
        model: "m".into(),
        provider: "p".into(),
        tokenizer: "tok-1".into(),
        price_table_digest: sha256_hex(TABLE.as_bytes()),
        minor_unit_exponent: 2,
        request_start_ms: 0,
        request_end_ms: 4_100,
        model_ready_ms: 50,
        inference_elapsed_ms: 4_000,
        full_input_tokens: 12_000,
        usage: BTreeMap::from([(ResourceKind::CpuMs, 10)]),
    }
}

/// Run `admit`; returns (exit success, stdout).
fn admit(
    dir: &Path,
    m: &BenchmarkMeasurement,
    table: Option<&str>,
    against: Option<&BenchmarkMeasurement>,
) -> (bool, String) {
    let mpath = dir.join("m.json");
    std::fs::write(&mpath, serde_json::to_string(m).unwrap()).unwrap();
    let mut cmd = Command::new(bin());
    cmd.arg("admit").arg(&mpath);
    if let Some(t) = table {
        let tpath = dir.join("t.json");
        std::fs::write(&tpath, t).unwrap();
        cmd.arg("--price-table").arg(&tpath);
    }
    if let Some(o) = against {
        let opath = dir.join("o.json");
        std::fs::write(&opath, serde_json::to_string(o).unwrap()).unwrap();
        cmd.arg("--against").arg(&opath);
    }
    let out = cmd.output().expect("spawn bench-claim");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

// spec: MEDULLA-DURABLE-KERNEL-R042.100.2
#[test]
fn each_missing_or_invalid_field_is_refused_with_its_code() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();

    let (ok, out) = admit(dir, &good(), Some(TABLE), None);
    assert!(ok, "a complete measurement is admitted: {out}");
    // 12_000 * 1000 / 4_000 tokens per second; 10 cpu_ms at 0.5 minor units.
    assert!(out.contains("tokens_per_second=3000"), "{out}");
    assert!(out.contains("cost_minor_units=5"), "{out}");

    let mut m = good();
    m.full_input_tokens = 0;
    assert_eq!(
        admit(dir, &m, Some(TABLE), None),
        (false, MEASUREMENT_TOKENS_MISSING.into())
    );

    let mut m = good();
    m.tokenizer = "  ".into();
    assert_eq!(
        admit(dir, &m, Some(TABLE), None),
        (false, MEASUREMENT_TOKENIZER_MISSING.into())
    );

    // No price table supplied, and a table whose digest differs from the named one.
    assert_eq!(
        admit(dir, &good(), None, None),
        (false, MEASUREMENT_PRICE_TABLE_MISSING.into())
    );
    let other = TABLE.replace("500000", "400000");
    assert_eq!(
        admit(dir, &good(), Some(&other), None),
        (false, MEASUREMENT_PRICE_TABLE_MISSING.into())
    );
    let mut m = good();
    m.price_table_digest = String::new();
    assert_eq!(
        admit(dir, &m, Some(TABLE), None),
        (false, MEASUREMENT_PRICE_TABLE_MISSING.into())
    );

    let mut m = good();
    m.inference_elapsed_ms = 0;
    assert_eq!(
        admit(dir, &m, Some(TABLE), None),
        (false, MEASUREMENT_ELAPSED_NOT_POSITIVE.into())
    );

    let mut m = good();
    m.minor_unit_exponent = 7;
    assert_eq!(
        admit(dir, &m, Some(TABLE), None),
        (false, MEASUREMENT_CURRENCY_EXPONENT_INVALID.into())
    );
    let bad_table = TABLE.replace("\"minor_unit_exponent\":2", "\"minor_unit_exponent\":9");
    let mut m = good();
    m.price_table_digest = sha256_hex(bad_table.as_bytes());
    assert_eq!(
        admit(dir, &m, Some(&bad_table), None),
        (false, MEASUREMENT_CURRENCY_EXPONENT_INVALID.into())
    );

    // Comparable on benchmark, workload and profile; incomparable otherwise.
    let (ok, _) = admit(dir, &good(), Some(TABLE), Some(&good()));
    assert!(ok);
    for mutate in [
        (|m: &mut BenchmarkMeasurement| m.benchmark_case = "tpcc".into()) as fn(&mut _),
        |m| m.workload = "read".into(),
        |m| m.profile = "cpu-only".into(),
    ] {
        let mut o = good();
        mutate(&mut o);
        assert_eq!(
            admit(dir, &good(), Some(TABLE), Some(&o)),
            (false, MEASUREMENT_PROFILE_INCOMPARABLE.into())
        );
    }
}

// The wire benchmark's receipt writer and summary, included from the bench tree
// so the benchmark and this proof share one definition.
#[allow(dead_code)]
#[path = "../benches/common/claim.rs"]
mod claim;
#[allow(dead_code)]
#[path = "../benches/common/wire_receipt.rs"]
mod wire_receipt;

// spec: MEDULLA-DURABLE-KERNEL-R042.5
#[test]
fn one_receipt_per_protocol_with_session_count() {
    use wire_receipt::{write_wire_receipts, WireCase};

    let case = |protocol: &'static str, budget: f64, base: f64| WireCase {
        protocol,
        operation: "point read",
        sessions: 64,
        p99_budget_ms: budget,
        // 100 ascending samples: p50 is sample 50, p99 is sample 99.
        samples_ms: (1..=100).map(|i| base * i as f64 / 100.0).collect(),
    };
    let cases = [
        case("pgwire", 2.0, 1.0),
        case("bolt", 3.0, 1.0),
        case("resp", 1.0, 1.0),
        case("s3", 10.0, 20.0),
    ];
    let root = tempfile::tempdir().unwrap();
    let paths = write_wire_receipts(root.path(), "3.3.0", &cases).expect("write receipts");
    assert_eq!(paths.len(), 4, "one receipt per protocol");

    for (path, name) in paths.iter().zip(["pgwire", "bolt", "resp", "s3"]) {
        assert_eq!(
            path,
            &root.path().join("3.3.0").join(format!("{name}.json"))
        );
        let receipt: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(receipt["case"], name);
        assert_eq!(
            receipt["session_count"], 64,
            "{name} records the session count"
        );
        assert!(receipt["p50_ms"].is_f64() && receipt["p99_ms"].is_f64());
        assert!(receipt["p50_ms"].as_f64() <= receipt["p99_ms"].as_f64());
    }
    // The s3 case misses its 10 ms budget and says so; the others hold.
    let verdicts: Vec<bool> = cases.iter().map(WireCase::within_budget).collect();
    assert_eq!(verdicts, vec![true, true, true, false]);

    // A receipt path segment that would leave the root is refused.
    assert!(claim::write_receipt(root.path(), "..", "x", &serde_json::json!({})).is_err());
    assert!(claim::write_receipt(root.path(), "3.3.0", "a/b", &serde_json::json!({})).is_err());
    // A case with no samples never reports a held budget.
    let empty = WireCase {
        samples_ms: vec![],
        ..case("pgwire", 2.0, 1.0)
    };
    assert!(!empty.within_budget());
}
