//! MEDULLA-UNIFIED-DATA-PLANE-R750.754: run history is the `RunTrace` receipt, so the crate
//! holds no second history store.

use std::path::Path;

// spec: MEDULLA-UNIFIED-DATA-PLANE-R750.754
#[test]
fn run_history_module_is_absent() {
    assert!(!Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/run_history.rs")).exists());
    assert!(!include_str!("../src/lib.rs").contains("run_history"));
}
