//! MEDULLA-DECISION-ENGINE-R178.2: compare a decision-budget run to its stored baseline.
//!
//! `cargo xtask decide-budgets <run-report.json> [--baseline PATH]`. A run report and the
//! baseline (`benches/baselines/decide.json` by default) are keyed by Criterion group, each with
//! `p50_ms`, `p99_ms` and `peak_mib`. The check fails when a measured value exceeds the absolute
//! budget of the baseline or regresses more than its tolerance above its stored value. A group
//! with no stored baseline is checked against the absolute budgets only.

use serde_json::Value;
use std::fs;
use std::path::Path;

pub const NAME: &str = "decide-budgets";
pub const SUMMARY: &str = "a decision-budget run stays within budget and its stored baseline";

const METRICS: [&str; 3] = ["p50_ms", "p99_ms", "peak_mib"];
const DEFAULT_BASELINE: &str = "benches/baselines/decide.json";
/// The capability-designation groups carry the extra metrics of
/// MEDULLA-DECISION-ENGINE-R172.10 beside the three generic ones.
const DESIGNATION_PREFIX: &str = "decide/capability_designation";
const DESIGNATION_CEILINGS: [&str; 3] = ["filtered_p50_ms", "filtered_p99_ms", "upsert_p99_ms"];

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut report = None;
    let mut baseline = root.join(DEFAULT_BASELINE);
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--baseline" {
            baseline = iter.next().ok_or("--baseline needs a path")?.into();
        } else {
            report = Some(arg);
        }
    }
    let report =
        report.ok_or("usage: cargo xtask decide-budgets <run-report.json> [--baseline PATH]")?;
    let findings = check(&load(Path::new(report))?, &load(&baseline)?)?;
    if findings.is_empty() {
        println!("decide budgets: every group within budget");
        return Ok(());
    }
    Err(findings
        .iter()
        .map(|f| format!("FAIL {f}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn load(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Formats like Python's `:g` for the values a report holds (integers lose the `.0`).
fn g(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

fn number(value: &Value, what: &str) -> Result<f64, String> {
    value
        .as_f64()
        .ok_or_else(|| format!("{what} is not a number"))
}

/// One finding per budget breach or regression; empty means pass.
pub fn check(run: &Value, baseline: &Value) -> Result<Vec<String>, String> {
    let budgets = &baseline["budgets"];
    let tolerance = number(&baseline["regression_tolerance"], "regression_tolerance")?;
    let mut findings = Vec::new();
    let groups = run["groups"].as_object().filter(|g| !g.is_empty());
    let Some(groups) = groups else {
        return Ok(vec!["run report has no groups".into()]);
    };
    let mut names: Vec<&String> = groups.keys().collect();
    names.sort();
    for group in names {
        let measured = &groups[group];
        for metric in METRICS {
            let Some(raw) = measured.get(metric) else {
                findings.push(format!("{group}: missing {metric}"));
                continue;
            };
            let value = number(raw, metric)?;
            // A group may carry its own tighter budget (`group_budgets`).
            let budget = match baseline["group_budgets"][group.as_str()].get(metric) {
                Some(own) => number(own, &format!("group budget {metric}"))?,
                None => number(&budgets[metric], &format!("budget {metric}"))?,
            };
            if value > budget {
                findings.push(format!(
                    "{group}: {metric} {} exceeds budget {}",
                    g(value),
                    g(budget)
                ));
            }
            if let Some(base) = baseline["groups"][group.as_str()]
                .get(metric)
                .and_then(Value::as_f64)
            {
                if value > base * (1.0 + tolerance) {
                    let pct = (value / base - 1.0) * 100.0;
                    findings.push(format!(
                        "{group}: {metric} {} regressed {pct:.1}% over baseline {}",
                        g(value),
                        g(base)
                    ));
                }
            }
        }
    }
    for group in groups.keys().filter(|n| n.starts_with(DESIGNATION_PREFIX)) {
        let budget = &baseline["group_budgets"][group.as_str()];
        designation_findings(group, &groups[group], budget, &mut findings)?;
    }
    Ok(findings)
}

/// The recall floor, the filtered and upsert latencies and the resident-byte ceiling of one
/// capability-designation group (MEDULLA-DECISION-ENGINE-R172.10).
fn designation_findings(
    group: &str,
    measured: &Value,
    budget: &Value,
    findings: &mut Vec<String>,
) -> Result<(), String> {
    if budget.is_null() {
        findings.push(format!("{group}: no group budget in the baseline"));
        return Ok(());
    }
    for metric in DESIGNATION_CEILINGS {
        match measured.get(metric) {
            None => findings.push(format!("{group}: missing {metric}")),
            Some(raw) => {
                let value = number(raw, metric)?;
                let ceiling = number(&budget[metric], &format!("group budget {metric}"))?;
                if value > ceiling {
                    findings.push(format!(
                        "{group}: {metric} {} exceeds budget {}",
                        g(value),
                        g(ceiling)
                    ));
                }
            }
        }
    }
    match measured.get("recall_at_5") {
        None => findings.push(format!("{group}: missing recall_at_5")),
        Some(raw) => {
            let value = number(raw, "recall_at_5")?;
            let floor = number(&budget["recall_at_5_min"], "group budget recall_at_5_min")?;
            if value < floor {
                findings.push(format!(
                    "{group}: recall_at_5 {} is below {}",
                    g(value),
                    g(floor)
                ));
            }
        }
    }
    let (Some(resident), Some(records), Some(dim)) = (
        measured.get("resident_bytes"),
        measured.get("records"),
        measured.get("dim"),
    ) else {
        findings.push(format!("{group}: missing resident_bytes, records or dim"));
        return Ok(());
    };
    let (resident, records, dim) = (
        number(resident, "resident_bytes")?,
        number(records, "records")?,
        number(dim, "dim")?,
    );
    let allowed = number(
        &budget["resident_bytes_vector_factor"],
        "group budget resident_bytes_vector_factor",
    )? * records
        * dim
        + number(
            &budget["resident_bytes_per_record_overhead"],
            "group budget resident_bytes_per_record_overhead",
        )? * records;
    if resident > allowed {
        findings.push(format!(
            "{group}: resident_bytes {} exceeds budget {}",
            g(resident),
            g(allowed)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;
    use serde_json::json;

    fn baseline() -> Value {
        let mut stored = load(&crate::workspace_root().join(DEFAULT_BASELINE)).unwrap();
        stored["groups"] =
            json!({"decide/route": {"p50_ms": 5.0, "p99_ms": 40.0, "peak_mib": 8.0}});
        stored
    }

    fn report(p50: f64, p99: f64, group: &str, peak: f64) -> Value {
        json!({"groups": {group: {"p50_ms": p50, "p99_ms": p99, "peak_mib": peak}}})
    }

    // spec: MEDULLA-DECISION-ENGINE-R178.2
    #[test]
    fn a_planted_regression_fails() {
        let baseline = baseline();
        assert!(check(&report(5.2, 41.0, "decide/route", 8.0), &baseline)
            .unwrap()
            .is_empty());
        let planted = check(&report(6.0, 48.0, "decide/route", 8.0), &baseline).unwrap();
        assert!(planted
            .iter()
            .any(|f| f.contains("p50_ms") && f.contains("regressed 20.0%")));
        assert!(planted
            .iter()
            .any(|f| f.contains("p99_ms") && f.contains("regressed 20.0%")));
        let breach = check(&report(21.0, 151.0, "decide/new", 33.0), &baseline).unwrap();
        assert!(
            breach.len() == 3 && breach.iter().all(|f| f.contains("exceeds budget")),
            "{breach:?}"
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R172.10
    #[test]
    fn a_designation_breach_fails_each_metric() {
        let baseline = baseline();
        let group = "decide/capability_designation";
        let steady = json!({"groups": {group: {
            "p50_ms": 2.5, "p99_ms": 12.0, "peak_mib": 100.0,
            "filtered_p50_ms": 1.5, "filtered_p99_ms": 8.0, "recall_at_5": 0.97,
            "upsert_p99_ms": 0.5, "records": 100000, "dim": 1024,
            "resident_bytes": 100000.0 * 1024.0 * 1.25
        }}});
        assert!(check(&steady, &baseline).unwrap().is_empty());
        let mut planted = steady.clone();
        planted["groups"][group]["recall_at_5"] = json!(0.9);
        planted["groups"][group]["upsert_p99_ms"] = json!(2.0);
        planted["groups"][group]["filtered_p99_ms"] = json!(11.0);
        planted["groups"][group]["resident_bytes"] = json!(200_000_000.0);
        let found = check(&planted, &baseline).unwrap();
        for metric in [
            "recall_at_5",
            "upsert_p99_ms",
            "filtered_p99_ms",
            "resident_bytes",
        ] {
            assert!(
                found.iter().any(|f| f.contains(metric)),
                "{metric}: {found:?}"
            );
        }
    }

    #[test]
    fn a_group_budget_overrides_the_default() {
        let mut baseline = baseline();
        baseline["group_budgets"] = json!({"decide/tight": {"p99_ms": 5.0}});
        let found = check(&report(1.0, 6.0, "decide/tight", 1.0), &baseline).unwrap();
        assert_eq!(found, vec!["decide/tight: p99_ms 6 exceeds budget 5"]);
        assert!(check(&report(1.0, 6.0, "decide/loose", 1.0), &baseline)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_empty_report_and_a_missing_metric_are_findings() {
        let baseline = baseline();
        assert_eq!(
            check(&json!({}), &baseline).unwrap(),
            vec!["run report has no groups"]
        );
        let partial = json!({"groups": {"decide/x": {"p50_ms": 1.0}}});
        let found = check(&partial, &baseline).unwrap();
        assert_eq!(
            found,
            vec!["decide/x: missing p99_ms", "decide/x: missing peak_mib"]
        );
    }

    #[test]
    fn the_command_reads_report_and_baseline_files() {
        let t = Tree::new(&[
            ("baseline.json", &baseline().to_string()),
            (
                "planted.json",
                &report(6.0, 48.0, "decide/route", 8.0).to_string(),
            ),
            (
                "steady.json",
                &report(5.2, 41.0, "decide/route", 8.0).to_string(),
            ),
        ]);
        let args = |file: &str| {
            vec![
                t.path().join(file).to_string_lossy().into_owned(),
                "--baseline".into(),
                t.path()
                    .join("baseline.json")
                    .to_string_lossy()
                    .into_owned(),
            ]
        };
        assert!(run(&crate::workspace_root(), &args("planted.json")).is_err());
        run(&crate::workspace_root(), &args("steady.json")).unwrap();
    }
}
