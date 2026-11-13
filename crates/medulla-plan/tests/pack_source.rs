//! A bound pack source under the federation optimizer: only the key filter its registration
//! declares is pushed, and the exact local residual filter always runs
//! (MEDULLA-FEDERATED-QUERY-R722.8).
#![cfg(all(feature = "federation", feature = "query"))]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use medulla_exec::{algebra::Op, algebra::Plan, algebra::Pred, exec::execute, exec::PlanCtx};
use medulla_federation::federation::{ForeignSource, ForeignSourceRegistry};
use medulla_federation::federation_opt::FederatedExec;
use medulla_types::wire::{ForeignOperationCapabilities, ForeignSourceSpec};

/// A bound source that claims it filtered by key but always answers a superset: every
/// local document plus ids the plan never selected.
struct ClaimsToFilter {
    full_calls: Mutex<usize>,
    key_calls: Mutex<Vec<Vec<String>>>,
}

impl ForeignSource for ClaimsToFilter {
    fn fetch(&self) -> Result<medulla_exec::rowset::RowSet, String> {
        *self.full_calls.lock().unwrap() += 1;
        Ok(superset())
    }

    fn fetch_keys(&self, keys: &[String]) -> Result<medulla_exec::rowset::RowSet, String> {
        self.key_calls.lock().unwrap().push(keys.to_vec());
        Ok(superset())
    }
}

fn superset() -> medulla_exec::rowset::RowSet {
    medulla_exec::rowset::RowSet::from_ids(
        ["d1", "d2", "d3", "d4", "d5", "not-selected"].map(str::to_string),
    )
}

/// Run `Scan(Doc) -> Filter(year > 2023) -> ForeignScan(join)` against a bound source whose
/// registration declares `key_filter_pushdown`, and return what the source saw and what the
/// plan kept.
fn run(operation: &str, key_filter_pushdown: bool) -> (Arc<ClaimsToFilter>, BTreeSet<String>) {
    let source = Arc::new(ClaimsToFilter {
        full_calls: Mutex::new(0),
        key_calls: Mutex::new(Vec::new()),
    });
    let spec = ForeignSourceSpec::Api {
        operation: operation.into(),
        capabilities: ForeignOperationCapabilities {
            key_filter_pushdown,
            ..ForeignOperationCapabilities::default()
        },
    };
    let mut registry = ForeignSourceRegistry::new();
    registry.register_bound_trusted_host("pack", spec, source.clone());
    let fx = medulla_exec::fixture::build();
    let plan = Plan::new(vec![
        Op::Scan {
            label: "Doc".into(),
        },
        Op::Filter {
            preds: vec![Pred::GtNum {
                prop: "year".into(),
                n: 2023.0,
            }],
        },
        Op::ForeignScan {
            source: Box::new(ForeignSourceSpec::Named {
                name: "pack".into(),
            }),
            join: true,
        },
    ]);
    let federated = FederatedExec::with_registry(&registry);
    let ctx = PlanCtx::new(&fx.view, &fx.semantic).with_foreign_executor(&federated);
    let kept = execute(&plan, &ctx).unwrap().ids().into_iter().collect();
    (source, kept)
}

/// The fixture's local selection (documents newer than 2023) is the residual's answer.
fn residual() -> BTreeSet<String> {
    ["d1", "d2", "d4", "d5"].map(str::to_string).into()
}

// spec: MEDULLA-FEDERATED-QUERY-R722.8
#[test]
fn undeclared_filters_stay_local_and_residual_always_runs() {
    // Undeclared: no key filter reaches the source, which is read whole.
    let (undeclared, kept) = run("pack_source_undeclared", false);
    assert!(
        undeclared.key_calls.lock().unwrap().is_empty(),
        "a key filter the registration did not declare must stay local"
    );
    assert!(*undeclared.full_calls.lock().unwrap() >= 1);
    assert_eq!(
        kept,
        residual(),
        "the local residual drops ids the source returned beyond the selection"
    );

    // Declared: the keys are pushed, the source still answers a superset, and the exact
    // local residual keeps only the selected documents.
    let (declared, kept) = run("pack_source_declared", true);
    let pushed = declared.key_calls.lock().unwrap().clone();
    assert!(
        !pushed.is_empty(),
        "a declared key filter must reach the source"
    );
    let pushed_keys: BTreeSet<String> = pushed.into_iter().flatten().collect();
    assert_eq!(
        pushed_keys,
        residual(),
        "only the local selection is pushed"
    );
    assert_eq!(
        *declared.full_calls.lock().unwrap(),
        0,
        "a declared key lookup replaces the full read"
    );
    assert_eq!(
        kept,
        residual(),
        "a source that claims it filtered never skips the residual"
    );
}
