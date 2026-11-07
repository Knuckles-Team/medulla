//! MEDULLA-CONTRACT-R053.9: fold the native `medulla-server` executable into the medulla wheel.
//!
//! `cargo xtask inject-server-binary <medulla_wheel> <server_binary>`. The wheel ships the server
//! as package data at `medulla/_bin/medulla-server` (`medulla-server.exe` on Windows), built for
//! the same platform as the wheel's extension modules. The executable is placed in the
//! already-built wheel, mode 0755, and `RECORD` is rewritten. The wheel is rewritten in place and
//! an existing server member is replaced.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

pub const NAME: &str = "inject-server-binary";
pub const SUMMARY: &str = "fold the release server executable into the medulla wheel";

const TARGET_DIR: &str = "medulla/_bin";
const SERVER_NAMES: [&str; 2] = ["medulla-server", "medulla-server.exe"];
const SERVER_MODE: u32 = 0o755;

pub fn run(_root: &Path, args: &[String]) -> Result<(), String> {
    let [wheel, binary] = args else {
        return Err(
            "usage: cargo xtask inject-server-binary <medulla_wheel> <server_binary>".into(),
        );
    };
    let target = inject(Path::new(wheel), Path::new(binary))?;
    println!("OK: injected {target} (mode {SERVER_MODE:04o}) into the release wheel");
    Ok(())
}

fn is_server_member(name: &str) -> bool {
    name.rsplit_once('/')
        .is_some_and(|(dir, base)| dir == TARGET_DIR && SERVER_NAMES.contains(&base))
}

struct Entry {
    name: String,
    data: Vec<u8>,
    method: CompressionMethod,
    time: Option<DateTime>,
    mode: Option<u32>,
}

impl Entry {
    fn options(&self) -> SimpleFileOptions {
        let mut options = SimpleFileOptions::default().compression_method(self.method);
        if let Some(time) = self.time {
            options = options.last_modified_time(time);
        }
        if let Some(mode) = self.mode {
            options = options.unix_permissions(mode);
        }
        options
    }
}

/// A CSV field, quoted only when it holds a comma, a quote or a line break.
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// An exact RECORD for the rewritten wheel.
fn record_bytes(entries: &[Entry], record_name: &str) -> Vec<u8> {
    let mut sorted: Vec<&Entry> = entries.iter().filter(|e| !e.name.ends_with('/')).collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::new();
    for entry in sorted {
        let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(&entry.data));
        out.push_str(&format!(
            "{},sha256={digest},{}\n",
            csv_field(&entry.name),
            entry.data.len()
        ));
    }
    out.push_str(&format!("{},,\n", csv_field(record_name)));
    out.into_bytes()
}

/// Folds `binary` into `wheel`; returns the member name written.
pub fn inject(wheel: &Path, binary: &Path) -> Result<String, String> {
    let binary_name = binary
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    if !SERVER_NAMES.contains(&binary_name.as_str()) || !binary.is_file() {
        return Err(format!("not a medulla-server executable: {binary_name}"));
    }
    let data = fs::read(binary).map_err(|e| e.to_string())?;
    if data.is_empty() {
        return Err("server executable is empty".into());
    }
    let target = format!("{TARGET_DIR}/{binary_name}");
    let mut archive =
        ZipArchive::new(File::open(wheel).map_err(|e| format!("{}: {e}", wheel.display()))?)
            .map_err(|e| format!("{}: {e}", wheel.display()))?;
    let comment = archive.comment().to_vec();
    let mut entries: Vec<Entry> = Vec::new();
    let mut record: Option<Entry> = None;
    let mut records = 0;
    let mut stamp: Option<Option<DateTime>> = None;
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        if !seen.insert(name.clone()) {
            return Err("wheel contains duplicate members".into());
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let entry = Entry {
            name: name.clone(),
            data: bytes,
            method: file.compression(),
            time: file.last_modified(),
            mode: file.unix_mode().map(|m| m & 0o777),
        };
        if stamp.is_none() && name.starts_with("medulla/") {
            stamp = Some(entry.time);
        }
        if name.ends_with(".dist-info/RECORD") {
            records += 1;
            record = Some(entry);
        } else if !is_server_member(&name) {
            entries.push(entry);
        }
    }
    let (true, Some(record)) = (records == 1, record) else {
        return Err("wheel must contain exactly one RECORD".into());
    };
    entries.push(Entry {
        name: target.clone(),
        data,
        method: CompressionMethod::Deflated,
        time: stamp.unwrap_or(record.time),
        mode: Some(SERVER_MODE),
    });
    let record_data = record_bytes(&entries, &record.name);
    let temporary = PathBuf::from(format!("{}.tmp", wheel.display()));
    {
        let mut out = ZipWriter::new(File::create(&temporary).map_err(|e| e.to_string())?);
        out.set_raw_comment(comment.into_boxed_slice());
        for entry in &entries {
            out.start_file(&entry.name, entry.options())
                .map_err(|e| e.to_string())?;
            out.write_all(&entry.data).map_err(|e| e.to_string())?;
        }
        out.start_file(&record.name, record.options())
            .map_err(|e| e.to_string())?;
        out.write_all(&record_data).map_err(|e| e.to_string())?;
        out.finish().map_err(|e| e.to_string())?;
    }
    fs::rename(&temporary, wheel).map_err(|e| e.to_string())?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    fn wheel(path: &Path, members: &[(&str, &[u8])]) {
        let mut writer = ZipWriter::new(File::create(path).expect("create"));
        for (name, data) in members {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .expect("start");
            writer.write_all(data).expect("write");
        }
        writer
            .start_file("medulla-1.0.dist-info/RECORD", SimpleFileOptions::default())
            .expect("start");
        writer.finish().expect("finish");
    }

    #[test]
    fn inject_places_an_executable_server_member_and_rewrites_record() {
        for exe in SERVER_NAMES {
            let tree = Tree::new(&[]);
            let path = tree.path().join("medulla-1.0-cp39-abi3-linux_x86_64.whl");
            wheel(
                &path,
                &[
                    ("medulla/__init__.py", b""),
                    ("medulla/_bin/medulla-server", b"stale"),
                ],
            );
            let binary = tree.path().join(exe);
            fs::write(&binary, b"\x7fELF server").expect("write");
            for _ in 0..2 {
                // idempotent: a second fold replaces the member
                inject(&path, &binary).expect("injects");
            }
            let mut archive = ZipArchive::new(File::open(&path).expect("open")).expect("zip");
            let bin: Vec<String> = archive
                .file_names()
                .filter(|n| n.contains("/_bin/"))
                .map(String::from)
                .collect();
            assert_eq!(bin, [format!("medulla/_bin/{exe}")]);
            let member = archive
                .by_name(&format!("medulla/_bin/{exe}"))
                .expect("member");
            let mode = member.unix_mode().expect("mode");
            assert_eq!(mode & 0o777, 0o755);
            assert_eq!(mode & 0o170000, 0o100000, "a regular file");
            drop(member);
            let mut text = String::new();
            archive
                .by_name("medulla-1.0.dist-info/RECORD")
                .expect("record")
                .read_to_string(&mut text)
                .expect("read");
            assert!(text.contains(&format!("medulla/_bin/{exe},sha256=")));
            assert!(text.trim_end().ends_with("medulla-1.0.dist-info/RECORD,,"));
        }
    }

    #[test]
    fn inject_refuses_anything_but_the_server_executable() {
        let tree = Tree::new(&[("nemesis", "x")]);
        let path = tree.path().join("medulla-1.0-py3-none-any.whl");
        wheel(&path, &[("medulla/x.py", b"")]);
        let error = inject(&path, &tree.path().join("nemesis")).expect_err("refused");
        assert!(error.contains("not a medulla-server executable"));
    }

    #[test]
    fn a_wheel_without_exactly_one_record_is_refused() {
        let tree = Tree::new(&[("medulla-server", "x")]);
        let path = tree.path().join("medulla-1.0-py3-none-any.whl");
        let mut writer = ZipWriter::new(File::create(&path).expect("create"));
        writer
            .start_file("medulla/x.py", SimpleFileOptions::default())
            .expect("start");
        writer.finish().expect("finish");
        let error = inject(&path, &tree.path().join("medulla-server")).expect_err("refused");
        assert!(error.contains("exactly one RECORD"));
    }

    #[test]
    fn record_rows_quote_only_when_needed() {
        assert_eq!(csv_field("a/b.py"), "a/b.py");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
    }
}
