//! Source analysis shared by the repository checks: lexical masks, the compiler-declared module
//! tree (`mod` declarations to files, with `cfg` evaluation), and call-graph slicing.
//!
//! The module tree is read from `mod`/`include!` declarations exactly as rustc would resolve them;
//! `syn` parses every `cfg` predicate and every item the call graph walks. Nothing here guesses a
//! filename list: a declared file that is missing, ambiguous, cyclic or unsupported is an error.

pub mod callgraph;
pub mod cfg;
pub mod mask;
pub mod module_tree;

use std::path::Path;

/// Read a UTF-8 file under `root` as a check input.
pub fn read(root: &Path, relative: &str) -> Result<String, String> {
    std::fs::read_to_string(root.join(relative)).map_err(|e| format!("cannot read {relative}: {e}"))
}
