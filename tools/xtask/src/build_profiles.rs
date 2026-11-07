//! MEDULLA-CONTRACT-R056.14: the Cargo build profiles keep iteration fast.
//!
//! Reads the root `Cargo.toml` profiles and the CI workflow and reports one finding per
//! deviation: `[profile.dev]` and `[profile.test]` need `codegen-units = 256`,
//! `incremental = true` and debug information no richer than line tables; CI builds through
//! `sccache`; the facade `--timings` report is the uploaded artifact of job `gates-facade`.

use std::fs;
use std::path::Path;
use toml_edit::{DocumentMut, Item};

pub const NAME: &str = "build-profiles";
pub const SUMMARY: &str = "dev/test profiles, compile cache and facade timings match the standard";

const CODEGEN_UNITS: i64 = 256;
const LINE_TABLES: &str = "line-tables-only";
const CI_WORKFLOW: &str = ".github/workflows/ci.yml";

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    let findings = profiles_match_the_standard(root)?;
    if findings.is_empty() {
        println!("build profiles match the standard");
        return Ok(());
    }
    Err(findings.join("\n"))
}

/// Whether a `debug` value is no richer than line tables.
fn lean_debug(item: Option<&Item>, allow_off: bool) -> bool {
    match item {
        Some(i) if i.as_str() == Some(LINE_TABLES) => true,
        Some(i) if allow_off => {
            i.as_integer() == Some(0)
                || i.as_bool() == Some(false)
                || matches!(i.as_str(), Some("none" | "line-directives-only"))
        }
        None => false,
        Some(_) => false,
    }
}

fn check_profile(doc: &DocumentMut, name: &str, allow_off: bool, findings: &mut Vec<String>) {
    let Some(profile) = doc.get("profile").and_then(|p| p.get(name)) else {
        findings.push(format!("Cargo.toml: [profile.{name}] is missing"));
        return;
    };
    if profile.get("codegen-units").and_then(Item::as_integer) != Some(CODEGEN_UNITS) {
        findings.push(format!(
            "[profile.{name}]: codegen-units must be {CODEGEN_UNITS}"
        ));
    }
    if profile.get("incremental").and_then(Item::as_bool) != Some(true) {
        findings.push(format!("[profile.{name}]: incremental must be true"));
    }
    if !lean_debug(profile.get("debug"), allow_off) {
        findings.push(format!(
            "[profile.{name}]: debug is richer than line tables"
        ));
    }
}

/// The text of one top-level (two-space indented) workflow job, or empty when absent.
fn job_block(workflow: &str, job: &str) -> String {
    let header = format!("  {job}:");
    let mut block = Vec::new();
    let mut inside = false;
    for line in workflow.lines() {
        if inside {
            let top_level =
                line.starts_with("  ") && !line.starts_with("   ") && !line.trim().is_empty();
            if top_level {
                break;
            }
        } else if line.trim_end() == header {
            inside = true;
        }
        if inside {
            block.push(line);
        }
    }
    block.join("\n")
}

pub fn profiles_match_the_standard(root: &Path) -> Result<Vec<String>, String> {
    let manifest =
        fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let doc: DocumentMut = manifest.parse().map_err(|e| format!("Cargo.toml: {e}"))?;
    let mut findings = Vec::new();
    check_profile(&doc, "dev", false, &mut findings);
    check_profile(&doc, "test", true, &mut findings);
    let workflow = fs::read_to_string(root.join(CI_WORKFLOW)).unwrap_or_default();
    if !workflow.contains("RUSTC_WRAPPER: sccache") {
        findings.push(format!(
            "{CI_WORKFLOW}: the shared compile cache (sccache) is off"
        ));
    }
    let facade = job_block(&workflow, "gates-facade");
    if !facade.contains("--timings") || !facade.contains("upload-artifact") {
        findings.push(format!(
            "{CI_WORKFLOW}: job gates-facade must build with --timings and upload the report"
        ));
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    const WORKFLOW: &str = "jobs:\n  gates-facade:\n    steps:\n      - run: cargo build --timings\n      - uses: actions/upload-artifact@v4\n  other:\n    env:\n      RUSTC_WRAPPER: sccache\n";
    const GOOD: &str = "[profile.dev]\ncodegen-units = 256\nincremental = true\ndebug = \"line-tables-only\"\n[profile.test]\ncodegen-units = 256\nincremental = true\ndebug = 0\n";

    #[test]
    fn profiles_match_the_standard_for_a_conforming_tree() {
        let t = Tree::new(&[("Cargo.toml", GOOD), (CI_WORKFLOW, WORKFLOW)]);
        assert_eq!(
            profiles_match_the_standard(t.path()).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_rich_debug_level_and_a_missing_cache_are_findings() {
        let bad = GOOD.replace("line-tables-only", "full");
        let t = Tree::new(&[
            ("Cargo.toml", &bad),
            (CI_WORKFLOW, "jobs:\n  gates-facade:\n    steps: []\n"),
        ]);
        let findings = profiles_match_the_standard(t.path()).unwrap();
        assert_eq!(findings.len(), 3, "{findings:#?}");
    }

    #[test]
    fn profiles_match_the_standard_holds_for_this_repository() {
        let findings = profiles_match_the_standard(&crate::workspace_root()).unwrap();
        assert!(findings.is_empty(), "{findings:#?}");
    }
}
