//! MEDULLA-CONTRACT-R053.7: conservatively identify documentation-only pull requests.
//!
//! `cargo xtask ci-change-scope --event E [--base SHA --head SHA] [--path-gates]`. Prints
//! `code=true|false`; with `--path-gates`, also one `<gate>=true|false` line per pull-request path
//! gate. Only a complete, verified pull-request diff of plain-mode documentation files skips the
//! code jobs; unknown refs, unavailable history and empty comparisons run full CI.

use std::path::Path;
use std::process::Command;

pub const NAME: &str = "ci-change-scope";
pub const SUMMARY: &str = "select code and path-gated CI jobs for a pull-request diff";

const ROOT_DOCUMENTS: [&str; 5] = [
    "README.md",
    "CHANGELOG.md",
    "CONTRIBUTING.md",
    "AGENTS.md",
    "CLAUDE.md",
];
const TOOLCHAIN_PATHS: [&str; 2] = ["Cargo.lock", "rust-toolchain.toml"];
/// Any change to CI wiring or to this selector runs every path-gated job.
const ALL_GATES_PATHS: [&str; 2] = [".github/", "tools/xtask/src/ci_change_scope.rs"];

/// Pull-request jobs that run only when a path they exercise changed. Push, schedule and
/// dispatch events always run them.
fn path_gate_table() -> Vec<(&'static str, Vec<&'static str>)> {
    let with_toolchain = |paths: &[&'static str]| -> Vec<&'static str> {
        paths
            .iter()
            .chain(TOOLCHAIN_PATHS.iter())
            .copied()
            .collect()
    };
    vec![
        (
            "clients",
            with_toolchain(&[
                "clients/",
                "contract/",
                "crates/medulla-method-codec/",
                "crates/medulla-types/",
                "scripts/build_method_codec_wasm.py",
            ]),
        ),
        (
            "whisper",
            with_toolchain(&[
                "crates/medulla-asr-whisper/",
                "crates/medulla-audio/",
                "crates/medulla-resource/",
                "scripts/fetch_whisper_test_fixture.sh",
            ]),
        ),
        (
            "ontology",
            with_toolchain(&[
                "crates/medulla-core/ontology/",
                "src/server/graph_schema/",
                "crates/medulla-h-admin/src/graph_schema.rs",
            ]),
        ),
        (
            "tts",
            with_toolchain(&[
                "crates/medulla-tts-piper/",
                "crates/medulla-audio/",
                "scripts/fetch_onnxruntime.sh",
            ]),
        ),
        (
            "packaging",
            with_toolchain(&[
                "crates/medulla-numeric/",
                "crates/medulla-pyengine/",
                "pyproject.toml",
                "build.rs",
                "build_backend.py",
                "Cargo.toml",
                "scripts/inject_numeric_kernel.py",
                "scripts/inject_ontology.py",
                "scripts/inject_server_binary.py",
                "scripts/build_release_server.sh",
                "scripts/strip_wheel_executables.py",
                "scripts/normalize_wheel_sbom.py",
                "scripts/normalize_wheel_build_paths.py",
                "tools/xtask/src/wheel_privacy.rs",
                "tools/xtask/src/wheel_completeness.rs",
                "scripts/configure_rust_path_remap.py",
            ]),
        ),
    ]
}

pub fn run(_root: &Path, args: &[String]) -> Result<(), String> {
    let (mut event, mut base, mut head, mut gates) = (None, String::new(), String::new(), false);
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |what: &str| iter.next().cloned().ok_or(format!("{what} needs a value"));
        match arg.as_str() {
            "--event" => event = Some(value("--event")?),
            "--base" => base = value("--base")?,
            "--head" => head = value("--head")?,
            "--path-gates" => gates = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let event = event.ok_or(
        "usage: cargo xtask ci-change-scope --event E [--base SHA --head SHA] [--path-gates]",
    )?;
    let code = requires_code(&event, &base, &head, &git);
    println!("code={code}");
    if gates {
        let selected = if code {
            path_gates(&event, &base, &head, &git)
        } else {
            path_gate_table()
                .into_iter()
                .map(|(g, _)| (g, false))
                .collect()
        };
        for (gate, on) in selected {
            println!("{gate}={on}");
        }
    }
    Ok(())
}

/// Runs `git <args>` and returns its stdout, or `None` when git is unavailable or fails.
fn git(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn is_sha(reference: &str) -> bool {
    reference.len() == 40
        && reference
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn posix_parts(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect()
}

/// Only non-executable prose and generated spec status can bypass builds.
pub fn documentation_path(path: &str) -> bool {
    let parts = posix_parts(path);
    if ROOT_DOCUMENTS.contains(&path) {
        return true;
    }
    if parts.len() < 2 || !["docs", "specs", ".specify", "architecture"].contains(&parts[0]) {
        return false;
    }
    path.ends_with(".md") || (parts.len() == 3 && parts[0] == "specs" && parts[2] == "status.json")
}

fn raw_entry_is_documentation(metadata: &[u8], path: &[u8], predicate: fn(&str) -> bool) -> bool {
    let fields: Vec<&[u8]> = metadata
        .split(|b| b.is_ascii_whitespace())
        .filter(|f| !f.is_empty())
        .collect();
    if fields.len() != 5 || !fields[0].starts_with(b":") {
        return false;
    }
    let modes = [&fields[0][1..], fields[1]];
    let plain = |m: &[u8]| m == b"100644" || m == b"000000";
    if !modes.iter().all(|m| plain(m)) || !modes.contains(&&b"100644"[..]) {
        return false;
    }
    if !matches!(fields[4], b"A" | b"M" | b"D") {
        return false;
    }
    std::str::from_utf8(path).is_ok_and(predicate)
}

/// Reads `--raw -z --no-renames` output, retaining deletions and executable modes.
pub fn documentation_diff(raw: &[u8], predicate: fn(&str) -> bool) -> bool {
    let fields: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
    if !fields.last().is_some_and(|f| f.is_empty()) || fields.len() < 3 || fields.len() % 2 != 1 {
        return false;
    }
    fields[..fields.len() - 1]
        .chunks(2)
        .all(|pair| raw_entry_is_documentation(pair[0], pair[1], predicate))
}

type Git<'a> = &'a dyn Fn(&[&str]) -> Option<Vec<u8>>;

/// Unknown refs, unavailable history and empty comparisons run full CI.
pub fn requires_code(event: &str, base: &str, head: &str, git: Git) -> bool {
    if event != "pull_request" || !is_sha(base) || !is_sha(head) {
        return true;
    }
    let range = format!("{base}...{head}");
    match git(&["diff", "--raw", "--no-renames", "-z", &range]) {
        Some(raw) => !documentation_diff(&raw, documentation_path),
        None => true,
    }
}

fn matches_any(path: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| {
        if p.ends_with('/') {
            path.starts_with(p)
        } else {
            path == *p
        }
    })
}

/// Path-gated jobs for a known pull-request file list.
pub fn gates_for_paths(paths: &[String]) -> Vec<(&'static str, bool)> {
    let table = path_gate_table();
    if paths.iter().any(|p| matches_any(p, &ALL_GATES_PATHS)) {
        return table.into_iter().map(|(g, _)| (g, true)).collect();
    }
    table
        .into_iter()
        .map(|(gate, prefixes)| (gate, paths.iter().any(|p| matches_any(p, &prefixes))))
        .collect()
}

/// Unknown refs, unavailable history and non-PR events run every gate.
pub fn path_gates(event: &str, base: &str, head: &str, git: Git) -> Vec<(&'static str, bool)> {
    let every = || -> Vec<(&'static str, bool)> {
        path_gate_table()
            .into_iter()
            .map(|(g, _)| (g, true))
            .collect()
    };
    if event != "pull_request" || !is_sha(base) || !is_sha(head) {
        return every();
    }
    let range = format!("{base}...{head}");
    let Some(out) = git(&["diff", "--name-only", "--no-renames", "-z", &range]) else {
        return every();
    };
    let Ok(text) = String::from_utf8(out) else {
        return every();
    };
    let paths: Vec<String> = text
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    if paths.is_empty() {
        return every();
    }
    gates_for_paths(&paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    fn raw(path: &str, old: &str, new: &str, status: &str) -> Vec<u8> {
        format!(":{old} {new} abc def {status}\0{path}\0").into_bytes()
    }
    fn m(path: &str) -> Vec<u8> {
        raw(path, "100644", "100644", "M")
    }
    fn docs(raw: &[u8]) -> bool {
        documentation_diff(raw, documentation_path)
    }
    fn gates(paths: &[&str]) -> Vec<&'static str> {
        let owned: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        gates_for_paths(&owned)
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(g, _)| g)
            .collect()
    }
    fn no_git(_: &[&str]) -> Option<Vec<u8>> {
        None
    }
    const NONE: Git<'static> = &no_git;
    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn documentation_only_diff() {
        for path in [
            "README.md",
            "docs/guide.md",
            "specs/demo/spec.md",
            "specs/demo/status.json",
        ] {
            assert!(docs(&m(path)), "{path}");
            assert!(docs(&raw(path, "100644", "000000", "D")), "{path}");
            assert!(docs(&raw(path, "000000", "100644", "A")), "{path}");
        }
    }

    #[test]
    fn unknown_or_executable_content_runs_code() {
        for path in [
            "src/lib.rs",
            "Cargo.lock",
            ".config/pre-commit.yaml",
            ".github/workflows/ci.yml",
            "docs/plugin.py",
            "specs/build.rs",
            ".specify/scripts/setup.sh",
            "docs/data.json",
            "crates/runtime/fixture.md",
            "architecture/component-registry.yml",
        ] {
            assert!(!docs(&m(path)), "{path}");
        }
    }

    #[test]
    fn executable_symlink_and_submodule_modes_run_code() {
        for mode in ["100755", "120000", "160000"] {
            assert!(!docs(&raw("docs/guide.md", mode, "100644", "M")));
            assert!(!docs(&raw("docs/guide.md", "100644", mode, "M")));
        }
    }

    #[test]
    fn code_renamed_to_docs_still_runs_code() {
        let mut diff = raw("src/old.rs", "100644", "000000", "D");
        diff.extend(raw("docs/new.md", "000000", "100644", "A"));
        assert!(!docs(&diff));
    }

    #[test]
    fn incomplete_diff_runs_code() {
        for data in [&b""[..], b"broken", b"\0", b":bad\0docs/a.md\0"] {
            assert!(!docs(data));
        }
    }

    #[test]
    fn path_gates_select_only_the_changed_surfaces() {
        let everything = vec!["clients", "whisper", "ontology", "tts", "packaging"];
        assert!(gates(&["src/lib.rs"]).is_empty());
        assert!(gates(&["crates/medulla-sql/src/lib.rs"]).is_empty());
        assert_eq!(gates(&["clients/js/index.mjs"]), ["clients"]);
        assert_eq!(gates(&["crates/medulla-types/src/lib.rs"]), ["clients"]);
        assert_eq!(
            gates(&["crates/medulla-asr-whisper/src/lib.rs"]),
            ["whisper"]
        );
        assert_eq!(
            gates(&["crates/medulla-audio/src/lib.rs"]),
            ["whisper", "tts"]
        );
        assert_eq!(gates(&["crates/medulla-tts-piper/src/lib.rs"]), ["tts"]);
        assert_eq!(gates(&["pyproject.toml"]), ["packaging"]);
        assert_eq!(
            gates(&["crates/medulla-pyengine/src/lib.rs"]),
            ["packaging"]
        );
        assert_eq!(
            gates(&["crates/medulla-core/ontology/core.ttl"]),
            ["ontology"]
        );
        assert_eq!(gates(&["src/server/graph_schema/compose.rs"]), ["ontology"]);
        assert_eq!(gates(&["Cargo.lock"]), everything);
        assert_eq!(gates(&[".github/workflows/ci.yml"]), everything);
        assert_eq!(gates(&["tools/xtask/src/ci_change_scope.rs"]), everything);
    }

    #[test]
    fn non_pr_events_run_every_path_gate_and_full_validation() {
        for event in ["push", "workflow_dispatch", "schedule", "merge_group", ""] {
            assert!(path_gates(event, A, B, NONE).iter().all(|(_, on)| *on));
            assert!(requires_code(event, A, B, NONE));
        }
    }

    #[test]
    fn unavailable_or_invalid_history_runs_everything() {
        assert!(path_gates("pull_request", A, B, NONE)
            .iter()
            .all(|(_, on)| *on));
        assert!(path_gates("pull_request", "", B, NONE)
            .iter()
            .all(|(_, on)| *on));
        assert!(requires_code("pull_request", "", B, NONE));
        assert!(requires_code("pull_request", A, B, NONE));
    }

    #[test]
    fn complete_git_diff_has_no_api_file_limit() {
        let mut diff = Vec::new();
        for _ in 0..301 {
            diff.extend(m("docs/a.md"));
        }
        diff.extend(m("src/a.rs"));
        let range = format!("{A}...{B}");
        let fake = move |args: &[&str]| {
            assert_eq!(
                args,
                ["diff", "--raw", "--no-renames", "-z", range.as_str()]
            );
            Some(diff.clone())
        };
        assert!(requires_code("pull_request", A, B, &fake));
    }

    #[test]
    fn a_documentation_diff_skips_the_code_jobs() {
        let fake = |_: &[&str]| Some(m("docs/a.md"));
        assert!(!requires_code("pull_request", A, B, &fake));
    }

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Real git ranges: a mode normalization or a non-plain mode keeps full CI.
    #[test]
    fn normalized_document_modes_keep_real_git_ranges_conservative() {
        for mode in ["100755", "120000", "160000"] {
            let tree = Tree::new(&[
                ("docs/guide.md", "original\n"),
                ("src/lib.rs", "original\n"),
            ]);
            let dir = tree.path();
            let g = |args: &[&str]| git_in(dir, args);
            g(&["init", "-q"]);
            g(&["config", "user.email", "fixture@example.invalid"]);
            g(&["config", "user.name", "Fixture"]);
            g(&["add", "."]);
            g(&["commit", "-qm", "fixture"]);
            let dir_s = dir.display().to_string();
            let in_dir = |args: &[&str]| {
                let mut full = vec!["-C", dir_s.as_str()];
                full.extend_from_slice(args);
                git(&full)
            };
            let scope = |base: &str, head: &str| requires_code("pull_request", base, head, &in_dir);

            g(&["update-index", "--chmod=+x", "docs/guide.md"]);
            g(&["commit", "-qm", "executable documentation baseline"]);
            let executable = g(&["rev-parse", "HEAD"]);
            g(&["update-index", "--chmod=-x", "docs/guide.md"]);
            g(&["commit", "-qm", "normalize documentation mode"]);
            let normalized = g(&["rev-parse", "HEAD"]);
            assert!(scope(&executable, &normalized));

            std::fs::write(dir.join("docs/guide.md"), "updated prose\n").expect("write");
            g(&["add", "docs/guide.md"]);
            g(&["commit", "-qm", "ordinary documentation edit"]);
            let prose = g(&["rev-parse", "HEAD"]);
            assert!(!scope(&normalized, &prose));
            assert!(scope(&executable, &prose));

            match mode {
                "100755" => {
                    g(&["update-index", "--chmod=+x", "docs/guide.md"]);
                }
                "120000" => {
                    std::fs::remove_file(dir.join("docs/guide.md")).expect("rm");
                    std::os::unix::fs::symlink("elsewhere", dir.join("docs/guide.md")).expect("ln");
                    g(&["add", "docs/guide.md"]);
                }
                _ => {
                    g(&[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("160000,{prose},docs/guide.md"),
                    ]);
                }
            }
            g(&["commit", "-qm", "non-document mode requires full CI"]);
            let head = g(&["rev-parse", "HEAD"]);
            assert!(scope(&prose, &head), "mode {mode}");
        }
    }
}
