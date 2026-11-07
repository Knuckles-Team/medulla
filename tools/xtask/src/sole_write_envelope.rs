//! MEDULLA-DURABLE-KERNEL-R043.102: no redb table is written outside the transaction crates.
//!
//! `cargo xtask sole-write-envelope`. Every durable write goes through the mutation envelope in
//! `crates/medulla-transaction` and `crates/medulla-storage`. Anywhere else under `crates/` or
//! `src/` an `open_table` call on a write transaction is a direct table write and is reported as
//! `SOLE_ENVELOPE_VIOLATION <file>:<line>`. A write transaction is recognised syntactically: a
//! binding taken from `begin_write()`, a chained `begin_write()?.open_table(`, or a parameter typed
//! `WriteTransaction`. Known exceptions live in `tools/xtask/sole_write_envelope.allow` as
//! `<path> <reason>` lines; a line without a reason, or one naming a path that no longer writes
//! directly, fails the check so the allow file cannot rot.

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const NAME: &str = "sole-write-envelope";
pub const SUMMARY: &str = "no redb table is written outside the transaction crates";

const ALLOW: &str = "tools/xtask/sole_write_envelope.allow";
const SCAN_ROOTS: [&str; 2] = ["crates", "src"];
const EXEMPT: [&str; 2] = ["crates/medulla-transaction/", "crates/medulla-storage/"];
const CODE: &str = "SOLE_ENVELOPE_VIOLATION";

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    let allow = fs::read_to_string(root.join(ALLOW)).unwrap_or_default();
    let errors = scan(root, &allow)?;
    if errors.is_empty() {
        println!("sole-write-envelope: no direct table writes outside the envelope");
        return Ok(());
    }
    Err(errors.join("\n"))
}

/// The 1-based lines of direct table writes in one Rust file.
pub fn scan_source(text: &str) -> Vec<usize> {
    let line_comment = Regex::new(r"//.*$").expect("regex");
    let binding =
        Regex::new(r"\blet\s+(?:mut\s+)?([a-z_][a-z0-9_]*)\s*(?::[^=]*)?=[^;]*\bbegin_write\s*\(")
            .expect("regex");
    let param =
        Regex::new(r"\b([a-z_][a-z0-9_]*)\s*:\s*&(?:mut\s+)?(?:[a-z_:]*::)?WriteTransaction\b")
            .expect("regex");
    let chained =
        Regex::new(r"\bbegin_write\s*\(\s*\)\s*\??\s*\.\s*open_table\s*\(").expect("regex");
    let lines: Vec<String> = text
        .lines()
        .map(|l| line_comment.replace(l, "").into_owned())
        .collect();
    let joined = lines.join("\n");
    let mut names: BTreeSet<String> = BTreeSet::new();
    for re in [&binding, &param] {
        names.extend(re.captures_iter(&joined).map(|c| c[1].to_string()));
    }
    let write_call = (!names.is_empty()).then(|| {
        let alternation: Vec<String> = names.iter().map(|n| regex::escape(n)).collect();
        Regex::new(&format!(
            r"\b(?:{})\s*\.\s*open_table\s*\(",
            alternation.join("|")
        ))
        .expect("regex")
    });
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            chained.is_match(line) || write_call.as_ref().is_some_and(|re| re.is_match(line))
        })
        .map(|(i, _)| i + 1)
        .collect()
}

/// `<path> <reason>` lines: the entries and the errors.
pub fn parse_allow(text: &str) -> (BTreeMap<String, String>, Vec<String>) {
    let mut entries = BTreeMap::new();
    let mut errors = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (path, reason) = line.split_once(' ').unwrap_or((line, ""));
        if reason.trim().is_empty() {
            errors.push(format!(
                "{CODE} {ALLOW}:{}: allow entry {path} has no reason",
                index + 1
            ));
            continue;
        }
        entries.insert(path.to_string(), reason.trim().to_string());
    }
    (entries, errors)
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Every violation under `root` given the allow file text.
pub fn scan(root: &Path, allow_text: &str) -> Result<Vec<String>, String> {
    let (allowed, mut errors) = parse_allow(allow_text);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for scan_root in SCAN_ROOTS {
        let base = root.join(scan_root);
        if !base.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        rust_files(&base, &mut files)?;
        for path in files {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if EXEMPT.iter().any(|e| relative.starts_with(e)) || relative.contains("/target/") {
                continue;
            }
            let text =
                String::from_utf8_lossy(&fs::read(&path).map_err(|e| e.to_string())?).into_owned();
            let hits = scan_source(&text);
            if hits.is_empty() {
                continue;
            }
            if allowed.contains_key(&relative) {
                seen.insert(relative);
                continue;
            }
            errors.extend(
                hits.iter()
                    .map(|line| format!("{CODE} {relative}:{line}: direct redb table write")),
            );
        }
    }
    errors.extend(
        allowed
            .keys()
            .filter(|p| !seen.contains(*p))
            .map(|p| format!("{CODE} {ALLOW}: {p} no longer writes a table directly; remove it")),
    );
    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    const DIRECT: &str = "\nfn seed(db: &redb::Database) {\n    let write = db.begin_write().unwrap();\n    // write.open_table(COMMENTED) is not a call\n    write.open_table(TABLE).unwrap();\n}\n";
    const READER: &str = "fn r(db: &redb::Database) {\n    let read = db.begin_read().unwrap();\n    read.open_table(T);\n}\n";

    #[test]
    fn direct_write_is_reported_and_allowlist_needs_reason() {
        let tree = Tree::new(&[
            ("crates/medulla-demo/src/lib.rs", DIRECT),
            ("crates/medulla-transaction/src/lib.rs", DIRECT),
            ("src/read.rs", READER),
        ]);
        let root = tree.path();
        let expected = format!("{CODE} crates/medulla-demo/src/lib.rs:5: direct redb table write");
        assert_eq!(scan(root, "").expect("scans"), [expected]);

        assert!(
            scan(root, "crates/medulla-demo/src/lib.rs seeds a fixture\n")
                .expect("scans")
                .is_empty()
        );

        let no_reason = scan(root, "crates/medulla-demo/src/lib.rs\n").expect("scans");
        assert!(no_reason.iter().any(|e| e.contains("has no reason")));
        assert!(no_reason.iter().any(|e| e.contains("lib.rs:5")));

        let stale = scan(
            root,
            "crates/medulla-demo/src/lib.rs seeds a fixture\nsrc/read.rs not a writer\n",
        )
        .expect("scans");
        assert_eq!(
            stale,
            [format!(
                "{CODE} {ALLOW}: src/read.rs no longer writes a table directly; remove it"
            )]
        );
    }

    #[test]
    fn the_repository_allow_file_matches_the_tree() {
        let root = crate::workspace_root();
        let allow = fs::read_to_string(root.join(ALLOW)).expect("allow file");
        assert_eq!(scan(&root, &allow).expect("scans"), Vec::<String>::new());
    }
}
