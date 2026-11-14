//! `medulla-server bench-claim admit` (MEDULLA-DURABLE-KERNEL-R042.100): admit a
//! benchmark measurement or print the refusal code and exit non-zero.
//!
//! Usage: `medulla-server bench-claim admit <measurement.json> --price-table <table.json>
//! [--against <other-measurement.json>]`.

use medulla_types::benchmark_claim::{BenchmarkMeasurement, MeasurementRefusal, PriceTable};

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

/// `argv[0]` is the subcommand name; the rest are the tool's arguments.
pub(crate) fn run(argv: Vec<std::ffi::OsString>) {
    let args: Vec<String> = argv
        .into_iter()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.first().map(String::as_str) != Some("admit") {
        eprintln!("usage: medulla-server bench-claim admit <measurement.json> --price-table <table.json> [--against <other.json>]");
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
