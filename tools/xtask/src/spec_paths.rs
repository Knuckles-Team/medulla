//! MEDULLA-CONTRACT-R056.10: every repository path a spec row names exists.
//!
//! Walks `specs/*/requirements.md` and `specs/*/test-spec.md` and reports one finding per
//! backticked repository path (a trailing `:line` or `::test` is stripped) that does not
//! exist. A path the row marks as new (`(new` directly after it) is planned and not checked, and
//! neither is a path in a row whose bullet or table row says it is planned (`(planned`, `planned,`,
//! `does not exist yet`, `owning change adds`): the owning change creates that file.
//!
//! `--raw` also reports planned rows. `--changed` checks only the rows a staged diff of `specs/`
//! adds or edits, so the pre-commit hook fails a touched row without failing on untouched debt.

use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;

pub const NAME: &str = "spec-paths";
pub const SUMMARY: &str = "every repository path a spec row names exists";

const SPEC_FILES: [&str; 2] = ["requirements.md", "test-spec.md"];

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let raw = args.iter().any(|a| a == "--raw");
    let only = if args.iter().any(|a| a == "--changed") {
        Some(staged_added_lines(root)?)
    } else {
        None
    };
    let findings = scan(root, only.as_ref(), !raw)?;
    if only.is_some() && findings.is_empty() {
        println!("spec paths: every changed row names existing files");
        return Ok(());
    }
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
    scan(root, None, true)
}

/// `(spec file, 1-based line)` pairs a staged diff of `specs/` adds or edits.
fn staged_added_lines(root: &Path) -> Result<HashSet<(String, usize)>, String> {
    let out = Command::new("git")
        .args(["diff", "--cached", "-U0", "--no-color", "--", "specs"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git diff: {e}"))?;
    if !out.status.success() {
        return Err("git diff --cached failed".into());
    }
    let mut added = HashSet::new();
    let mut file = String::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(name) = line.strip_prefix("+++ b/") {
            file = name.to_string();
        } else if line.starts_with("@@") {
            // `@@ -a,b +c,d @@`: lines c..c+d are the added side; d defaults to 1.
            let Some(plus) = line.split_whitespace().find(|t| t.starts_with('+')) else {
                continue;
            };
            let mut parts = plus.trim_start_matches('+').split(',');
            let start: usize = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let count: usize = parts.next().and_then(|v| v.parse().ok()).unwrap_or(1);
            for n in start..start + count {
                added.insert((file.clone(), n));
            }
        }
    }
    Ok(added)
}

/// For each line, whether its bullet or table row (the line plus its continuation lines) marks
/// the named files as planned.
fn planned_lines(text: &str, marker: &Regex) -> Vec<bool> {
    let lines: Vec<&str> = text.lines().collect();
    let starts = |l: &str| l.starts_with("- ") || l.starts_with("| ") || l.starts_with('#');
    let mut out = vec![false; lines.len()];
    let mut begin = 0;
    while begin < lines.len() {
        let mut end = begin + 1;
        while end < lines.len() && !lines[end].trim().is_empty() && !starts(lines[end]) {
            end += 1;
        }
        let planned = lines[begin..end].iter().any(|l| marker.is_match(l));
        out[begin..end].iter_mut().for_each(|v| *v = planned);
        begin = end;
    }
    out
}

fn scan(
    root: &Path,
    only: Option<&HashSet<(String, usize)>>,
    skip_planned: bool,
) -> Result<Vec<String>, String> {
    let planned_marker = Regex::new(
        r"(?i)\(planned|\(new\b|\bplanned\b|does not exist yet|not yet (?:exist|land|written)|to be created|owning change adds",
    )
    .map_err(|e| e.to_string())?;
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
            let planned = planned_lines(&text, &planned_marker);
            for (index, line) in text.lines().enumerate() {
                if let Some(only) = only {
                    if !only.contains(&(rel.clone(), index + 1)) {
                        continue;
                    }
                }
                if skip_planned && planned[index] {
                    continue;
                }
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

    #[test]
    fn a_planned_row_is_skipped_unless_raw() {
        let t = tree("- `R1`: `crates/x/later.rs::t` (planned, L1): the change adds it.\n  more\n- `R2` `crates/x/gone.rs`");
        let f = scan(t.path(), None, true).unwrap();
        assert_eq!(f.len(), 1);
        assert!(f[0].contains("gone.rs"));
        assert_eq!(scan(t.path(), None, false).unwrap().len(), 2);
    }
}
