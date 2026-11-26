//! Print the required pre-merge test-module manifest as JSON
//! (MEDULLA-DURABLE-KERNEL-R011.3).
//!
//! The pull-request job `required-runtime` passes this document to the shared gate, which
//! runs each module and fails on a failed test or on `ZERO_SELECTED_TESTS`. The document is
//! `required_test_modules()` and nothing else: one object per module with its category, its
//! package, its libtest filter, its features and the exact `cargo` arguments that select it.

use medulla_source_types::pre_merge_test_module::required_test_modules;

fn main() {
    let modules: Vec<serde_json::Value> = required_test_modules()
        .iter()
        .map(|module| {
            let cargo_args = module
                .cargo_test_args()
                .unwrap_or_else(|error| panic!("{}: {error}", module.crate_name));
            serde_json::json!({
                "category": module.category.name(),
                "crate": module.crate_name,
                "filter": module.cargo_test_filter,
                "features": module.cargo_features,
                "cargo_args": cargo_args,
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string(&modules).expect("the manifest serializes")
    );
}
