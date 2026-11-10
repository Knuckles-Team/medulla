//! `cargo xtask <check>`: engine-specific repository checks and certification campaigns.
//!
//! One module per check; each module exposes `NAME`, `SUMMARY` and `run`. A check that every
//! repository needs is a `pipelines-hook` mode instead and does not belong here.

mod analysis;
mod build_profiles;
mod canonical_property_schema;
mod ci_affected_crates;
mod ci_change_scope;
mod crate_isolation;
mod decide_budgets;
mod dispatch_decomposition;
mod fanout_host_groups;
mod fetch_server_binary;
mod inject_server_binary;
mod integration_baseline;
mod mint_lease_call_sites;
mod orphan_modules;
mod p2_analytics_reasoning_architecture;
mod p2_modality_architecture;
mod repo_root_parity;
mod sdist_completeness;
mod sole_write_envelope;
#[cfg(test)]
mod tempdir;
mod verify_release_tag;
mod wheel_completeness;
mod wheel_privacy;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// A check: the subcommand name, a one-line summary, and the function that runs it.
struct Check {
    name: &'static str,
    summary: &'static str,
    run: fn(&Path, &[String]) -> Result<(), String>,
    /// Needs an argument or an artifact (or is not yet green), so `checks` lists it but does not run it.
    standalone: bool,
    /// Reads the tracked-file list, so `checks` skips it in a tree that has no `.git`.
    needs_git: bool,
}

/// Every registered check, in the order `cargo xtask --list` prints them.
const CHECKS: &[Check] = &[
    Check {
        name: build_profiles::NAME,
        summary: build_profiles::SUMMARY,
        run: build_profiles::run,
        standalone: false,
        needs_git: false,
    },
    Check {
        name: canonical_property_schema::NAME,
        summary: canonical_property_schema::SUMMARY,
        run: canonical_property_schema::run,
        standalone: false,
        needs_git: false,
    },
    Check {
        name: ci_affected_crates::NAME,
        summary: ci_affected_crates::SUMMARY,
        run: ci_affected_crates::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: ci_change_scope::NAME,
        summary: ci_change_scope::SUMMARY,
        run: ci_change_scope::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: decide_budgets::NAME,
        summary: decide_budgets::SUMMARY,
        run: decide_budgets::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: dispatch_decomposition::NAME,
        summary: dispatch_decomposition::SUMMARY,
        run: dispatch_decomposition::run,
        standalone: false,
        needs_git: false,
    },
    Check {
        name: fanout_host_groups::NAME,
        summary: fanout_host_groups::SUMMARY,
        run: fanout_host_groups::run,
        standalone: false,
        needs_git: false,
    },
    Check {
        name: fetch_server_binary::NAME,
        summary: fetch_server_binary::SUMMARY,
        run: fetch_server_binary::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: inject_server_binary::NAME,
        summary: inject_server_binary::SUMMARY,
        run: inject_server_binary::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: integration_baseline::NAME,
        summary: integration_baseline::SUMMARY,
        run: integration_baseline::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: mint_lease_call_sites::NAME,
        summary: mint_lease_call_sites::SUMMARY,
        run: mint_lease_call_sites::run,
        standalone: false,
        needs_git: false,
    },
    Check {
        name: orphan_modules::NAME,
        summary: orphan_modules::SUMMARY,
        run: orphan_modules::run,
        standalone: false,
        needs_git: true,
    },
    Check {
        name: p2_analytics_reasoning_architecture::NAME,
        summary: p2_analytics_reasoning_architecture::SUMMARY,
        run: p2_analytics_reasoning_architecture::run,
        standalone: false,
        needs_git: false,
    },
    Check {
        name: p2_modality_architecture::NAME,
        summary: p2_modality_architecture::SUMMARY,
        run: p2_modality_architecture::run,
        standalone: false,
        needs_git: false,
    },
    Check {
        name: repo_root_parity::NAME,
        summary: repo_root_parity::SUMMARY,
        run: repo_root_parity::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: sdist_completeness::NAME,
        summary: sdist_completeness::SUMMARY,
        run: sdist_completeness::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: sole_write_envelope::NAME,
        summary: sole_write_envelope::SUMMARY,
        run: sole_write_envelope::run,
        standalone: false,
        needs_git: false,
    },
    Check {
        name: verify_release_tag::NAME,
        summary: verify_release_tag::SUMMARY,
        run: verify_release_tag::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: wheel_completeness::NAME,
        summary: wheel_completeness::SUMMARY,
        run: wheel_completeness::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: crate_isolation::NAME,
        summary: crate_isolation::SUMMARY,
        run: crate_isolation::run,
        standalone: true,
        needs_git: false,
    },
    Check {
        name: wheel_privacy::NAME,
        summary: wheel_privacy::SUMMARY,
        run: wheel_privacy::run,
        standalone: true,
        needs_git: false,
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
    if name == "checks" {
        return run_all(&workspace_root());
    }
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

/// `cargo xtask checks`: every repository check, one verdict line each; fails if any fails.
fn run_all(root: &Path) -> ExitCode {
    let mut failed = Vec::new();
    let has_git = root.join(".git").exists();
    for check in CHECKS.iter().filter(|c| !c.standalone) {
        if check.needs_git && !has_git {
            eprintln!("xtask checks: {:<32} skipped (no .git)", check.name);
            continue;
        }
        match (check.run)(root, &[]) {
            Ok(()) => eprintln!("xtask checks: {:<32} ok", check.name),
            Err(message) => {
                eprintln!("{message}\nxtask checks: {:<32} FAIL", check.name);
                failed.push(check.name);
            }
        }
    }
    if failed.is_empty() {
        return ExitCode::SUCCESS;
    }
    eprintln!(
        "xtask checks: {} failed: {}",
        failed.len(),
        failed.join(", ")
    );
    ExitCode::FAILURE
}

fn usage() -> ExitCode {
    eprintln!("usage: cargo xtask <check> [args]\n       cargo xtask checks   (every check that needs no argument)\n\nchecks:");
    for check in CHECKS {
        let note = if check.standalone {
            " [standalone: not run by `checks`]"
        } else {
            ""
        };
        eprintln!("  {:<32} {}{note}", check.name, check.summary);
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
