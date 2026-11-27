//! MEDULLA-DURABLE-KERNEL-R042.100.2 -- `bench-claim admit` refuses each missing
//! or invalid measurement field with its stable code, through the real binary.

use medulla_mutation_types::benchmark_claim::{
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
    if let Some(path) = option_env!("CARGO_BIN_EXE_medulla-server") {
        return path.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .join("medulla-server")
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
    cmd.arg("bench-claim").arg("admit").arg(&mpath);
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
#[path = "../benches/common/latency_receipt.rs"]
mod latency_receipt;
#[allow(dead_code)]
#[path = "../benches/common/latency_stats.rs"]
mod latency_stats;
#[allow(dead_code)]
#[path = "../benches/common/receipt_fields.rs"]
mod receipt_fields;
#[allow(dead_code)]
#[path = "../benches/common/shape_receipt.rs"]
mod shape_receipt;
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

// spec: MEDULLA-DURABLE-KERNEL-R042.2
#[test]
fn receipt_carries_configuration_corpus_digest_p50_p99() {
    use latency_receipt::{record_target, write_cross_modal_receipt, CrossModalCase};
    use receipt_fields::RECEIPT_FIELD_MISSING;

    let case = |samples: Vec<f64>| CrossModalCase {
        case: "crossmodal_match_traverse_rank".into(),
        configuration_digest: sha256_hex(b"1000000/10000000/1000000/384/seed-7"),
        corpus_digest: sha256_hex(b"corpus"),
        claims: 1_000_000,
        edges: 10_000_000,
        vectors: 1_000_000,
        dimensions: 384,
        p50_budget_ms: 150.0,
        p99_budget_ms: 500.0,
        // 100 ascending samples: p50 is sample 50, p99 is sample 99.
        samples_ms: samples,
    };
    let ascending = |top: f64| {
        (1..=100)
            .map(|i| top * i as f64 / 100.0)
            .collect::<Vec<_>>()
    };
    let root = tempfile::tempdir().unwrap();
    let held = case(ascending(100.0));
    let path = write_cross_modal_receipt(root.path(), "3.3.0", &held).expect("write receipt");
    assert_eq!(
        path,
        root.path()
            .join("3.3.0")
            .join("crossmodal_match_traverse_rank.json")
    );
    let receipt: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(receipt["configuration_digest"], held.configuration_digest);
    assert_eq!(receipt["corpus_digest"], held.corpus_digest);
    assert_eq!(receipt["corpus"]["claims"], 1_000_000);
    assert_eq!(receipt["corpus"]["edges"], 10_000_000);
    assert_eq!(receipt["corpus"]["vectors"], 1_000_000);
    assert_eq!(receipt["corpus"]["dimensions"], 384);
    assert!(receipt["hardware"]["logical_cpus"].is_u64());
    assert!(receipt["p50_ms"].is_f64() && receipt["p99_ms"].is_f64());
    assert!(receipt["p50_ms"].as_f64() <= receipt["p99_ms"].as_f64());
    assert_eq!(receipt["within_budget"], true);

    // A p99 past its budget is reported as missed, never as held.
    assert!(!case(ascending(900.0)).within_budget());

    // A receipt without a digest or without samples is refused with its code.
    for bad in [
        CrossModalCase {
            corpus_digest: String::new(),
            ..case(ascending(1.0))
        },
        CrossModalCase {
            configuration_digest: String::new(),
            ..case(ascending(1.0))
        },
        case(vec![]),
    ] {
        let err = write_cross_modal_receipt(root.path(), "3.3.0", &bad).unwrap_err();
        assert!(err.to_string().starts_with(RECEIPT_FIELD_MISSING), "{err}");
    }

    // The first run sets the target to 1.2 times its p99 and later runs keep it.
    let targets = record_target(&root.path().join("receipts"), "crossmodal", 100.0).unwrap();
    record_target(&root.path().join("receipts"), "crossmodal", 999.0).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(targets).unwrap()).unwrap();
    assert!((v["crossmodal"].as_f64().unwrap() - 120.0).abs() < 1e-9);
}

// spec: MEDULLA-DURABLE-KERNEL-R064
#[test]
fn shape_receipt_requires_both_variants() {
    use shape_receipt::{write_shape_receipt, ShapeBenchCase, SHAPE_RECEIPT_VARIANT_MISSING};

    let case = |memoized: Vec<f64>, parse: Vec<f64>| ShapeBenchCase {
        writes: 1_000,
        shapes: 50,
        profile: "release".into(),
        memoized_ms: memoized,
        parse_per_write_ms: parse,
    };
    let root = tempfile::tempdir().unwrap();

    // Either variant missing refuses with the variant code and writes nothing.
    for bad in [case(vec![], vec![1.0]), case(vec![1.0], vec![])] {
        let err = write_shape_receipt(root.path(), "3.3.0", &bad).unwrap_err();
        assert!(
            err.to_string().starts_with(SHAPE_RECEIPT_VARIANT_MISSING),
            "{err}"
        );
    }
    assert!(!root.path().join("3.3.0").exists());

    // A 30 percent p50 win is adopted; a 10 percent win is not.
    let win = case(vec![0.7; 10], vec![1.0; 10]);
    let path = write_shape_receipt(root.path(), "3.3.0", &win).expect("write receipt");
    let receipt: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(receipt["case"], "shape_validation");
    assert_eq!(receipt["writes"], 1_000);
    assert_eq!(receipt["shapes"], 50);
    assert_eq!(receipt["profile"], "release");
    assert!(receipt["hardware"]["arch"].is_string());
    assert_eq!(receipt["memoized"]["sample_count"], 10);
    assert_eq!(receipt["parse_per_write"]["sample_count"], 10);
    assert_eq!(receipt["adopt_memoized"], true);
    assert!(!case(vec![0.9; 10], vec![1.0; 10]).adopt_memoized());
}

/// Write one receipt of `case` under `<root>/<release>/`.
fn receipt(root: &Path, release: &str, case: &str, digest: &str, p50: f64, p99: f64) {
    let dir = root.join(release);
    std::fs::create_dir_all(&dir).unwrap();
    let body = serde_json::json!({
        "configuration_digest": digest,
        "p50_ms": p50,
        "p99_ms": p99,
    });
    std::fs::write(dir.join(format!("{case}.json")), body.to_string()).unwrap();
}

/// Run `compare`; returns (exit success, stdout).
fn compare(root: &Path, release: &str) -> (bool, String) {
    let out = Command::new(bin())
        .arg("bench-claim")
        .arg("compare")
        .arg(root)
        .arg(release)
        .output()
        .expect("spawn bench-claim compare");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

// spec: MEDULLA-DURABLE-KERNEL-R042.6
#[test]
fn regression_over_ten_percent_fails_and_missing_baseline_is_created() {
    let root = tempfile::tempdir().unwrap();
    let digest = sha256_hex(b"configuration");

    // The first release has no previous receipt: it passes and logs the baseline.
    receipt(root.path(), "3.2.0", "ycsb-a", &digest, 100.0, 200.0);
    let (ok, out) = compare(root.path(), "3.2.0");
    assert!(ok, "a missing baseline must pass once: {out}");
    assert!(out.contains("BENCH_BASELINE_CREATED ycsb-a"), "{out}");

    // Exactly 10 percent slower still passes; both values are within the limit.
    receipt(root.path(), "3.3.0", "ycsb-a", &digest, 110.0, 220.0);
    let (ok, out) = compare(root.path(), "3.3.0");
    assert!(ok, "a 10 percent change is not a regression: {out}");
    assert!(!out.contains("BENCH_REGRESSION"), "{out}");

    // More than 10 percent slower on either percentile fails, naming the case.
    receipt(root.path(), "3.4.0", "ycsb-a", &digest, 110.0, 245.0);
    let (ok, out) = compare(root.path(), "3.4.0");
    assert!(!ok, "a 22.5 percent p99 regression must fail: {out}");
    assert!(out.contains("BENCH_REGRESSION ycsb-a p99_ms"), "{out}");

    // A case measured under another configuration has no comparable baseline.
    receipt(
        root.path(),
        "3.5.0",
        "ycsb-a",
        &sha256_hex(b"another"),
        900.0,
        900.0,
    );
    let (ok, out) = compare(root.path(), "3.5.0");
    assert!(
        ok,
        "another configuration is incomparable, not a regression: {out}"
    );
    assert!(out.contains("BENCH_BASELINE_CREATED ycsb-a"), "{out}");
}
