//! `medulla-server raft-flake-budget` — the retry-budget decision for
//! `scripts/validate-raft-cluster.sh` (MEDULLA-DURABLE-KERNEL-R014.3.2).
//!
//! The script calls this tool after a failed cluster-proof attempt. The tool replays the
//! failures through [`DiagnosticRetryLog::record_failure`] under the declared
//! [`FlakeBudget`] and prints the decision for the named attempt:
//!
//! * `pass attempt=<n>` (exit 0) when the attempt's exit code is 0;
//! * `retry attempt=<n> backoff_ms=<ms>` (exit 0) when another attempt is allowed;
//! * `refused RAFT_RETRY_BUDGET_EXHAUSTED attempt=<n>` (exit 3) otherwise.
//!
//! Usage:
//!   medulla-server raft-flake-budget --attempt <n> --exit-code <rc>
//!       [--max-retries 1] [--backoff-ms 1000] [--max-backoff-ms 30000]
//!
//! A malformed argument or budget exits 2.

use medulla_raft_xshard::flake_budget::{DiagnosticRetryDecision, DiagnosticRetryLog, FlakeBudget};

const REFUSED_EXIT: i32 = 3;
const USAGE_EXIT: i32 = 2;

struct Opts {
    attempt: usize,
    exit_code: i32,
    max_retries: u32,
    backoff_ms: u64,
    max_backoff_ms: u64,
}

fn parse(argv: Vec<std::ffi::OsString>) -> Result<Opts, String> {
    let mut attempt = None;
    let mut exit_code = None;
    let mut opts = Opts {
        attempt: 0,
        exit_code: 0,
        max_retries: 1,
        backoff_ms: 1_000,
        max_backoff_ms: 30_000,
    };
    // argv[0] is the subcommand name.
    let mut args = argv
        .into_iter()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned());
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let bad = |error: std::num::ParseIntError| format!("{flag} {value}: {error}");
        match flag.as_str() {
            "--attempt" => attempt = Some(value.parse::<usize>().map_err(bad)?),
            "--exit-code" => exit_code = Some(value.parse::<i32>().map_err(bad)?),
            "--max-retries" => opts.max_retries = value.parse().map_err(bad)?,
            "--backoff-ms" => opts.backoff_ms = value.parse().map_err(bad)?,
            "--max-backoff-ms" => opts.max_backoff_ms = value.parse().map_err(bad)?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    opts.attempt = attempt.ok_or("--attempt is required")?;
    opts.exit_code = exit_code.ok_or("--exit-code is required")?;
    if opts.attempt == 0 {
        return Err("--attempt is one-based".to_string());
    }
    Ok(opts)
}

fn decide(opts: &Opts) -> Result<(String, i32), String> {
    if opts.exit_code == 0 {
        return Ok((format!("pass attempt={}", opts.attempt), 0));
    }
    let budget = FlakeBudget::new(opts.max_retries, opts.backoff_ms, opts.max_backoff_ms)?;
    let mut log = DiagnosticRetryLog::new();
    let mut decision = None;
    for attempt in 1..=opts.attempt {
        decision = Some(log.record_failure(&budget, format!("attempt {attempt} failed"))?);
    }
    Ok(match decision.ok_or("no attempt recorded")? {
        DiagnosticRetryDecision::Retry {
            attempt,
            backoff_ms,
        } => (
            format!("retry attempt={attempt} backoff_ms={backoff_ms}"),
            0,
        ),
        DiagnosticRetryDecision::Exhausted { attempt }
        | DiagnosticRetryDecision::Refused { attempt, .. } => (
            format!(
                "refused {} attempt={attempt}",
                medulla_raft_xshard::flake_budget::RAFT_RETRY_BUDGET_EXHAUSTED
            ),
            REFUSED_EXIT,
        ),
    })
}

pub(crate) fn run(argv: Vec<std::ffi::OsString>) {
    match parse(argv).and_then(|opts| decide(&opts)) {
        Ok((line, code)) => {
            println!("{line}");
            std::process::exit(code);
        }
        Err(error) => {
            eprintln!("medulla-server raft-flake-budget: {error}");
            std::process::exit(USAGE_EXIT);
        }
    }
}
