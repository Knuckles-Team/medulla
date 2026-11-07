//! `cargo xtask <check>`: engine-specific repository checks and certification campaigns.
//!
//! One module per check; each module exposes `NAME`, `SUMMARY` and `run`. A check that every
//! repository needs is a `pipelines-hook` mode instead and does not belong here.

mod build_profiles;
mod repo_root_parity;
mod spec_paths;
#[cfg(test)]
mod tempdir;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// A check: the subcommand name, a one-line summary, and the function that runs it.
struct Check {
    name: &'static str,
    summary: &'static str,
    run: fn(&Path, &[String]) -> Result<(), String>,
}

/// Every registered check, in the order `cargo xtask --list` prints them.
const CHECKS: &[Check] = &[
    Check {
        name: build_profiles::NAME,
        summary: build_profiles::SUMMARY,
        run: build_profiles::run,
    },
    Check {
        name: repo_root_parity::NAME,
        summary: repo_root_parity::SUMMARY,
        run: repo_root_parity::run,
    },
    Check {
        name: spec_paths::NAME,
        summary: spec_paths::SUMMARY,
        run: spec_paths::run,
    },
];

/// The workspace root: the parent of `tools/`, found from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("tools/xtask sits two levels below the workspace root")
        .to_path_buf()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = args.first() else {
        return usage();
    };
    if name == "--list" || name == "help" || name == "--help" {
        return usage();
    }
    let Some(check) = CHECKS.iter().find(|c| c.name == name) else {
        eprintln!("xtask: unknown check `{name}`");
        return usage();
    };
    match (check.run)(&workspace_root(), &args[1..]) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask {name}: FAIL\n{message}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: cargo xtask <check> [args]\n\nchecks:");
    for check in CHECKS {
        eprintln!("  {:<32} {}", check.name, check.summary);
    }
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_names_are_unique() {
        let mut names: Vec<&str> = CHECKS.iter().map(|c| c.name).collect();
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total);
    }

    #[test]
    fn workspace_root_holds_the_workspace_manifest() {
        assert!(workspace_root().join("Cargo.toml").is_file());
    }
}
