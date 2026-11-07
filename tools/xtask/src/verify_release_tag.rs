//! MEDULLA-CONTRACT-R053: fail publication if the canonical remote tag no longer names this run's source.
//!
//! `cargo xtask verify-release-tag`, run by the publish action on a canonical tag push. It reads
//! `GITHUB_EVENT_NAME`, `GITHUB_REPOSITORY`, `GITHUB_REF` and `GITHUB_SHA`, asks the canonical
//! remote for the tag and its peeled commit, and requires both the checkout and the remote tag to
//! name the event's source commit.

use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

pub const NAME: &str = "verify-release-tag";
pub const SUMMARY: &str = "the canonical remote release tag still names this run's source";

const REMOTE: &str = "https://github.com/Knuckles-Team/medulla.git";

pub fn run(_root: &Path, _args: &[String]) -> Result<(), String> {
    let env = |name: &str| std::env::var(name).unwrap_or_default();
    if env("GITHUB_EVENT_NAME") != "push" || env("GITHUB_REPOSITORY") != "Knuckles-Team/medulla" {
        return Err("publication requires canonical tag push".into());
    }
    let (reference, source) = (env("GITHUB_REF"), env("GITHUB_SHA"));
    let head = git(&["rev-parse", "HEAD"])?;
    if head.trim() != source {
        return Err("checkout differs from event source".into());
    }
    let peeled = format!("{reference}^{{}}");
    let lines = git(&["ls-remote", "--exit-code", REMOTE, &reference, &peeled])?;
    verify(&lines, &reference, &source)?;
    println!("Canonical release tag still names {source}");
    Ok(())
}

fn git(args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("git {} failed", args[0]));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Checks the `ls-remote` rows for a tag ref against the source commit.
pub fn verify(lines: &str, reference: &str, source: &str) -> Result<(), String> {
    let tag = Regex::new(r"^refs/tags/v[0-9][A-Za-z0-9.+_-]*$").expect("regex");
    let sha = Regex::new(r"^[0-9a-f]{40}$").expect("regex");
    if !tag.is_match(reference) {
        return Err("invalid release tag".into());
    }
    if !sha.is_match(source) {
        return Err("invalid source commit".into());
    }
    let rows: Vec<Vec<&str>> = lines
        .lines()
        .map(|l| l.split_whitespace().collect())
        .collect();
    if rows.iter().any(|r| r.len() != 2) {
        return Err("missing/ambiguous canonical tag".into());
    }
    let refs: BTreeMap<&str, &str> = rows.iter().map(|r| (r[1], r[0])).collect();
    let peeled = format!("{reference}^{{}}");
    let allowed = |name: &&str| *name == reference || *name == peeled;
    if refs.len() != rows.len() || !refs.contains_key(reference) || !refs.keys().all(allowed) {
        return Err("missing/ambiguous canonical tag".into());
    }
    let named = refs.get(peeled.as_str()).unwrap_or(&refs[reference]);
    if *named != source {
        return Err("canonical release tag moved".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotated_and_lightweight_tag() {
        let reference = "refs/tags/v2.27.0";
        let source = "a".repeat(40);
        verify(&format!("{source}\t{reference}"), reference, &source).expect("lightweight");
        let annotated = format!(
            "{}\t{reference}\n{source}\t{reference}^{{}}",
            "b".repeat(40)
        );
        verify(&annotated, reference, &source).expect("annotated");
        for lines in [
            String::new(),
            format!("{}\t{reference}", "b".repeat(40)),
            format!("{source}\t{reference}\n{source}\t{reference}"),
        ] {
            assert!(verify(&lines, reference, &source).is_err(), "{lines:?}");
        }
        assert!(verify(
            &format!("{source}\trefs/heads/main"),
            "refs/heads/main",
            &source
        )
        .is_err());
    }
}
