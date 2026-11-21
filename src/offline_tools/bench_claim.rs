//! `medulla-server bench-claim admit` (MEDULLA-DURABLE-KERNEL-R042.100): admit a
//! benchmark measurement or print the refusal code and exit non-zero.
//!
//! Usage: `medulla-server bench-claim admit <measurement.json> --price-table <table.json>
//! [--against <other-measurement.json>]`.
//!
//! `medulla-server bench-claim compare <receipts-root> <release> [--previous <release>]`
//! (MEDULLA-DURABLE-KERNEL-R042.6) compares the receipts of `<release>` with those of
//! the previous release under the same root. See [`compare`].

use medulla_mutation_types::benchmark_claim::{
    BenchmarkMeasurement, MeasurementRefusal, PriceTable,
};

fn read_measurement(path: &str) -> Result<BenchmarkMeasurement, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("cannot parse {path}: {e}"))
}

fn admit(args: &[String]) -> Result<String, MeasurementRefusal> {
    let mut path = None;
    let mut table_path = None;
    let mut against = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--price-table" => table_path = it.next(),
            "--against" => against = it.next(),
            other => path = Some(other.to_string()),
        }
    }
    let path = path.ok_or(MeasurementRefusal::TokensMissing)?;
    let measurement = read_measurement(&path).map_err(|e| {
        eprintln!("{e}");
        MeasurementRefusal::TokensMissing
    })?;
    measurement.validate()?;
    let (table, digest) = match table_path.map(std::fs::read_to_string) {
        Some(Ok(text)) => PriceTable::load(&text)?,
        _ => return Err(MeasurementRefusal::PriceTableMissing),
    };
    if digest != measurement.price_table_digest {
        return Err(MeasurementRefusal::PriceTableMissing);
    }
    if table.minor_unit_exponent != measurement.minor_unit_exponent {
        return Err(MeasurementRefusal::CurrencyExponentInvalid);
    }
    if let Some(other) = against {
        let other = read_measurement(other).map_err(|e| {
            eprintln!("{e}");
            MeasurementRefusal::ProfileIncomparable
        })?;
        measurement.comparable_with(&other)?;
    }
    Ok(format!(
        "admitted tokens_per_second={} cost_minor_units={} currency={} price_table_digest={}",
        measurement.throughput_tokens_per_second(),
        measurement.cost_minor_units(&table),
        table.currency,
        digest
    ))
}

/// The refusal printed when a measured value regresses by more than the allowed share.
const BENCH_REGRESSION: &str = "BENCH_REGRESSION";
/// The line printed, once, for a case that has no previous receipt to compare with.
const BENCH_BASELINE_CREATED: &str = "BENCH_BASELINE_CREATED";
/// A measured value may grow by this share of the previous release's value.
const REGRESSION_LIMIT: f64 = 0.10;

/// A release directory name as a comparable version, `None` for a name that is not
/// dotted numbers.
fn release_version(name: &str) -> Option<Vec<u64>> {
    name.split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

/// The greatest release under `root` that is older than `release`.
fn previous_release(root: &std::path::Path, release: &str) -> Option<String> {
    let current = release_version(release)?;
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| release_version(&name).map(|version| (version, name)))
        .filter(|(version, _)| *version < current)
        .max()
        .map(|(_, name)| name)
}

fn read_receipt(path: &std::path::Path) -> Result<serde_json::Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

/// Compare every receipt of `release` with the same case of the previous release.
/// Answers the lines to print and whether any value regressed: p50 and p99 of one
/// case and one configuration digest may grow by at most 10 percent. A case with no
/// previous receipt, or whose previous receipt has another configuration digest,
/// has no baseline: it passes and prints `BENCH_BASELINE_CREATED`.
fn compare(args: &[String]) -> Result<(Vec<String>, bool), String> {
    let mut positional = Vec::new();
    let mut previous = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--previous" => previous = it.next().cloned(),
            other => positional.push(other.to_string()),
        }
    }
    let [root, release] = positional.as_slice() else {
        return Err("compare needs <receipts-root> <release>".to_string());
    };
    let root = std::path::Path::new(root);
    let previous = previous.or_else(|| previous_release(root, release));
    let mut cases: Vec<_> = std::fs::read_dir(root.join(release))
        .map_err(|e| format!("cannot read the receipts of {release}: {e}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    cases.sort();
    let mut lines = Vec::new();
    let mut regressed = false;
    for path in cases {
        let case = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let current = read_receipt(&path)?;
        let baseline = previous
            .as_ref()
            .map(|previous| root.join(previous).join(format!("{case}.json")))
            .filter(|path| path.is_file())
            .map(|path| read_receipt(&path))
            .transpose()?
            .filter(|baseline| {
                baseline.get("configuration_digest") == current.get("configuration_digest")
            });
        let Some(baseline) = baseline else {
            lines.push(format!("{BENCH_BASELINE_CREATED} {case}"));
            continue;
        };
        for key in ["p50_ms", "p99_ms"] {
            let (Some(now), Some(then)) = (
                current.get(key).and_then(serde_json::Value::as_f64),
                baseline.get(key).and_then(serde_json::Value::as_f64),
            ) else {
                continue;
            };
            if now > then * (1.0 + REGRESSION_LIMIT) {
                regressed = true;
                lines.push(format!("{BENCH_REGRESSION} {case} {key} {then} -> {now}"));
            }
        }
    }
    Ok((lines, regressed))
}

/// `argv[0]` is the subcommand name; the rest are the tool's arguments.
pub(crate) fn run(argv: Vec<std::ffi::OsString>) {
    let args: Vec<String> = argv
        .into_iter()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.first().map(String::as_str) == Some("compare") {
        match compare(&args[1..]) {
            Ok((lines, regressed)) => {
                for line in lines {
                    println!("{line}");
                }
                std::process::exit(i32::from(regressed));
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    }
    if args.first().map(String::as_str) != Some("admit") {
        eprintln!("usage: medulla-server bench-claim admit <measurement.json> --price-table <table.json> [--against <other.json>]");
        eprintln!("       medulla-server bench-claim compare <receipts-root> <release> [--previous <release>]");
        std::process::exit(2);
    }
    match admit(&args[1..]) {
        Ok(line) => println!("{line}"),
        Err(refusal) => {
            println!("{}", refusal.code());
            std::process::exit(1);
        }
    }
}
