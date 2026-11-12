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
