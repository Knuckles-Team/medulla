//! MEDULLA-CONTRACT-R053: fail closed when a wheel retains build roots or first-party identity.
//!
//! `cargo xtask wheel-privacy [--deny-prefix PREFIX]... WHEEL...`. Every regular wheel member is
//! streamed through the same byte-level audit, so native binaries, SBOM documents, package
//! metadata and ordinary Python files receive equivalent coverage. The detector looks for
//! concrete home/workspace paths and exact runtime build prefixes; it does not classify an email
//! address as sensitive because it appears in third-party attribution. Only first-party
//! `Author`/`Maintainer` headers in the wheel's core `METADATA` must use identity-neutral project
//! values. Findings are reported by category and a hash of the member name, never by echoing the
//! sensitive content.
//!
//! The options `--isolated-env` and `--env NAME=VALUE` replace the process environment and the
//! working directory as the source of runtime build prefixes; the release tests use them to audit
//! a fixture wheel against a chosen environment.

use regex::bytes::{Regex, RegexBuilder};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const NAME: &str = "wheel-privacy";
pub const SUMMARY: &str = "a wheel retains no build root or first-party identity";

const CHUNK_SIZE: usize = 1024 * 1024;
const OVERLAP_SIZE: usize = 8192;
const MAX_METADATA_SIZE: usize = 4 * 1024 * 1024;
const SEGMENT: &str = r"[A-Za-z0-9_.@+ -]{1,128}";
const NEUTRAL_NAMES: [&str; 2] = ["project contributors", "repository maintainers"];
const NEUTRAL_ROLE_ADDRESSES: [&str; 1] = ["maintainers@knuckles.team"];
const IDENTITY_HEADERS: [&str; 4] = ["author", "maintainer", "author-email", "maintainer-email"];
const RUNTIME_ENV: [&str; 11] = [
    "GITHUB_WORKSPACE",
    "RUNNER_WORKSPACE",
    "CARGO_TARGET_DIR",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUNNER_TOOL_CACHE",
    "RUNNER_TEMP",
    "USERPROFILE",
    "LOCALAPPDATA",
    "APPDATA",
    "HOME",
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub artifact: usize,
    pub member_id: String,
    pub category: String,
}

pub struct AuditResult {
    pub findings: Vec<Finding>,
    pub member_count: usize,
}

fn build(pattern: &str, ignore_case: bool) -> Regex {
    RegexBuilder::new(pattern)
        .unicode(false)
        .case_insensitive(ignore_case)
        .size_limit(512 << 20)
        .dfa_size_limit(512 << 20)
        .build()
        .unwrap_or_else(|e| panic!("pattern {pattern}: {e}"))
}

/// The path-shaped categories. Each pattern needs no alphanumeric directly before its start (a
/// lookbehind the regex engine lacks), applied by `has_match`.
fn path_patterns() -> Vec<(&'static str, Regex)> {
    let mk = |category, pattern: String| (category, build(&pattern, true));
    vec![
        mk(
            "wsl-home-prefix",
            format!(r"/mnt/[a-z]/(?:users|documents[ ]and[ ]settings)/{SEGMENT}[/\\]"),
        ),
        mk(
            "windows-home-prefix",
            format!(
                r"(?:[a-z]:[\\/]|\\\\[^\\/\x00]{{1,128}}[\\/][^\\/\x00]{{1,128}}[\\/])(?:users|documents[ ]and[ ]settings)[\\/]{SEGMENT}[/\\]"
            ),
        ),
        mk(
            "posix-home-prefix",
            format!(r"/(?:home|users)/{SEGMENT}[/\\]"),
        ),
        mk(
            "privileged-home-prefix",
            r"/(?:root|github/home)[/\\]".to_string(),
        ),
        mk(
            "concrete-workspace-prefix",
            format!(
                r"(?:[a-z]:[\\/]|/)(?:[A-Za-z0-9_.@+ -]{{1,128}}[\\/]){{0,8}}(?:workspace|workspaces)[\\/]{SEGMENT}(?:[/\\\x00\s]|$)"
            ),
        ),
    ]
}

/// A match whose start has no ASCII alphanumeric byte before it.
fn has_match(pattern: &Regex, data: &[u8]) -> bool {
    let mut at = 0;
    while at <= data.len() {
        let Some(m) = pattern.find_at(data, at) else {
            return false;
        };
        if m.start() == 0 || !data[m.start() - 1].is_ascii_alphanumeric() {
            return true;
        }
        at = m.start() + 1;
    }
    false
}

fn safe_prefix(value: Option<&str>) -> Option<String> {
    let value = value?;
    if value.is_empty() || value.contains(['\0', '\n', '\r']) {
        return None;
    }
    let candidate = value.trim_end_matches(['/', '\\']);
    let two_char_drive = candidate.chars().count() == 2 && candidate.ends_with(':');
    (!candidate.is_empty() && !two_char_drive).then(|| candidate.to_string())
}

/// Concrete build prefixes derived in memory, longest first.
pub fn runtime_deny_prefixes(
    env: &BTreeMap<String, String>,
    checkout: Option<&str>,
) -> Vec<String> {
    let mut values: Vec<Option<&str>> = vec![checkout];
    values.extend(RUNTIME_ENV.iter().map(|n| env.get(*n).map(String::as_str)));
    let mut unique: BTreeMap<String, String> = BTreeMap::new();
    for value in values {
        if let Some(prefix) = safe_prefix(value) {
            let key = if prefix.contains(['\\', ':']) {
                prefix.to_lowercase()
            } else {
                prefix.clone()
            };
            unique.entry(key).or_insert(prefix);
        }
    }
    let mut out: Vec<String> = unique.into_values().collect();
    out.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b)));
    out
}

fn escape_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!(r"\x{b:02X}")).collect()
}

fn exact_prefix_patterns(prefixes: &[String]) -> Vec<Regex> {
    let boundary = r#"(?:$|[\\/\x00\r\n\t "'`,;:=)\]}])"#;
    let wide_boundary = r#"(?:$|[\\/\x00\r\n\t "'`,;:=)\]}]\x00)"#;
    let mut patterns = Vec::new();
    for prefix in prefixes {
        let variants: BTreeSet<String> = [
            prefix.clone(),
            prefix.replace('\\', "/"),
            prefix.replace('/', "\\"),
        ]
        .into_iter()
        .collect();
        for variant in variants {
            patterns.push(build(
                &format!("{}{boundary}", escape_bytes(variant.as_bytes())),
                true,
            ));
            let wide: Vec<u8> = variant.encode_utf16().flat_map(u16::to_le_bytes).collect();
            patterns.push(build(
                &format!("{}{wide_boundary}", escape_bytes(&wide)),
                true,
            ));
        }
    }
    patterns
}

/// Printable UTF-16LE strings as ASCII, at both byte alignments.
fn wide_ascii_strings(data: &[u8], wide: &Regex) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for alignment in [0, 1] {
        let view = data.get(alignment..).unwrap_or(&[]);
        out.extend(
            wide.find_iter(view)
                .map(|m| m.as_bytes().iter().step_by(2).copied().collect()),
        );
    }
    out
}

struct Audit {
    paths: Vec<(&'static str, Regex)>,
    exact: Vec<Regex>,
    wide: Regex,
    pth: Regex,
}

impl Audit {
    fn new(prefixes: &[String]) -> Self {
        Self {
            paths: path_patterns(),
            exact: exact_prefix_patterns(prefixes),
            wide: build(r"(?:[\x20-\x7e]\x00){6,}", false),
            pth: build(r"(?m)^[ \t]*(?:[A-Za-z]:[\\/]|/)", false),
        }
    }

    fn categories(&self, data: &[u8]) -> BTreeSet<&'static str> {
        let mut found: BTreeSet<&'static str> = self
            .paths
            .iter()
            .filter(|(_, p)| has_match(p, data))
            .map(|(c, _)| *c)
            .collect();
        if self.exact.iter().any(|p| p.is_match(data)) {
            found.insert("runtime-build-prefix");
        }
        if data.contains(&0) {
            for text in wide_ascii_strings(data, &self.wide) {
                found.extend(
                    self.paths
                        .iter()
                        .filter(|(_, p)| has_match(p, &text))
                        .map(|(c, _)| *c),
                );
            }
        }
        found
    }
}

fn member_id(name: &str) -> String {
    let digest = Sha256::digest(name.as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

// ---- metadata identity -----------------------------------------------------------------------

/// Header name (lower-cased) and unfolded value, up to the first blank line.
fn parse_headers(data: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(data);
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = headers.last_mut() {
                value.push_str(line);
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_lowercase(), value.to_string()));
        }
    }
    headers
}

/// `(display name, address)` pairs of a header value, comma separated outside quotes and angles.
fn get_addresses(value: &str) -> Vec<(String, String)> {
    let (mut parts, mut current) = (Vec::new(), String::new());
    let (mut quoted, mut angle) = (false, false);
    for c in value.chars() {
        match c {
            '"' => quoted = !quoted,
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            ',' if !quoted && !angle => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    parts.push(current);
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|part| {
            if let (Some(open), Some(close)) = (part.find('<'), part.rfind('>')) {
                let name = part[..open].trim().trim_matches('"').trim().to_string();
                (name, part[open + 1..close].trim().to_string())
            } else if let (Some(open), Some(close)) = (part.find('('), part.rfind(')')) {
                (
                    part[open + 1..close].trim().to_string(),
                    part[..open].trim().to_string(),
                )
            } else {
                (String::new(), part.to_string())
            }
        })
        .collect()
}

fn address_is_neutral(display_name: &str, address: &str) -> bool {
    if !display_name.is_empty() && !NEUTRAL_NAMES.contains(&display_name.to_lowercase().as_str()) {
        return false;
    }
    let folded = address.to_lowercase();
    // A role address names no person; PyPI rejects the reserved .invalid domain.
    folded.ends_with("@example.invalid") || NEUTRAL_ROLE_ADDRESSES.contains(&folded.as_str())
}

fn header_value_is_neutral(header: &str, value: &str) -> bool {
    if header == "author" || header == "maintainer" {
        return NEUTRAL_NAMES.contains(&value.to_lowercase().as_str());
    }
    let addresses = get_addresses(value);
    !addresses.is_empty() && addresses.iter().all(|(n, a)| address_is_neutral(n, a))
}

fn neutral_metadata(data: &[u8]) -> bool {
    if data.len() > MAX_METADATA_SIZE {
        return false;
    }
    parse_headers(data)
        .iter()
        .filter(|(name, _)| IDENTITY_HEADERS.contains(&name.as_str()))
        .all(|(name, raw)| {
            let value = raw.trim();
            value.is_empty() || header_value_is_neutral(name, value)
        })
}

// ---- archive audit ---------------------------------------------------------------------------

fn unsafe_member_name(name: &str) -> bool {
    name.starts_with('/') || name.split('/').any(|p| p == "..") || name.contains('\\')
}

fn member_name_findings(
    audit: &Audit,
    id: &str,
    name: &str,
    artifact: usize,
    out: &mut BTreeSet<Finding>,
) {
    let add = |out: &mut BTreeSet<Finding>, category: &str| {
        out.insert(Finding {
            artifact,
            member_id: id.to_string(),
            category: category.to_string(),
        });
    };
    if unsafe_member_name(name) {
        add(out, "unsafe-member-name");
    }
    let parts: Vec<&str> = name
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let suffix = parts
        .last()
        .and_then(|f| f.rfind('.').filter(|i| *i > 0).map(|i| &f[i..]));
    if parts.contains(&"__pycache__") || matches!(suffix, Some(".pyc" | ".pyo")) {
        add(out, "python-bytecode-member");
    }
    for category in audit.categories(name.as_bytes()) {
        add(out, category);
    }
}

fn scan_member(
    audit: &Audit,
    name: &str,
    stream: &mut dyn Read,
) -> Result<(BTreeSet<&'static str>, Option<Vec<u8>>), std::io::Error> {
    let mut categories = BTreeSet::new();
    let mut metadata: Option<Vec<u8>> = name.ends_with(".dist-info/METADATA").then(Vec::new);
    let is_sbom = name.contains(".dist-info/sboms/");
    let is_pth = name.ends_with(".pth");
    let mut tail: Vec<u8> = Vec::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    loop {
        let mut filled = 0;
        while filled < CHUNK_SIZE {
            let n = stream.read(&mut buffer[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        let chunk = &buffer[..filled];
        let mut combined = std::mem::take(&mut tail);
        combined.extend_from_slice(chunk);
        categories.extend(audit.categories(&combined));
        if is_sbom && combined.windows(12).any(|w| w == b"path+file://") {
            categories.insert("filesystem-sbom-reference");
        }
        if is_pth && audit.pth.is_match(&combined) {
            categories.insert("absolute-pth-reference");
        }
        tail = combined[combined.len().saturating_sub(OVERLAP_SIZE)..].to_vec();
        if let Some(collected) = metadata.as_mut() {
            if collected.len() + chunk.len() > MAX_METADATA_SIZE {
                categories.insert("oversized-core-metadata");
                metadata = None;
            } else {
                collected.extend_from_slice(chunk);
            }
        }
        if filled < CHUNK_SIZE {
            break;
        }
    }
    Ok((categories, metadata))
}

/// Audits one wheel; a member that fails part-way keeps the findings gathered before it.
pub fn audit_wheel(
    path: &Path,
    artifact: usize,
    deny_prefixes: &[String],
    env: &BTreeMap<String, String>,
    checkout: Option<&str>,
) -> AuditResult {
    let mut prefixes = runtime_deny_prefixes(env, checkout);
    prefixes.extend(deny_prefixes.iter().filter_map(|p| safe_prefix(Some(p))));
    let audit = Audit::new(&prefixes);
    let mut findings: BTreeSet<Finding> = BTreeSet::new();
    let mut member_count = 0;
    let invalid = |findings: &mut BTreeSet<Finding>| {
        findings.insert(Finding {
            artifact,
            member_id: "archive-index".into(),
            category: "invalid-wheel".into(),
        });
    };
    let Ok(file) = File::open(path) else {
        invalid(&mut findings);
        return AuditResult {
            findings: findings.into_iter().collect(),
            member_count,
        };
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        invalid(&mut findings);
        return AuditResult {
            findings: findings.into_iter().collect(),
            member_count,
        };
    };
    let comment = archive.comment().to_vec();
    if !comment.is_empty() {
        for category in audit.categories(&comment) {
            findings.insert(Finding {
                artifact,
                member_id: "archive-comment".into(),
                category: category.into(),
            });
        }
    }
    let mut names: Vec<(String, usize)> = Vec::new();
    for index in 0..archive.len() {
        match archive.by_index_raw(index) {
            Ok(entry) => names.push((entry.name().to_string(), index)),
            Err(_) => {
                invalid(&mut findings);
            }
        }
    }
    names.sort();
    let unique: BTreeSet<&String> = names.iter().map(|(n, _)| n).collect();
    if unique.len() != names.len() {
        findings.insert(Finding {
            artifact,
            member_id: "archive-index".into(),
            category: "duplicate-member".into(),
        });
    }
    for (name, index) in &names {
        member_count += 1;
        let id = member_id(name);
        member_name_findings(&audit, &id, name, artifact, &mut findings);
        if name.ends_with('/') {
            continue;
        }
        let scanned = archive
            .by_index(*index)
            .map_err(std::io::Error::other)
            .and_then(|mut entry| scan_member(&audit, name, &mut entry));
        match scanned {
            Ok((categories, metadata)) => {
                for category in categories {
                    findings.insert(Finding {
                        artifact,
                        member_id: id.clone(),
                        category: category.into(),
                    });
                }
                if metadata.is_some_and(|m| !neutral_metadata(&m)) {
                    findings.insert(Finding {
                        artifact,
                        member_id: id.clone(),
                        category: "first-party-metadata-identity".into(),
                    });
                }
            }
            Err(_) => {
                invalid(&mut findings);
                break;
            }
        }
    }
    AuditResult {
        findings: findings.into_iter().collect(),
        member_count,
    }
}

pub fn run(_root: &Path, args: &[String]) -> Result<(), String> {
    let (mut wheels, mut deny, mut isolated) = (Vec::<PathBuf>::new(), Vec::new(), false);
    let mut pairs = BTreeMap::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--deny-prefix" => deny.push(iter.next().ok_or("--deny-prefix needs a value")?.clone()),
            "--isolated-env" => isolated = true,
            "--env" => {
                let pair = iter.next().ok_or("--env needs NAME=VALUE")?;
                let (k, v) = pair.split_once('=').ok_or("--env needs NAME=VALUE")?;
                pairs.insert(k.to_string(), v.to_string());
            }
            other => wheels.push(other.into()),
        }
    }
    if wheels.is_empty() {
        return Err("usage: cargo xtask wheel-privacy [--deny-prefix PREFIX]... WHEEL...".into());
    }
    let (env, checkout) = if isolated {
        (pairs, None)
    } else {
        let cwd = std::env::current_dir()
            .ok()
            .map(|c| c.display().to_string());
        (std::env::vars().collect(), cwd)
    };
    wheels.sort_by_key(|w| w.file_name().map(|n| n.to_os_string()));
    let (mut findings, mut member_count) = (Vec::new(), 0);
    for (index, path) in wheels.iter().enumerate() {
        let result = audit_wheel(path, index + 1, &deny, &env, checkout.as_deref());
        findings.extend(result.findings);
        member_count += result.member_count;
    }
    if findings.is_empty() {
        println!(
            "OK: audited {} wheel artifact(s), {member_count} member(s); no concrete build identity retained",
            wheels.len()
        );
        return Ok(());
    }
    findings.sort();
    let mut message = format!(
        "wheel privacy audit found {} issue(s) across {} artifact(s)\n",
        findings.len(),
        wheels.len()
    );
    for f in &findings {
        message.push_str(&format!(
            "artifact={} member_id={} category={}\n",
            f.artifact, f.member_id, f.category
        ));
    }
    // A "normalized 0 occurrence(s)" line from the wheel normalizers directly above a nonzero
    // failure here is not a contradiction: the normalizers rewrite only exact, known build roots,
    // while this audit also matches broader byte-level patterns no normalizer can rewrite.
    message.push_str(
        "note: 'normalized N occurrence(s)' above covers only EXACT roots known to configure_rust_path_remap.py's path_remaps(); the categories found here may be broader PATTERNS (path_patterns in wheel_privacy.rs) that no normalizer can fix after the fact -- the fix belongs at the build (stop the path from being embedded), not in a normalizer.",
    );
    Err(message)
}

#[cfg(test)]
mod tests {
    /// A rooted path built at run time, so no fixture path is a literal in the source.
    fn p(parts: &[&str]) -> String {
        format!("/{}", parts.join("/"))
    }

    use super::*;
    use crate::tempdir::Tree;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn neutral_metadata_bytes(body: &str) -> Vec<u8> {
        format!("Metadata-Version: 2.4\nName: fixture-package\nVersion: 1.0.0\nAuthor-email: Repository Maintainers <maintainers@knuckles.team>\n\n{body}").into_bytes()
    }

    fn wheel_with(
        members: &[(&str, &[u8])],
        comment: Option<&str>,
        stored: bool,
    ) -> (Tree, PathBuf) {
        let tree = Tree::new(&[]);
        let path = tree.path().join("fixture_package-1.0.0-py3-none-any.whl");
        let mut writer = zip::ZipWriter::new(File::create(&path).expect("create"));
        let options = SimpleFileOptions::default().compression_method(if stored {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        });
        let mut sorted = members.to_vec();
        sorted.sort();
        for (name, payload) in sorted {
            writer.start_file(name, options).expect("start");
            writer.write_all(payload).expect("write");
        }
        if let Some(c) = comment {
            writer.set_comment(c);
        }
        writer.finish().expect("finish");
        (tree, path)
    }

    fn wheel(members: &[(&str, &[u8])]) -> (Tree, PathBuf) {
        wheel_with(members, None, false)
    }

    fn categories(path: &Path, deny: &[&str]) -> BTreeSet<String> {
        let deny: Vec<String> = deny.iter().map(|d| d.to_string()).collect();
        audit_wheel(path, 1, &deny, &BTreeMap::new(), None)
            .findings
            .into_iter()
            .map(|f| f.category)
            .collect()
    }

    const META: &str = "fixture_package-1.0.0.dist-info/METADATA";

    #[test]
    fn sanitized_wheel_allows_third_party_attribution_emails() {
        let meta =
            neutral_metadata_bytes("License attribution: Contributor <third.party@example.org>\n");
        let (_t, path) = wheel(&[
            (META, &meta),
            (
                "fixture_package-1.0.0.dist-info/sbom.spdx.json",
                br#"{"supplier":"third.party@example.org","source":"/build/source"}"#,
            ),
            (
                "fixture_package/native.so",
                b"ELF\x00/build/cargo/registry\x00",
            ),
        ]);
        let result = audit_wheel(&path, 1, &[], &BTreeMap::new(), None);
        assert_eq!(result.member_count, 3);
        assert!(result.findings.is_empty(), "{:?}", result.findings);
    }

    #[test]
    fn members_are_scanned_regardless_of_content_type() {
        let meta = neutral_metadata_bytes("");
        let wide = |text: &str| {
            text.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<u8>>()
        };
        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            (
                "fixture_package/native.so",
                format!("ELF\x00{}\x00", p(&["home", "fixture-builder", "source"])).into_bytes(),
                "posix-home-prefix",
            ),
            (
                "fixture_package/sbom.cdx.json",
                p(&[
                    "mnt",
                    "z",
                    "Users",
                    "fixture-builder",
                    "Workspace",
                    "source",
                ])
                .into_bytes(),
                "wsl-home-prefix",
            ),
            (
                "fixture_package/native.pdb",
                wide("Z:\\Users\\fixture-builder\\Workspace\\source"),
                "windows-home-prefix",
            ),
        ];
        for (member, payload, expected) in cases {
            let (_t, path) = wheel(&[(META, &meta), (member, &payload)]);
            assert!(categories(&path, &[]).contains(expected), "{member}");
        }
    }

    #[test]
    fn a_path_glued_to_an_alphanumeric_is_not_a_home_prefix() {
        let meta = neutral_metadata_bytes("");
        let (_t, path) = wheel(&[
            (META, &meta),
            (
                "fixture_package/a.so",
                &format!("ELF\x00xx{}\x00", p(&["home", "builder", "src"])).into_bytes(),
            ),
        ]);
        assert!(categories(&path, &[]).is_empty());
    }

    #[test]
    fn runtime_prefix_detects_nonstandard_build_root_and_requires_a_path_boundary() {
        let meta = neutral_metadata_bytes("");
        let (_t, path) = wheel(&[
            (META, &meta),
            (
                "fixture_package/native.so",
                &format!(
                    "ELF\x00{}/src/lib.rs",
                    p(&["var", "lib", "fixture-builder", "source"])
                )
                .into_bytes(),
            ),
        ]);
        assert!(categories(
            &path,
            &[p(&["var", "lib", "fixture-builder", "source"]).as_str()]
        )
        .contains("runtime-build-prefix"));
        let (_t, path) = wheel(&[
            (META, &meta),
            (
                "fixture_package/native.so",
                b"ELF\x00/build/source/src/root_cause.rs\x00",
            ),
        ]);
        assert!(!categories(&path, &["/root"]).contains("runtime-build-prefix"));
    }

    #[test]
    fn python_bytecode_members_are_rejected() {
        let meta = neutral_metadata_bytes("");
        for member in [
            "fixture_package/__pycache__/module.cpython-312.pyc",
            "fixture_package/module.pyc",
            "fixture_package/module.pyo",
        ] {
            let (_t, path) = wheel(&[(META, &meta), (member, b"compiled-python-cache")]);
            assert!(
                categories(&path, &[]).contains("python-bytecode-member"),
                "{member}"
            );
        }
    }

    #[test]
    fn privileged_container_roots_are_flagged() {
        let meta = neutral_metadata_bytes("");
        let (_t, path) = wheel(&[
            (META, &meta),
            (
                "fixture_package/native.so",
                &format!("ELF\x00{}\x00", p(&["root", ".cargo", "registry", "src"])).into_bytes(),
            ),
        ]);
        assert!(categories(&path, &[]).contains("privileged-home-prefix"));
    }

    #[test]
    fn cargo_target_dir_is_denied_at_runtime() {
        let env = BTreeMap::from([(
            "CARGO_TARGET_DIR".to_string(),
            p(&["var", "tmp", "fixture-target"]),
        )]);
        assert!(runtime_deny_prefixes(&env, None).contains(&p(&["var", "tmp", "fixture-target"])));
        assert!(runtime_deny_prefixes(&BTreeMap::new(), None).is_empty());
    }

    #[test]
    fn sbom_filesystem_references_and_absolute_pth_files_are_rejected() {
        let meta = neutral_metadata_bytes("");
        let (_t, path) = wheel(&[
            (META, &meta),
            (
                "fixture_package-1.0.0.dist-info/sboms/x.cdx.json",
                b"path+file://build",
            ),
        ]);
        assert_eq!(
            categories(&path, &[]),
            BTreeSet::from(["filesystem-sbom-reference".to_string()])
        );
        let (_t, path) = wheel(&[(META, &meta), ("fixture_package.pth", b"/build/source\n")]);
        assert_eq!(
            categories(&path, &[]),
            BTreeSet::from(["absolute-pth-reference".to_string()])
        );
    }

    #[test]
    fn only_core_metadata_identity_fields_must_be_neutral() {
        let meta = b"Metadata-Version: 2.4\nName: fixture-package\nVersion: 1.0.0\nAuthor-email: Fixture Identity <fixture.identity@example.org>\n\nAttribution: Third Party <third.party@example.org>\n";
        let (_t, path) = wheel(&[(META, meta)]);
        assert_eq!(
            categories(&path, &[]),
            BTreeSet::from(["first-party-metadata-identity".to_string()])
        );
        let neutral = b"Metadata-Version: 2.4\nAuthor: Project Contributors\nMaintainer-email: Repository Maintainers <a@example.invalid>, <b@example.invalid>\n\n";
        let (_t, path) = wheel(&[(META, neutral)]);
        assert!(categories(&path, &[]).is_empty());
    }

    #[test]
    fn role_address_is_neutral() {
        assert!(address_is_neutral(
            "Repository Maintainers",
            "maintainers@knuckles.team"
        ));
        assert!(!address_is_neutral(
            "Repository Maintainers",
            "someone@knuckles.team"
        ));
    }

    #[test]
    fn findings_before_a_corrupt_member_survive_the_invalid_wheel_finding() {
        let corrupt = b"stored-payload-whose-crc-will-not-match";
        let (_t, path) = wheel_with(
            &[
                (
                    "a_leaking.so",
                    &format!(
                        "ELF\x00{}\x00",
                        p(&["home", "fixture-builder", "work", "x"])
                    )
                    .into_bytes(),
                ),
                ("b_corrupt.so", corrupt),
            ],
            Some(p(&["home", "fixture-commenter", "src"]).as_str()),
            true,
        );
        let mut raw = std::fs::read(&path).expect("read");
        let at = raw
            .windows(corrupt.len())
            .position(|w| w == corrupt)
            .expect("payload");
        raw[at] ^= 0xFF;
        std::fs::write(&path, raw).expect("write");
        let result = audit_wheel(&path, 1, &[], &BTreeMap::new(), None);
        assert_eq!(result.member_count, 2);
        let mut cats: Vec<&str> = result
            .findings
            .iter()
            .map(|f| f.category.as_str())
            .collect();
        cats.sort_unstable();
        assert_eq!(
            cats,
            ["invalid-wheel", "posix-home-prefix", "posix-home-prefix"]
        );
        let ids: BTreeSet<&str> = result
            .findings
            .iter()
            .map(|f| f.member_id.as_str())
            .collect();
        assert!(ids.contains("archive-comment") && ids.contains("archive-index"));
    }

    #[test]
    fn the_command_never_echoes_a_sensitive_prefix() {
        let meta = neutral_metadata_bytes("");
        let (_t, path) = wheel(&[
            (META, &meta),
            (
                "fixture_package/native.so",
                &format!(
                    "ELF\x00{}/src/lib.rs",
                    p(&["var", "lib", "fixture-builder", "source"])
                )
                .into_bytes(),
            ),
        ]);
        let args = vec![
            "--isolated-env".to_string(),
            "--deny-prefix".to_string(),
            p(&["var", "lib", "fixture-builder", "source"]),
            path.display().to_string(),
        ];
        let failure = run(Path::new("."), &args).expect_err("leak fails");
        assert!(!failure.contains("fixture-builder"));
        assert!(failure.contains("category=runtime-build-prefix"));
    }

    #[test]
    fn an_unreadable_wheel_is_invalid() {
        let tree = Tree::new(&[("not.whl", "not a zip")]);
        let result = audit_wheel(&tree.path().join("not.whl"), 1, &[], &BTreeMap::new(), None);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].category, "invalid-wheel");
    }
}
