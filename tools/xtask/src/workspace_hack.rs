//! `cargo xtask workspace-hack [--root <dir>]`: the generated workspace-hack crate is current.
//!
//! Cargo unifies dependency features per build, so `cargo build -p a` and `cargo build -p b` can
//! build the same dependency with different feature sets and rebuild it. A generated crate
//! (`medulla-workspace-hack`, made by `cargo-hakari`) names the unified feature set and every
//! member depends on it. This check regenerates and diffs it: it runs
//! `cargo hakari generate --diff`, `cargo hakari manage-deps --dry-run` and `cargo hakari verify`,
//! and fails with the diff when the checked-in crate or a member's dependency line is stale.
//! The tool version is checked first; any version but the pinned one refuses with
//! `HAKARI_VERSION_MISMATCH`. `--root` points the check at another workspace (the fixture tests).

use std::path::{Path, PathBuf};
use std::process::Command;

pub const NAME: &str = "workspace-hack";
pub const SUMMARY: &str =
    "the generated workspace-hack crate and every member's dependency on it are current";

/// The `cargo-hakari` release this repository generates with (`scripts/bootstrap.sh` installs it).
pub const PINNED_VERSION: &str = "0.9.39";

/// Accept exactly the pinned version in `cargo hakari --version` output (`cargo-hakari 0.9.39`).
pub fn check_version(output: &str) -> Result<(), String> {
    let found = output.split_whitespace().last().unwrap_or("");
    if found == PINNED_VERSION {
        return Ok(());
    }
    Err(format!(
        "HAKARI_VERSION_MISMATCH: cargo-hakari {PINNED_VERSION} is pinned, found `{}`; run \
         `cargo install --locked cargo-hakari --version ={PINNED_VERSION}`",
        output.trim()
    ))
}

fn hakari(root: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let out = Command::new("cargo")
        .arg("hakari")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("CANNOT RUN: cargo hakari {}: {e}", args.join(" ")))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

fn target_root(root: &Path, args: &[String]) -> Result<PathBuf, String> {
    match args {
        [] => Ok(root.to_path_buf()),
        [flag, dir] if flag == "--root" => Ok(PathBuf::from(dir)),
        _ => Err("usage: cargo xtask workspace-hack [--root <dir>]".to_string()),
    }
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let root = target_root(root, args)?;
    let (ok, text) = hakari(&root, &["--version"])?;
    if !ok {
        return Err(format!(
            "CANNOT RUN: cargo-hakari is not installed:\n{text}"
        ));
    }
    check_version(&text)?;
    let steps: [(&str, &[&str]); 3] = [
        ("generated crate", &["generate", "--diff"]),
        ("member dependency lines", &["manage-deps", "--dry-run"]),
        ("feature unification", &["verify"]),
    ];
    let mut stale = Vec::new();
    for (what, step) in steps {
        let (ok, text) = hakari(&root, step)?;
        if !ok {
            stale.push(format!(
                "  {what} is stale (cargo hakari {}):\n{text}",
                step.join(" ")
            ));
        }
    }
    if stale.is_empty() {
        println!("workspace-hack: PASS: the generated crate and every dependency line are current");
        return Ok(());
    }
    stale.push("  run `cargo hakari generate && cargo hakari manage-deps` and commit".to_string());
    Err(stale.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_version_passes() {
        assert!(check_version(&format!("cargo-hakari {PINNED_VERSION}\n")).is_ok());
    }

    #[test]
    fn any_other_version_refuses_with_the_code() {
        for other in ["cargo-hakari 0.9.38", "cargo-hakari 0.9.390", "", "0.10.0"] {
            let err = check_version(other).unwrap_err();
            assert!(err.starts_with("HAKARI_VERSION_MISMATCH"), "{err}");
        }
    }

    #[test]
    fn root_flag_selects_another_workspace() {
        let default = target_root(Path::new("/a"), &[]).unwrap();
        assert_eq!(default, Path::new("/a"));
        let given = target_root(Path::new("/a"), &["--root".into(), "/b".into()]).unwrap();
        assert_eq!(given, Path::new("/b"));
        assert!(target_root(Path::new("/a"), &["--bogus".into()]).is_err());
    }
}
