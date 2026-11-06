//! No operator ships without an oracle case (MEDULLA-INCREMENTAL-VIEWS-R004).
//!
//! Every `Op` variant that `Circuit::compile` accepts must have one named case in
//! `ORACLE_CASES`, and each named case must be a test that exists in an oracle test file.
//! A construct with no case fails the test with its name.
#![cfg(feature = "query")]

use medulla_exec::incremental::Circuit;
use medulla_types::wire::{Op, Plan, Pred, TimeAxis};

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

// spec: MEDULLA-INCREMENTAL-VIEWS-R004
#[test]
fn every_accepted_construct_has_an_oracle_case() {
    let files: Vec<String> = ORACLE_FILES.iter().map(|f| read(f)).collect();
    let accepted = accepted();
    let missing = gaps(&accepted, &files);
    assert!(missing.is_empty(), "oracle coverage gaps: {missing:?}");
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
