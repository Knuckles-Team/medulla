//! `bench-claim admit` (MEDULLA-DURABLE-KERNEL-R042.100): admit a benchmark
//! measurement or print the refusal code and exit non-zero.
//!
//! Usage: `bench-claim admit <measurement.json> --price-table <table.json>
//! [--against <other-measurement.json>]`.

use medulla_types::benchmark_claim::{BenchmarkMeasurement, MeasurementRefusal, PriceTable};
use std::process::ExitCode;

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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("admit") {
        eprintln!("usage: bench-claim admit <measurement.json> --price-table <table.json> [--against <other.json>]");
        return ExitCode::from(2);
    }
    match admit(&args[1..]) {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(refusal) => {
            println!("{}", refusal.code());
            ExitCode::FAILURE
        }
    }
}
