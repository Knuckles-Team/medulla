//! MEDULLA-CONTRACT-R053: run the integration suite and gate on regressions, not on absolute green.
//!
//! `cargo xtask integration-baseline [--today YYYY-MM-DD] [-- pytest args]`. The suite is gated
//! differentially against `tests/integration_failure_baseline.txt`: a committed list of known
//! failing test ids, each carrying an owner and a review date. The gate fails when a test outside
//! the list fails (regression), when a listed test passes (repaired: remove its line), or when an
//! entry is past its review-by date. Only pytest's `short test summary info` block is read, and
//! the failure reason is dropped: the gate tracks which tests fail, never why.

use regex::Regex;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub const NAME: &str = "integration-baseline";
pub const SUMMARY: &str = "the integration suite has no regression against its failure baseline";

const BASELINE: &str = "tests/integration_failure_baseline.txt";

/// A review-by date as (year, month, day); tuples order chronologically.
type Date = (u32, u32, u32);

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut today = None;
    let mut pytest_args = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--today" => today = Some(iter.next().ok_or("--today needs a date")?.clone()),
            "--" => {
                pytest_args.extend(iter.by_ref().cloned());
            }
            other => pytest_args.push(other.to_string()),
        }
    }
    let runner = |pytest_args: &[String]| -> (i32, String) {
        let out = Command::new("python3")
            .args(["-m", "pytest"])
            .args(pytest_args)
            .current_dir(root)
            .stdin(Stdio::null())
            .output();
        match out {
            Ok(o) => (
                o.status.code().unwrap_or(-1),
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
            ),
            Err(e) => (-1, format!("cannot start pytest: {e}")),
        }
    };
    let verdict = evaluate(
        &root.join(BASELINE),
        today.as_deref(),
        &pytest_args,
        &runner,
    );
    print!("{}", verdict.stdout);
    if verdict.code == 0 {
        return Ok(());
    }
    Err(verdict.stderr)
}

pub struct Verdict {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Parses one `YYYY-MM-DD`; rejects impossible calendar dates.
fn parse_date(text: &str) -> Result<Date, String> {
    let bad = || format!("Invalid isoformat string: '{text}'");
    let parts: Vec<&str> = text.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return Err(bad());
    }
    let n = |s: &str| s.parse::<u32>().map_err(|_| bad());
    let (y, m, d) = (n(parts[0])?, n(parts[1])?, n(parts[2])?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28 + u32::from(leap),
        _ => return Err(bad()),
    };
    if y == 0 || d == 0 || d > days {
        return Err(bad());
    }
    Ok((y, m, d))
}

fn today(override_: Option<&str>) -> Result<Date, String> {
    if let Some(text) = override_ {
        return parse_date(text);
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    // Civil-from-days (proleptic Gregorian), days since 1970-01-01.
    let z = i64::try_from(secs / 86_400).map_err(|e| e.to_string())? + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Ok((y as u32, m as u32, d as u32))
}

fn base(node: &str) -> &str {
    node.split('[').next().unwrap_or(node)
}

type Baseline = (BTreeSet<String>, Vec<(String, Date)>);

/// Node ids and review-by dates; refuses a malformed line.
fn parse_baseline(text: &str, name: &str) -> Result<Baseline, String> {
    let entry = Regex::new(
        r"^(?P<node>.+?)\s\s+#\s*owner=(?P<owner>\S+)\s+review-by=(?P<date>[0-9]{4}-[0-9]{2}-[0-9]{2})\s*(?:note=(?P<note>\S+))?\s*$",
    )
    .expect("regex");
    let mut nodes = BTreeSet::new();
    let mut dated = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(m) = entry.captures(line) else {
            return Err(format!(
                "{name}:{}: malformed entry. Every line must be\n  <test-node-id>  # owner=@handle review-by=YYYY-MM-DD [note=tag]\ngot: {line:?}",
                index + 1
            ));
        };
        nodes.insert(m["node"].to_string());
        dated.push((m["node"].to_string(), parse_date(&m["date"])?));
    }
    Ok((nodes, dated))
}

fn load_baseline(path: &Path) -> Result<Baseline, String> {
    let text = fs::read_to_string(path).map_err(|e| {
        format!(
            "cannot read required integration baseline {}: {e}",
            path.display()
        )
    })?;
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    parse_baseline(&text, &name).map_err(|detail| {
        format!(
            "malformed required integration baseline {}: {detail}",
            path.display()
        )
    })
}

/// Strips pytest's ` - <reason>` suffix without truncating a parametrised id.
fn node_id(captured: &str) -> String {
    if captured.contains('[') {
        if let Some(close) = captured.rfind(']') {
            return captured[..=close].trim().to_string();
        }
    }
    captured
        .split_once(" - ")
        .map_or(captured, |(head, _)| head)
        .trim()
        .to_string()
}

/// Failing or erroring node ids, read only from pytest's summary block.
fn parse_outcomes(output: &str) -> BTreeSet<String> {
    let start = Regex::new(r"^=+\s*short test summary info\s*=+$").expect("regex");
    let end = Regex::new(r"^=+.*(?:passed|failed|error|no tests ran).*=+$").expect("regex");
    let outcome = Regex::new(r"^(?:FAILED|ERROR)\s+(.+)$").expect("regex");
    let mut found = BTreeSet::new();
    let mut inside = false;
    for raw in output.lines() {
        let line = raw.trim();
        if !inside {
            inside = start.is_match(line);
            continue;
        }
        if end.is_match(line) {
            break;
        }
        if let Some(m) = outcome.captures(line) {
            found.insert(node_id(&m[1]));
        }
    }
    found
}

/// Runs the whole gate against an injected pytest runner.
pub fn evaluate(
    baseline_path: &Path,
    today_override: Option<&str>,
    pytest_args: &[String],
    runner: &dyn Fn(&[String]) -> (i32, String),
) -> Verdict {
    let refuse = |message: String| Verdict {
        code: 1,
        stdout: String::new(),
        stderr: format!("REFUSED: {message}"),
    };
    let (baseline, dated) = match load_baseline(baseline_path) {
        Ok(loaded) => loaded,
        Err(message) => return refuse(message),
    };
    let (code, output) = runner(pytest_args);
    let mut stdout = format!("{output}\n");
    let failing = parse_outcomes(&output);
    if code != 0 && code != 1 {
        return Verdict {
            code: 1,
            stdout,
            stderr: format!(
                "REFUSED: pytest exited {code}, which is not a test verdict (0=all passed, 1=tests failed). Nothing was compared to the baseline."
            ),
        };
    }
    let covered = |node: &String| baseline.contains(node) || baseline.contains(base(node));
    let regressions: Vec<&String> = failing.iter().filter(|n| !covered(n)).collect();
    let repaired: Vec<&String> = baseline
        .iter()
        .filter(|entry| {
            !failing
                .iter()
                .any(|n| n == *entry || base(n) == entry.as_str())
        })
        .collect();
    let mut stale = Vec::new();
    for (node, review) in &dated {
        match today(today_override) {
            Ok(now) if *review < now => stale.push(node),
            Ok(_) => {}
            Err(e) => return refuse(e),
        }
    }
    stale.sort();
    let mut stderr = String::new();
    if !regressions.is_empty() {
        stderr
            .push_str("\nREGRESSION — these tests are NOT in the baseline and are failing now:\n");
        for node in &regressions {
            stderr.push_str(&format!("  {node}\n"));
        }
        stderr.push_str("Fix them. Adding a line to the baseline is for debt this change did not introduce, never for a failure it did.\n");
    }
    if !repaired.is_empty() {
        stderr.push_str("\nFIXED — these are baselined but now PASS. Remove their lines from integration_failure_baseline.txt so the ratchet keeps tightening:\n");
        for node in &repaired {
            stderr.push_str(&format!("  {node}\n"));
        }
    }
    if !stale.is_empty() {
        stderr.push_str("\nOVERDUE — these baseline entries are past their review-by date. Fix the test or move the date deliberately (a real decision):\n");
        for node in &stale {
            stderr.push_str(&format!("  {node}\n"));
        }
    }
    if !stderr.is_empty() {
        return Verdict {
            code: 1,
            stdout,
            stderr,
        };
    }
    if failing.is_empty() {
        stdout.push_str("\nintegration baseline: OK — nothing failing at all.\n");
    } else {
        stdout.push_str(&format!(
            "\nintegration baseline: OK — {} known failure(s), no regressions. See integration_failure_baseline.txt.\n",
            failing.len()
        ));
    }
    Verdict {
        code: 0,
        stdout,
        stderr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    const KNOWN: &str = "tests/test_example.py::test_known_failure";
    const SUMMARY: &str = "================ short test summary info ================\n";

    fn entry(review_by: &str) -> String {
        format!("{KNOWN}  # owner=@proof review-by={review_by}\n")
    }
    fn known_failed() -> String {
        format!("FAILED {KNOWN} - expected debt\n")
    }
    fn one_known_failure() -> String {
        format!(
            "{SUMMARY}{}=================== 1 failed ===================\n",
            known_failed()
        )
    }

    /// Runs the gate with a canned pytest verdict; returns the verdict.
    fn gate(baseline: &str, today: Option<&str>, code: i32, output: &str) -> Verdict {
        let tree = Tree::new(&[("baseline.txt", baseline)]);
        let output = output.to_string();
        let runner = move |_: &[String]| (code, output.clone());
        evaluate(&tree.path().join("baseline.txt"), today, &[], &runner)
    }

    #[test]
    fn untrustworthy_baseline_refuses_to_run_pytest() {
        let panics = |_: &[String]| -> (i32, String) { panic!("pytest must not run") };
        let tree = Tree::new(&[
            ("malformed.txt", "not a baseline entry\n"),
            ("date.txt", &entry("2026-02-30")),
        ]);
        std::fs::create_dir_all(tree.path().join("dir")).expect("mkdir");
        for (file, refusal) in [
            (
                "missing.txt",
                "REFUSED: cannot read required integration baseline",
            ),
            ("dir", "REFUSED: cannot read required integration baseline"),
            (
                "malformed.txt",
                "REFUSED: malformed required integration baseline",
            ),
            (
                "date.txt",
                "REFUSED: malformed required integration baseline",
            ),
        ] {
            let v = evaluate(&tree.path().join(file), None, &[], &panics);
            assert_eq!(v.code, 1, "{file}");
            assert!(v.stderr.contains(refusal), "{file}: {}", v.stderr);
        }
    }

    #[test]
    fn baselined_failure_passes() {
        let v = gate(&entry("2099-01-01"), None, 1, &one_known_failure());
        assert_eq!(v.code, 0);
        assert!(v.stdout.contains("1 known failure(s), no regressions"));
    }

    #[test]
    fn new_unbaselined_failure_is_a_regression() {
        let output = format!(
            "{SUMMARY}{}FAILED tests/test_other.py::test_new_break - surprise\n=================== 2 failed ===================\n",
            known_failed()
        );
        let v = gate(&entry("2099-01-01"), None, 1, &output);
        assert_eq!(v.code, 1);
        assert!(v.stderr.contains("REGRESSION"));
        assert!(v.stderr.contains("tests/test_other.py::test_new_break"));
        assert!(!v.stderr.contains(KNOWN));
    }

    #[test]
    fn baselined_test_now_passing_is_repaired() {
        let v = gate(
            &entry("2099-01-01"),
            None,
            0,
            "================ 1 passed ================\n",
        );
        assert_eq!(v.code, 1);
        assert!(v.stderr.contains("FIXED") && v.stderr.contains(KNOWN));
    }

    #[test]
    fn entry_past_review_date_is_overdue_but_due_today_is_not() {
        let v = gate(
            &entry("2020-01-01"),
            Some("2026-01-01"),
            1,
            &one_known_failure(),
        );
        assert_eq!(v.code, 1);
        assert!(v.stderr.contains("OVERDUE") && v.stderr.contains(KNOWN));
        let v = gate(
            &entry("2026-01-01"),
            Some("2026-01-01"),
            1,
            &one_known_failure(),
        );
        assert_eq!(v.code, 0);
        assert!(!v.stderr.contains("OVERDUE"));
    }

    #[test]
    fn unbracketed_entry_covers_every_parametrisation() {
        let output = format!(
            "{SUMMARY}FAILED {KNOWN}[case-a] - expected debt\n=================== 1 failed ===================\n"
        );
        let v = gate(&entry("2099-01-01"), None, 1, &output);
        assert_eq!(v.code, 0);
        assert!(v.stdout.contains("1 known failure(s), no regressions"));
        assert!(!v.stderr.contains("REGRESSION"));
    }

    #[test]
    fn untrustworthy_pytest_exit_code_refuses_to_compare() {
        let v = gate(&entry("2099-01-01"), None, 2, "internal error\n");
        assert_eq!(v.code, 1);
        assert!(v.stderr.contains("REFUSED: pytest exited 2"));
        assert!(v.stderr.contains("Nothing was compared to the baseline"));
    }

    #[test]
    fn nothing_failing_at_all_and_an_unparsed_today_without_dated_entries() {
        for today in [None, Some("not-a-date")] {
            let v = gate("", today, 0, "================ 3 passed ================\n");
            assert_eq!(v.code, 0);
            assert!(v.stdout.contains("nothing failing at all"));
        }
        let v = gate("", Some("not-a-date"), 1, &one_known_failure());
        assert_eq!(v.code, 1);
        assert!(v.stderr.contains("REGRESSION") && v.stderr.contains(KNOWN));
    }

    #[test]
    fn only_the_summary_block_is_read_and_parameter_ids_keep_their_dashes() {
        let output = "ERROR    asyncio:base_events.py:1785 Task was destroyed but it is pending!\n\
            ================ short test summary info ================\n\
            FAILED t.py::drift[does not exactly cover - x] - AssertionError\n\
            ERROR t.py::other - boom\n\
            =================== 2 failed ===================\n\
            ERROR after the block\n";
        let found = parse_outcomes(output);
        let expected: BTreeSet<String> = ["t.py::drift[does not exactly cover - x]", "t.py::other"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn dates_are_calendar_checked() {
        assert!(parse_date("2026-02-30").is_err());
        assert!(parse_date("2024-02-29").is_ok());
        assert!(parse_date("2026-13-01").is_err());
        assert!(today(None).expect("today").0 >= 2026);
    }
}
