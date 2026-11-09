//! `cargo xtask crate-isolation [--clippy] [--keep-going]`: every workspace crate builds alone.
//!
//! Whole-workspace builds unify cargo features, so a crate that relies on a feature another
//! member enables compiles there yet fails when built by itself. This check runs
//! `cargo check --locked -p <crate> --all-targets` (and, with `--clippy`,
//! `cargo clippy --locked -p <crate> --all-targets -- -D warnings`) once per member, each as its
//! own invocation, and fails listing every crate that does not build alone. Heavy: run it on a
//! build host, nightly or per tag, not per pull request.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

pub const NAME: &str = "crate-isolation";
pub const SUMMARY: &str = "every workspace crate builds alone with its own default features";

fn members(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("cargo")
        .args(["metadata", "--no-deps", "--locked", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("CANNOT RUN: cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "CANNOT RUN: cargo metadata failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let meta: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("CANNOT RUN: cargo metadata output: {e}"))?;
    let mut names: Vec<String> = meta["packages"]
        .as_array()
        .ok_or("CANNOT RUN: cargo metadata has no packages")?
        .iter()
        .filter_map(|p| p["name"].as_str().map(str::to_owned))
        .collect();
    names.sort();
    Ok(names)
}

fn builds_alone(root: &Path, krate: &str, clippy: bool) -> bool {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root);
    if clippy {
        cmd.args(["clippy", "--locked", "-p", krate, "--all-targets", "--", "-D", "warnings"]);
    } else {
        cmd.args(["check", "--locked", "-p", krate, "--all-targets"]);
    }
    cmd.status().is_ok_and(|s| s.success())
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let clippy = args.iter().any(|a| a == "--clippy");
    let names = members(root)?;
    let mut failed = Vec::new();
    for name in &names {
        eprintln!("crate-isolation: {name}");
        if !builds_alone(root, name, false) || (clippy && !builds_alone(root, name, true)) {
            failed.push(name.clone());
        }
    }
    if failed.is_empty() {
        println!("crate-isolation: PASS: {} crate(s) each build alone", names.len());
        return Ok(());
    }
    Err(format!(
        "  {} of {} crate(s) do not build alone: {}",
        failed.len(),
        names.len(),
        failed.join(", ")
    ))
}
