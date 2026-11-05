//! Prints the bounded diagnostic-retry decision for one failed attempt.
//!
//! MEDULLA-DURABLE-KERNEL-R014.3.2: `scripts/validate-raft-cluster.sh` reads its
//! retry budget through this bin instead of looping on its own.
//!
//! Usage: `raft_flake_budget <max_retries> <backoff_ms> <max_backoff_ms> <attempt> <exit_code>`
//! where `attempt` is the one-based number of the execution that just finished.
//!
//! Output (one line) and exit code:
//! - `pass`                                  exit 0 (the attempt exited 0)
//! - `retry <backoff_ms>`                    exit 0
//! - `exhausted`                             exit 1 (terminal failure)
//! - `refused RAFT_RETRY_BUDGET_EXHAUSTED`   exit 3
//! - a usage or budget error on stderr       exit 2

use medulla_raft_xshard::flake_budget::{DiagnosticRetryDecision, DiagnosticRetryLog, FlakeBudget};

fn parse<T: std::str::FromStr>(args: &[String], index: usize, name: &str) -> Result<T, String> {
    args.get(index)
        .ok_or_else(|| format!("missing argument {name}"))?
        .parse()
        .map_err(|_| format!("{name} must be a non-negative integer"))
}

fn decide(args: &[String]) -> Result<(String, i32), String> {
    let budget = FlakeBudget::new(
        parse(args, 0, "max_retries")?,
        parse(args, 1, "backoff_ms")?,
        parse(args, 2, "max_backoff_ms")?,
    )?;
    let attempt: usize = parse(args, 3, "attempt")?;
    let exit_code: i32 = parse(args, 4, "exit_code")?;
    if attempt == 0 {
        return Err("attempt is one-based".into());
    }
    if exit_code == 0 {
        return Ok(("pass".into(), 0));
    }
    let mut log = DiagnosticRetryLog::new();
    let mut decision = None;
    for n in 1..=attempt {
        decision = Some(log.record_failure(&budget, format!("attempt {n} exit {exit_code}"))?);
    }
    Ok(match decision {
        Some(DiagnosticRetryDecision::Retry { backoff_ms, .. }) => {
            (format!("retry {backoff_ms}"), 0)
        }
        Some(DiagnosticRetryDecision::Exhausted { .. }) => ("exhausted".into(), 1),
        Some(DiagnosticRetryDecision::Refused { code, .. }) => (format!("refused {code}"), 3),
        None => return Err("attempt is one-based".into()),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match decide(&args) {
        Ok((line, code)) => {
            println!("{line}");
            std::process::exit(code);
        }
        Err(message) => {
            eprintln!("raft_flake_budget: {message}");
            std::process::exit(2);
        }
    }
}
