//! MEDULLA-SDLC-R022: a built wheel is complete before it can be published.
//!
//! `cargo xtask wheel-completeness [--require-engine-kernel] [--require-server-binary] <whl>...`
//! asserts the invariants of a release wheel, each one a regression that shipped:
//! the numeric kernel is present; no console scripts; the python surface (package plus
//! `kvcache` and `skills`) is complete; `RECORD` lists every member with its sha256 and size;
//! `msgpack` is an unconditional requirement and `numpy` is declared nowhere; the in-process
//! engine binding is present when `--require-engine-kernel` is set; exactly one executable
//! server binary is present when `--require-server-binary` is set. Only wheel-relative member
//! names are printed, never a build path.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

pub const NAME: &str = "wheel-completeness";
pub const SUMMARY: &str =
    "a built wheel holds the kernel, package data, a true RECORD and no stray scripts";

const PACKAGE: &str = "medulla";
const KERNEL_SUFFIXES: [&str; 2] = [".so", ".pyd"];
const REQUIRED_SUBPACKAGES: [&str; 2] = ["kvcache", "skills"];
/// PEP 503 normalised names that must be unconditional runtime requirements.
const REQUIRED_BASE_DEPENDENCIES: [&str; 1] = ["msgpack"];
/// NumPy is a developer-only parity reference, never a shipped runtime or extra.
const FORBIDDEN_DEPENDENCIES: [&str; 1] = ["numpy"];
const SERVER_MEMBERS: [&str; 2] = [
    "medulla/_bin/medulla-server",
    "medulla/_bin/medulla-server.exe",
];

#[derive(Default, Clone, Copy)]
pub struct Options {
    pub require_engine_kernel: bool,
    pub require_server_binary: bool,
}

struct Member {
    name: String,
    size: u64,
    mode: u32,
    data: Vec<u8>,
}

pub fn run(_root: &Path, args: &[String]) -> Result<(), String> {
    let mut options = Options::default();
    let mut wheels: Vec<&String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--require-engine-kernel" => options.require_engine_kernel = true,
            "--require-server-binary" => options.require_server_binary = true,
            _ => wheels.push(arg),
        }
    }
    if wheels.is_empty() {
        return Err("usage: cargo xtask wheel-completeness [--require-engine-kernel] [--require-server-binary] <wheel>...".into());
    }
    wheels.sort_by_key(|w| Path::new(w).file_name().map(|n| n.to_owned()));
    let mut incomplete = 0;
    for wheel in &wheels {
        let path = Path::new(wheel);
        let label = path
            .file_name()
            .map_or_else(|| (*wheel).clone(), |n| n.to_string_lossy().into_owned());
        match check_wheel(path, options) {
            Err(_) => {
                println!("FAIL: {label} could not be read as a wheel");
                incomplete += 1;
            }
            Ok(failures) if !failures.is_empty() => {
                incomplete += 1;
                println!("FAIL: {label} is INCOMPLETE and must not be published:");
                for failure in failures {
                    println!("  - {failure}");
                }
            }
            Ok(_) => println!("OK: {label} is complete."),
        }
    }
    if incomplete > 0 {
        return Err(format!(
            "{incomplete} of {} wheel artifact(s) failed the completeness gate - publishing is blocked.",
            wheels.len()
        ));
    }
    println!("OK: all {} wheel artifact(s) are complete.", wheels.len());
    Ok(())
}

fn read_members(path: &Path) -> Result<Vec<Member>, String> {
    let mut archive =
        ZipArchive::new(File::open(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut members = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|e| e.to_string())?;
        let mut data = Vec::new();
        if !file.is_dir() {
            file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        }
        members.push(Member {
            name: file.name().to_owned(),
            size: file.size(),
            mode: file.unix_mode().unwrap_or(0),
            data,
        });
    }
    Ok(members)
}

/// The failed invariants of one wheel (empty means complete).
pub fn check_wheel(path: &Path, options: Options) -> Result<Vec<String>, String> {
    let members = read_members(path)?;
    let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
    let requirements = metadata_requirements(&members)?;
    let mut failures = Vec::new();
    failures.extend(check_numeric_kernel(&names));
    failures.extend(check_engine_kernel(&names, options.require_engine_kernel));
    failures.extend(check_console_scripts(&names));
    failures.extend(check_server_binary(&members, options.require_server_binary));
    failures.extend(check_python_surface(&names));
    failures.extend(check_base_dependencies(&requirements));
    failures.extend(check_forbidden_dependencies(&requirements));
    failures.extend(record_failures(&members)?);
    Ok(failures)
}

fn kernel_members<'a>(names: &[&'a str], stem: &str) -> Vec<&'a str> {
    let prefix = format!("{PACKAGE}/{stem}");
    names
        .iter()
        .copied()
        .filter(|n| n.starts_with(&prefix) && KERNEL_SUFFIXES.iter().any(|s| n.ends_with(s)))
        .collect()
}

fn check_numeric_kernel(names: &[&str]) -> Vec<String> {
    if !kernel_members(names, "numeric").is_empty() {
        return Vec::new();
    }
    vec![format!(
        "no numeric kernel: expected {PACKAGE}/numeric*{{{}}} - the medulla-numeric inject did not run \
         or was undone (import medulla.numeric would fail)",
        KERNEL_SUFFIXES.join(",")
    )]
}

fn check_engine_kernel(names: &[&str], required: bool) -> Vec<String> {
    if !required || !kernel_members(names, "engine").is_empty() {
        return Vec::new();
    }
    vec![format!(
        "no engine kernel: expected {PACKAGE}/engine*{{{}}} - --require-engine-kernel was set but \
         the medulla-pyengine inject did not run or was undone (import medulla.engine would fail)",
        KERNEL_SUFFIXES.join(",")
    )]
}

/// `<name>-<version>.data/scripts/<binary>` is maturin's console-binary location.
fn check_console_scripts(names: &[&str]) -> Vec<String> {
    let scripts = Regex::new(r"^[^/]+\.data/scripts/[^/]+$").expect("static regex");
    names
        .iter()
        .filter(|n| scripts.is_match(n))
        .map(|n| {
            format!(
                "{n} is a console script - the server ships as {}, never under *.data/scripts/",
                SERVER_MEMBERS[0]
            )
        })
        .collect()
}

fn check_server_binary(members: &[Member], required: bool) -> Vec<String> {
    if !required {
        return Vec::new();
    }
    let found: Vec<&Member> = members
        .iter()
        .filter(|m| SERVER_MEMBERS.contains(&m.name.as_str()))
        .collect();
    let [info] = found.as_slice() else {
        return vec![format!(
            "expected exactly one server executable ({}), found {} - the server inject did not run or was undone",
            SERVER_MEMBERS.join(" or "),
            found.len()
        )];
    };
    if info.size == 0 {
        return vec![format!("{} is empty", info.name)];
    }
    if !info.name.ends_with(".exe") && info.mode & 0o111 == 0 {
        return vec![format!(
            "{} has no executable bit; pip would install it 0644",
            info.name
        )];
    }
    Vec::new()
}

fn check_python_surface(names: &[&str]) -> Vec<String> {
    let mut failures = Vec::new();
    if !names.contains(&format!("{PACKAGE}/__init__.py").as_str()) {
        failures.push(format!("no {PACKAGE}/__init__.py - python surface missing"));
    }
    for sub in REQUIRED_SUBPACKAGES {
        let prefix = format!("{PACKAGE}/{sub}/");
        if !names.iter().any(|n| n.starts_with(&prefix)) {
            failures.push(format!("no {PACKAGE}/{sub}/ package data"));
        }
    }
    failures
}

/// Normalised `Requires-Dist` names with their extra-gating status.
fn metadata_requirements(members: &[Member]) -> Result<Vec<(String, bool)>, String> {
    let metadata: Vec<&Member> = members
        .iter()
        .filter(|m| m.name.ends_with(".dist-info/METADATA"))
        .collect();
    let [metadata] = metadata.as_slice() else {
        return Ok(Vec::new());
    };
    let text = String::from_utf8(metadata.data.clone()).map_err(|e| e.to_string())?;
    let split = Regex::new(r"[\s<>=!~\[(;]").expect("static regex");
    let separators = Regex::new(r"[-_.]+").expect("static regex");
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(requirement) = line.strip_prefix("Requires-Dist:") else {
            continue;
        };
        let requirement = requirement.trim();
        let marker = requirement.split_once(';').map_or("", |(_, m)| m);
        let raw = split.splitn(requirement, 2).next().unwrap_or("");
        out.push((
            separators.replace_all(raw, "-").to_lowercase(),
            marker.contains("extra"),
        ));
    }
    Ok(out)
}

fn check_base_dependencies(requirements: &[(String, bool)]) -> Vec<String> {
    REQUIRED_BASE_DEPENDENCIES
        .iter()
        .filter(|required| !requirements.iter().any(|(name, gated)| name == *required && !gated))
        .map(|r| {
            format!(
                "'{r}' is not an unconditional Requires-Dist - the bundled kernel needs it on a bare \
                 `pip install medulla`, so it must not sit behind an extra"
            )
        })
        .collect()
}

fn check_forbidden_dependencies(requirements: &[(String, bool)]) -> Vec<String> {
    FORBIDDEN_DEPENDENCIES
        .iter()
        .filter(|forbidden| requirements.iter().any(|(name, _)| name == *forbidden))
        .map(|f| {
            format!(
                "'{f}' is a forbidden wheel dependency - native numeric runtime must not resolve the \
                 retired interpreter package"
            )
        })
        .collect()
}

/// Parses CSV text (quoted fields, doubled quotes, newlines inside quotes) into rows.
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let (mut row, mut field, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.into_iter()
        .filter(|r| !(r.len() == 1 && r[0].is_empty()))
        .collect()
}

fn urlsafe_sha256(data: &[u8]) -> String {
    format!("sha256={}", URL_SAFE_NO_PAD.encode(Sha256::digest(data)))
}

/// RECORD inconsistencies: a missing row, a wrong hash or a wrong size.
fn record_failures(members: &[Member]) -> Result<Vec<String>, String> {
    let records: Vec<&Member> = members
        .iter()
        .filter(|m| m.name.ends_with(".dist-info/RECORD"))
        .collect();
    let [record] = records.as_slice() else {
        return Ok(vec![format!(
            "expected exactly one dist-info/RECORD, found {}",
            records.len()
        )]);
    };
    let text = String::from_utf8(record.data.clone()).map_err(|e| e.to_string())?;
    let rows: BTreeMap<String, (String, String)> = csv_rows(&text)
        .into_iter()
        .filter(|r| r.len() >= 3)
        .map(|r| (r[0].clone(), (r[1].clone(), r[2].clone())))
        .collect();
    let mut failures = Vec::new();
    for member in members {
        if member.name.ends_with('/') || member.name == record.name {
            continue;
        }
        let Some((hash, size)) = rows.get(&member.name) else {
            failures.push(format!("RECORD is missing an entry for {}", member.name));
            continue;
        };
        if *hash != urlsafe_sha256(&member.data) {
            failures.push(format!("RECORD hash mismatch for {}", member.name));
        }
        if *size != member.data.len().to_string() {
            failures.push(format!("RECORD size mismatch for {}", member.name));
        }
    }
    Ok(failures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;
    use std::io::Write;
    use std::path::PathBuf;
    use zip::write::SimpleFileOptions;

    const VERSION: &str = "9.9.9";
    const SERVER: &str = "medulla/_bin/medulla-server";

    fn dist_info() -> String {
        format!("medulla_engine-{VERSION}.dist-info")
    }

    fn metadata(requires: &[&str]) -> String {
        let mut text = format!("Metadata-Version: 2.3\nName: medulla\nVersion: {VERSION}\n");
        for r in requires {
            text.push_str(&format!("Requires-Dist: {r}\n"));
        }
        text.push_str("Requires-Python: >=3.10\n");
        text
    }

    fn complete_metadata() -> String {
        metadata(&["msgpack>=1.2.1", "pyoxigraph>=0.3.22 ; extra == 'owl'"])
    }

    fn complete_members(metadata: &str) -> Vec<(String, Vec<u8>)> {
        vec![
            ("medulla/__init__.py".into(), vec![]),
            ("medulla/client.py".into(), vec![]),
            ("medulla/numeric.abi3.so".into(), b"\x7fELF-kernel".to_vec()),
            ("medulla/kvcache/__init__.py".into(), vec![]),
            (
                "medulla/skills/kg-modality-sql/SKILL.md".into(),
                b"# skill".to_vec(),
            ),
            (
                format!("{}/METADATA", dist_info()),
                metadata.as_bytes().to_vec(),
            ),
            (
                format!("{}/WHEEL", dist_info()),
                b"Wheel-Version: 1.0\n".to_vec(),
            ),
        ]
    }

    /// Builds a wheel; `corrupt` records different bytes, `misreport` offsets one member's size.
    fn build(
        dir: &Path,
        name: &str,
        members: &[(String, Vec<u8>)],
        executable: &[&str],
        corrupt: bool,
        misreport: Option<&str>,
    ) -> PathBuf {
        let path = dir.join(name);
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        for (member, payload) in members {
            let mode = if executable.contains(&member.as_str()) {
                0o755
            } else {
                0o644
            };
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(mode);
            writer.start_file(member.as_str(), options).unwrap();
            writer.write_all(payload).unwrap();
        }
        let mut rows: Vec<&(String, Vec<u8>)> = members.iter().collect();
        rows.sort();
        let mut record = String::new();
        for (member, payload) in rows {
            let mut recorded = payload.clone();
            if corrupt {
                recorded.push(b'x');
            }
            let off = usize::from(misreport == Some(member.as_str()));
            record.push_str(&format!(
                "{member},{},{}\n",
                urlsafe_sha256(&recorded),
                recorded.len() + off
            ));
        }
        record.push_str(&format!("{}/RECORD,,\n", dist_info()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer
            .start_file(format!("{}/RECORD", dist_info()), options)
            .unwrap();
        writer.write_all(record.as_bytes()).unwrap();
        writer.finish().unwrap();
        path
    }

    fn fixture() -> (Tree, PathBuf) {
        let t = Tree::new(&[]);
        let dir = t.path().to_path_buf();
        (t, dir)
    }

    fn failures(path: &Path, options: Options) -> Vec<String> {
        check_wheel(path, options).unwrap()
    }

    fn has(list: &[String], needle: &str) -> bool {
        list.iter().any(|f| f.contains(needle))
    }

    #[test]
    fn a_correctly_folded_wheel_passes() {
        let (_t, dir) = fixture();
        let wheel = build(
            &dir,
            "ok.whl",
            &complete_members(&complete_metadata()),
            &[],
            false,
            None,
        );
        assert!(failures(&wheel, Options::default()).is_empty());
        run(Path::new("."), &[wheel.to_string_lossy().into_owned()]).unwrap();
    }

    #[test]
    fn a_wheel_without_the_numeric_kernel_is_rejected() {
        let (_t, dir) = fixture();
        let mut members = complete_members(&complete_metadata());
        members.retain(|(n, _)| n != "medulla/numeric.abi3.so");
        let wheel = build(&dir, "kernel-less.whl", &members, &[], false, None);
        assert!(has(
            &failures(&wheel, Options::default()),
            "no numeric kernel"
        ));
        assert!(run(Path::new("."), &[wheel.to_string_lossy().into_owned()]).is_err());
    }

    #[test]
    fn the_engine_kernel_is_optional_until_required() {
        let (_t, dir) = fixture();
        let plain = build(
            &dir,
            "plain.whl",
            &complete_members(&complete_metadata()),
            &[],
            false,
            None,
        );
        assert!(failures(&plain, Options::default()).is_empty());
        let required = Options {
            require_engine_kernel: true,
            ..Options::default()
        };
        assert!(has(&failures(&plain, required), "no engine kernel"));
        let mut members = complete_members(&complete_metadata());
        members.push((
            "medulla/engine.abi3.so".into(),
            b"\x7fELF-engine-kernel".to_vec(),
        ));
        let with = build(&dir, "with-engine.whl", &members, &[], false, None);
        assert!(failures(&with, required).is_empty());
    }

    #[test]
    fn a_windows_wheel_is_accepted() {
        let (_t, dir) = fixture();
        let mut members = complete_members(&complete_metadata());
        members[2].0 = "medulla/numeric.pyd".into();
        let wheel = build(&dir, "windows.whl", &members, &[], false, None);
        assert!(failures(&wheel, Options::default()).is_empty());
    }

    #[test]
    fn a_wheel_carrying_a_console_script_is_rejected() {
        let (_t, dir) = fixture();
        let script = format!("medulla_engine-{VERSION}.data/scripts/medulla-server");
        let mut members = complete_members(&complete_metadata());
        members.push((script.clone(), b"\x7fELF-server".to_vec()));
        let wheel = build(&dir, "with-script.whl", &members, &[&script], false, None);
        assert!(has(
            &failures(&wheel, Options::default()),
            "is a console script"
        ));
    }

    #[test]
    fn the_packaged_server_is_required_only_when_asked() {
        let (_t, dir) = fixture();
        let required = Options {
            require_server_binary: true,
            ..Options::default()
        };
        let without = build(
            &dir,
            "without.whl",
            &complete_members(&complete_metadata()),
            &[],
            false,
            None,
        );
        assert!(failures(&without, Options::default()).is_empty());
        assert!(has(
            &failures(&without, required),
            "expected exactly one server executable"
        ));
        let mut members = complete_members(&complete_metadata());
        members.push((SERVER.into(), b"\x7fELF-server".to_vec()));
        let packaged = build(&dir, "packaged.whl", &members, &[SERVER], false, None);
        assert!(failures(&packaged, required).is_empty());
    }

    #[test]
    fn a_non_executable_or_empty_packaged_server_is_rejected() {
        let (_t, dir) = fixture();
        let required = Options {
            require_server_binary: true,
            ..Options::default()
        };
        let mut members = complete_members(&complete_metadata());
        members.push((SERVER.into(), b"\x7fELF-server".to_vec()));
        let plain = build(&dir, "plain.whl", &members, &[], false, None);
        assert!(has(&failures(&plain, required), "no executable bit"));
        members.last_mut().unwrap().1 = Vec::new();
        let empty = build(&dir, "empty.whl", &members, &[SERVER], false, None);
        assert!(has(&failures(&empty, required), "is empty"));
        let mut windows = complete_members(&complete_metadata());
        windows.push((format!("{SERVER}.exe"), b"MZ-server".to_vec()));
        let exe = build(&dir, "windows.whl", &windows, &[], false, None);
        assert!(failures(&exe, required).is_empty());
    }

    #[test]
    fn numpy_is_rejected_in_base_and_extra_requirements() {
        let (_t, dir) = fixture();
        for (name, requires) in [
            ("base.whl", vec!["msgpack>=1.0.0", "numpy>=1.22.0"]),
            (
                "extra.whl",
                vec!["msgpack>=1.0.0", "numpy>=1.22.0 ; extra == 'numeric'"],
            ),
        ] {
            let wheel = build(
                &dir,
                name,
                &complete_members(&metadata(&requires)),
                &[],
                false,
                None,
            );
            assert!(
                has(
                    &failures(&wheel, Options::default()),
                    "forbidden wheel dependency"
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn missing_package_data_is_rejected() {
        let (_t, dir) = fixture();
        let mut members = complete_members(&complete_metadata());
        members.retain(|(n, _)| n != "medulla/kvcache/__init__.py");
        let wheel = build(&dir, "no-kvcache.whl", &members, &[], false, None);
        assert!(has(&failures(&wheel, Options::default()), "kvcache"));
    }

    #[test]
    fn an_inconsistent_record_is_rejected() {
        let (_t, dir) = fixture();
        let wheel = build(
            &dir,
            "bad.whl",
            &complete_members(&complete_metadata()),
            &[],
            true,
            None,
        );
        assert!(has(
            &failures(&wheel, Options::default()),
            "RECORD hash mismatch"
        ));
    }

    #[test]
    fn each_release_invariant_violation_is_the_only_failure() {
        let (_t, dir) = fixture();
        let behind_extra = metadata(&["msgpack>=1.2.1 ; extra == 'wire'"]);
        let without = metadata(&["pyoxigraph>=0.3.22 ; extra == 'owl'"]);
        let cases: [(&str, String, Option<&str>, &str); 3] = [
            (
                "extra.whl",
                behind_extra,
                None,
                "'msgpack' is not an unconditional Requires-Dist",
            ),
            (
                "missing.whl",
                without,
                None,
                "'msgpack' is not an unconditional Requires-Dist",
            ),
            (
                "size.whl",
                complete_metadata(),
                Some("medulla/client.py"),
                "RECORD size mismatch for medulla/client.py",
            ),
        ];
        for (name, meta, misreport, expected) in cases {
            let wheel = build(&dir, name, &complete_members(&meta), &[], false, misreport);
            let found = failures(&wheel, Options::default());
            assert_eq!(found.len(), 1, "{name}: {found:?}");
            assert!(found[0].starts_with(expected), "{name}: {found:?}");
        }
    }

    #[test]
    fn the_command_fails_when_any_wheel_is_incomplete() {
        let (_t, dir) = fixture();
        let good = build(
            &dir,
            "good.whl",
            &complete_members(&complete_metadata()),
            &[],
            false,
            None,
        );
        let mut members = complete_members(&complete_metadata());
        members.retain(|(n, _)| n != "medulla/numeric.abi3.so");
        let bad = build(&dir, "bad.whl", &members, &[], false, None);
        let message = run(
            Path::new("."),
            &[
                good.to_string_lossy().into_owned(),
                bad.to_string_lossy().into_owned(),
            ],
        )
        .expect_err("one wheel is incomplete");
        assert!(message.starts_with("1 of 2 wheel artifact(s)"), "{message}");
    }

    #[test]
    fn csv_rows_handle_quotes_and_blank_lines() {
        let rows = csv_rows("a,\"b,c\",d\n\n\"x\"\"y\",,\n");
        assert_eq!(rows, vec![vec!["a", "b,c", "d"], vec!["x\"y", "", ""]]);
    }
}
