//! No operator ships without an oracle case (MEDULLA-INCREMENTAL-VIEWS-R004).
//!
//! Every `Op` variant that `Circuit::compile` accepts must have one named case in
//! `ORACLE_CASES`, and each named case must be a test that exists in an oracle test file.
//! A construct with no case fails the test with its name.
#![cfg(feature = "query")]

use medulla_exec::incremental::Circuit;
use medulla_query_types::wire::{Op, Plan, Pred, TimeAxis};

/// `(construct, oracle case)`: the case is a test function in `ORACLE_FILES`.
const ORACLE_CASES: &[(&str, &str)] = &[
    ("AsOf", "membership_matches_execute"),
    ("Filter", "membership_matches_execute"),
    ("Limit", "window_matches_execute"),
    ("Scan", "scan_type_only_matches_execute"),
    ("WindowAgg", "window_matches_execute"),
];

/// The test files whose `fn` names the cases above refer to.
const ORACLE_FILES: &[&str] = &[
    "tests/incremental_execute_oracle.rs",
    "tests/incremental_oracle.rs",
];

/// The circuit builder, whose `match` arms are the ops it folds after the `Scan`.
const BUILDER: &str = "../medulla-exec/src/incremental/circuit/build.rs";

fn scan() -> Op {
    Op::Scan {
        label: "Doc".into(),
    }
}

/// One minimal plan per op kind the circuit may accept, keyed by the wire variant name.
fn probes() -> Vec<(&'static str, Plan)> {
    let after_scan = |op: Op| Plan::new(vec![scan(), op]);
    vec![
        ("Scan", Plan::new(vec![scan()])),
        (
            "Filter",
            after_scan(Op::Filter {
                preds: vec![Pred::GtNum {
                    prop: "year".into(),
                    n: 2000.0,
                }],
            }),
        ),
        (
            "AsOf",
            after_scan(Op::AsOf {
                ts: 1.0,
                axis: TimeAxis::Valid,
            }),
        ),
        (
            "WindowAgg",
            after_scan(Op::WindowAgg {
                secs: 10.0,
                agg: "count".into(),
            }),
        ),
        ("Limit", after_scan(Op::Limit { k: 1 })),
    ]
}

fn read(path: &str) -> String {
    let full = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {full}: {e}"))
}

/// The ops the builder folds, read from its `Op::<Variant>` match arms.
fn builder_ops() -> Vec<String> {
    let mut names: Vec<String> = read(BUILDER)
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("Op::"))
        .map(|rest| rest.chars().take_while(|c| c.is_alphanumeric()).collect())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn accepted() -> Vec<&'static str> {
    probes()
        .into_iter()
        .filter(|(_, plan)| Circuit::compile(plan).is_ok())
        .map(|(name, _)| name)
        .collect()
}

/// Constructs with no oracle case, plus cases that name no existing test.
fn gaps(accepted: &[&str], files: &[String]) -> Vec<String> {
    let mut gaps = Vec::new();
    for name in accepted {
        match ORACLE_CASES.iter().find(|(construct, _)| construct == name) {
            None => gaps.push(format!("{name}: no oracle case")),
            Some((_, case)) if !files.iter().any(|f| f.contains(&format!("fn {case}"))) => {
                gaps.push(format!(
                    "{name}: case {case} is not a test in the oracle files"
                ));
            }
            Some(_) => {}
        }
    }
    gaps
}

/// The nightly job of `.github/workflows/ci.yml` that runs the three budget benches.
const NIGHTLY_JOB: &str = "incremental-budget-benches";

/// The bench commands the release gate requires on the nightly schedule path (R001.10,
/// R009.8, R011.7).
const NIGHTLY_BENCHES: &[&str] = &[
    "cargo bench -p medulla-plan --features query --bench incremental_delta",
    "cargo bench -p medulla-sql-tests --features sql --bench matview_incremental",
    "cargo bench -p medulla-reasoner --features owl --bench owl_retract",
];

/// What is missing from the nightly job in the workflow text: the job itself, its
/// schedule-only condition, and each bench command inside the job's own block.
fn nightly_bench_gaps(workflow: &str) -> Vec<String> {
    let header = format!("\n  {NIGHTLY_JOB}:\n");
    let Some(start) = workflow.find(&header) else {
        return vec![format!("{NIGHTLY_JOB}: no such job")];
    };
    let rest = &workflow[start + header.len()..];
    // The job ends at the next top-level key (two-space indent, then a name and a colon).
    let end = rest
        .lines()
        .scan(0usize, |at, line| {
            let here = *at;
            *at += line.len() + 1;
            Some((here, line))
        })
        .find(|(_, line)| {
            line.starts_with("  ")
                && !line.starts_with("   ")
                && line.trim_end().ends_with(':')
                && !line.trim_start().starts_with('#')
        })
        .map_or(rest.len(), |(at, _)| at);
    let block = &rest[..end];
    let mut gaps = Vec::new();
    if !block.contains("if: github.event_name == 'schedule'") {
        gaps.push(format!(
            "{NIGHTLY_JOB}: not limited to the nightly schedule"
        ));
    }
    for command in NIGHTLY_BENCHES {
        if !block.contains(command) {
            gaps.push(format!("{NIGHTLY_JOB}: no `{command}`"));
        }
    }
    gaps
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R004
// spec: MEDULLA-INCREMENTAL-VIEWS-R015
#[test]
fn every_accepted_construct_has_an_oracle_case() {
    let files: Vec<String> = ORACLE_FILES.iter().map(|f| read(f)).collect();
    let accepted = accepted();
    let missing = gaps(&accepted, &files);
    assert!(missing.is_empty(), "oracle coverage gaps: {missing:?}");
    // The release gate also requires the three budget benches on the nightly path.
    let workflow = read("../../.github/workflows/ci.yml");
    let nightly = nightly_bench_gaps(&workflow);
    assert!(nightly.is_empty(), "nightly budget bench gaps: {nightly:?}");
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R015
#[test]
fn a_missing_nightly_bench_is_named() {
    let found = nightly_bench_gaps("jobs:\n  other:\n    steps: []\n");
    assert_eq!(
        found,
        vec![format!("{NIGHTLY_JOB}: no such job")],
        "a workflow without the job names it"
    );
    let partial = format!(
        "\n  {NIGHTLY_JOB}:\n    if: github.event_name == 'schedule'\n    steps:\n      - run: {}\n  next:\n    steps:\n      - run: {}\n",
        NIGHTLY_BENCHES[0], NIGHTLY_BENCHES[1]
    );
    let found = nightly_bench_gaps(&partial);
    assert_eq!(
        found,
        vec![
            format!("{NIGHTLY_JOB}: no `{}`", NIGHTLY_BENCHES[1]),
            format!("{NIGHTLY_JOB}: no `{}`", NIGHTLY_BENCHES[2]),
        ],
        "a command outside the job's own block does not count"
    );
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R004
#[test]
fn every_builder_op_has_a_probe() {
    let probed: Vec<&str> = probes().into_iter().map(|(name, _)| name).collect();
    let unprobed: Vec<String> = builder_ops()
        .into_iter()
        .filter(|op| !probed.contains(&op.as_str()))
        .collect();
    assert!(
        unprobed.is_empty(),
        "ops with no minimal plan: {unprobed:?}"
    );
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R004
#[test]
fn a_construct_without_a_case_is_named() {
    let found = gaps(&["Rolling"], &[]);
    assert_eq!(found, vec!["Rolling: no oracle case".to_string()]);
}
