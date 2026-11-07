//! MEDULLA-CONTRACT-R056.10: every repository path a spec row names exists.
//!
//! Walks `specs/*/requirements.md` and `specs/*/test-spec.md` and reports one finding per
//! backticked repository path (a trailing `:line` or `::test` is stripped) that does not
//! exist. A path the row marks as new (`(new` directly after it) is planned and not checked.

use regex::Regex;
use std::fs;
use std::path::Path;

pub const NAME: &str = "spec-paths";
pub const SUMMARY: &str = "every repository path a spec row names exists";

const SPEC_FILES: [&str; 2] = ["requirements.md", "test-spec.md"];

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    let findings = spec_rows_name_existing_files(root)?;
    if findings.is_empty() {
        println!("spec paths: every named file exists");
        return Ok(());
    }
    Err(format!(
        "{}\n{} spec path(s) name missing files",
        findings.join("\n"),
        findings.len()
    ))
}

/// One finding per missing path, as `specs/<dir>/<file>:<line>: names missing file <path>`.
pub fn spec_rows_name_existing_files(root: &Path) -> Result<Vec<String>, String> {
    let repo_path = Regex::new(
        r"`((?:crates|scripts|tests|src|benches|python|\.github|tools)/[A-Za-z0-9_./-]+?)(?::\d+)?(?:::[A-Za-z0-9_:]+)?`",
    )
    .map_err(|e| e.to_string())?;
    let mut dirs: Vec<_> = fs::read_dir(root.join("specs"))
        .map_err(|e| format!("specs/: {e}"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut findings = Vec::new();
    for dir in dirs {
        for name in SPEC_FILES {
            let file = dir.join(name);
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            let rel = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .display()
                .to_string();
            for (index, line) in text.lines().enumerate() {
                for hit in repo_path.captures_iter(line) {
                    let whole = hit.get(0).expect("group 0 always matches");
                    if line[whole.end()..].trim_start().starts_with("(new") {
                        continue;
                    }
                    let path = hit[1].trim_end_matches(['/', '.']);
                    if !root.join(path).exists() {
                        findings.push(format!("{rel}:{}: names missing file {path}", index + 1));
                    }
                }
            }
        }
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(spec_text: &str) -> crate::tempdir::Tree {
        crate::tempdir::Tree::new(&[
            ("specs/a/requirements.md", spec_text),
            ("crates/x/lib.rs", ""),
        ])
    }

    #[test]
    fn a_missing_path_is_a_finding() {
        let t = tree("| `R1` | uses `crates/x/gone.rs:12` |");
        let f = spec_rows_name_existing_files(t.path()).unwrap();
        assert_eq!(f.len(), 1);
        assert!(f[0].contains("crates/x/gone.rs"));
    }

    #[test]
    fn an_existing_path_and_a_new_marker_pass() {
        let t = tree("| `R1` | `crates/x/lib.rs::t` and `crates/x/planned.rs` (new) |");
        assert!(spec_rows_name_existing_files(t.path()).unwrap().is_empty());
    }
}
