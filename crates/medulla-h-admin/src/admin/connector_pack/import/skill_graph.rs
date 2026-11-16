//! Skill graph bundle validation (MEDULLA-TYPED-PACKS-R700.2).
//!
//! A skill graph travels in a pack as `skill_file` entries named
//! `<graph>/<relative path>`: its `SKILL.md`, `index.json`, `sources.json` and
//! hash-named `reference/` files. `validate_bundle` checks the bundle against
//! the manifest read from `sources.json` in one pass over the manifest (every
//! lookup is a hash probe, so the cost is O(entries)) and returns a
//! `ConformanceReport` naming each rule it checked and each violation, the
//! failing path first. Collection stops at `MAX_PACK_VIOLATIONS` (R040).
//!
//! Canonical text: a body is hashed as UTF-8 text with CRLF line endings
//! folded to LF; a body that is not UTF-8 is hashed as raw bytes.

use std::collections::{HashMap, HashSet};

use medulla_catalog_types::connector_pack::skill_graph::{ConformanceReport, SkillGraphManifest};
use medulla_catalog_types::connector_pack::{PackEntry, MAX_PACK_VIOLATIONS};
use sha2::{Digest, Sha256};

use super::validation::{section_bytes, valid_skill_frontmatter};

/// Rule id: every manifest path has an entry whose body lies in the archive.
pub(super) const RULE_ENTRIES_PRESENT: &str = "skill_graph.entries_present";
/// Rule id: each SHA-256 over canonical text matches the manifest.
pub(super) const RULE_HASHES_MATCH: &str = "skill_graph.hashes_match";
/// Rule id: every `SKILL.md` has valid front matter.
pub(super) const RULE_FRONT_MATTER: &str = "skill_graph.front_matter";
/// Rule id: `index.json` sections equal the manifest `reference/` paths.
pub(super) const RULE_INDEX_SECTIONS: &str = "skill_graph.index_sections";

const INDEX_PATH: &str = "index.json";
const SKILL_FILE_NAME: &str = "SKILL.md";
const REFERENCE_PREFIX: &str = "reference/";

struct Collector {
    violations: Vec<String>,
}

impl Collector {
    fn full(&self) -> bool {
        self.violations.len() >= MAX_PACK_VIOLATIONS
    }

    fn push(&mut self, code: &str, path: &str, detail: &str) {
        if !self.full() {
            self.violations.push(format!("{code}: {path}: {detail}"));
        }
    }
}

/// Validates the bundle of graph `graph` against `manifest`. `entries` are the
/// pack's entries; the graph's files are those named `<graph>/<path>`.
pub(super) fn validate_bundle(
    graph: &str,
    archive: &[u8],
    entries: &[&PackEntry],
    manifest: &SkillGraphManifest,
) -> ConformanceReport {
    let prefix = format!("{graph}/");
    let by_name: HashMap<&str, &PackEntry> = entries
        .iter()
        .filter_map(|entry| Some((entry.name.strip_prefix(prefix.as_str())?, *entry)))
        .collect();
    let mut found = Collector {
        violations: Vec::new(),
    };
    let mut reference_paths: HashSet<&str> = HashSet::new();
    let mut duplicate_reference = None;

    for item in &manifest.entries {
        if found.full() {
            break;
        }
        let path = item.path.as_str();
        if path.starts_with(REFERENCE_PREFIX) && !reference_paths.insert(path) {
            duplicate_reference.get_or_insert(path);
        }
        let Some(entry) = by_name.get(path) else {
            found.push(
                "SKILL_GRAPH_ENTRY_MISSING",
                path,
                "no pack entry for the path",
            );
            continue;
        };
        let Ok(body) = section_bytes(archive, &entry.body) else {
            found.push(
                "SKILL_GRAPH_ENTRY_MISSING",
                path,
                "body lies outside the archive",
            );
            continue;
        };
        if sha256_hex(&canonical(body)) != item.sha256.as_str() {
            found.push(
                "SKILL_GRAPH_HASH_MISMATCH",
                path,
                "canonical text does not match the manifest digest",
            );
        }
        if path.rsplit('/').next() == Some(SKILL_FILE_NAME) && !valid_skill_frontmatter(body, graph)
        {
            found.push(
                "SKILL_GRAPH_FRONT_MATTER",
                path,
                "invalid SKILL.md front matter",
            );
        }
    }

    if let Some(path) = duplicate_reference {
        found.push(
            "SKILL_GRAPH_INDEX_MISMATCH",
            path,
            "listed more than once in the manifest",
        );
    }
    check_index(archive, &by_name, &reference_paths, &mut found);

    ConformanceReport {
        checked_rules: vec![
            RULE_ENTRIES_PRESENT.to_string(),
            RULE_HASHES_MATCH.to_string(),
            RULE_FRONT_MATTER.to_string(),
            RULE_INDEX_SECTIONS.to_string(),
        ],
        violations: found.violations,
    }
}

fn check_index(
    archive: &[u8],
    by_name: &HashMap<&str, &PackEntry>,
    reference_paths: &HashSet<&str>,
    found: &mut Collector,
) {
    let Some(index) = by_name.get(INDEX_PATH) else {
        found.push(
            "SKILL_GRAPH_INDEX_MISSING",
            INDEX_PATH,
            "no index.json entry",
        );
        return;
    };
    let Ok(body) = section_bytes(archive, &index.body) else {
        found.push(
            "SKILL_GRAPH_INDEX_MISSING",
            INDEX_PATH,
            "body lies outside the archive",
        );
        return;
    };
    let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(body) else {
        found.push("SKILL_GRAPH_INDEX_MISMATCH", INDEX_PATH, "not valid JSON");
        return;
    };
    let Some(sections) = parsed.get("sections").and_then(serde_json::Value::as_array) else {
        found.push(
            "SKILL_GRAPH_INDEX_MISMATCH",
            INDEX_PATH,
            "no sections array",
        );
        return;
    };
    let mut indexed: HashSet<&str> = HashSet::with_capacity(sections.len());
    for section in sections {
        let Some(path) = section.get("path").and_then(serde_json::Value::as_str) else {
            found.push(
                "SKILL_GRAPH_INDEX_MISMATCH",
                INDEX_PATH,
                "a section has no path",
            );
            continue;
        };
        if !indexed.insert(path) {
            found.push(
                "SKILL_GRAPH_INDEX_MISMATCH",
                path,
                "listed more than once in the index",
            );
        } else if !reference_paths.contains(path) {
            found.push(
                "SKILL_GRAPH_INDEX_MISMATCH",
                path,
                "indexed but not in the manifest",
            );
        }
    }
    for path in reference_paths {
        if !indexed.contains(path) {
            found.push(
                "SKILL_GRAPH_INDEX_MISMATCH",
                path,
                "in the manifest but not indexed",
            );
        }
    }
}

fn canonical(body: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(body) {
        Ok(text) if text.contains("\r\n") => text.replace("\r\n", "\n").into_bytes(),
        _ => body.to_vec(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::connector_pack::skill_graph::{Sha256Hex, SkillGraphEntry};
    use medulla_catalog_types::connector_pack::{PackEntryKind, PackSection};
    use medulla_types::contract::Digest256;

    const GRAPH: &str = "demo";

    struct Bundle {
        archive: Vec<u8>,
        entries: Vec<PackEntry>,
        manifest: SkillGraphManifest,
    }

    fn add(bundle: &mut Bundle, path: &str, body: &[u8], declared: &[u8]) {
        let offset = bundle.archive.len() as u64;
        bundle.archive.extend_from_slice(body);
        let mut entry = medulla_catalog_types::catalog_fixtures::contract_pack::entry(
            PackEntryKind::SkillFile,
            &format!("{GRAPH}/{path}"),
        );
        entry.body = PackSection {
            offset,
            length: body.len() as u64,
            sha256: Digest256::from_bytes(Sha256::digest(body).into()),
        };
        bundle.entries.push(entry);
        bundle.manifest.entries.push(SkillGraphEntry {
            path: path.to_string(),
            sha256: Sha256Hex::new(sha256_hex(declared)).unwrap(),
        });
    }

    /// A bundle with `n` reference files; `bad_hash` declares a wrong digest on each.
    fn bundle(n: usize, bad_hash: bool) -> Bundle {
        let mut bundle = Bundle {
            archive: Vec::new(),
            entries: Vec::new(),
            manifest: SkillGraphManifest {
                entries: Vec::new(),
            },
        };
        let skill = format!("---\nname: {GRAPH}\ndescription: demo graph\n---\nbody\n");
        add(
            &mut bundle,
            SKILL_FILE_NAME,
            skill.as_bytes(),
            skill.as_bytes(),
        );
        let mut sections = Vec::with_capacity(n);
        for i in 0..n {
            let path = format!("reference/{i}.md");
            let text = format!("reference {i}\r\nline\r\n");
            let canonical_text = format!("reference {i}\nline\n");
            let declared = if bad_hash {
                b"other".as_slice()
            } else {
                canonical_text.as_bytes()
            };
            add(&mut bundle, &path, text.as_bytes(), declared);
            sections.push(serde_json::json!({ "path": path }));
        }
        let index = serde_json::json!({ "sections": sections }).to_string();
        add(&mut bundle, INDEX_PATH, index.as_bytes(), index.as_bytes());
        bundle
    }

    fn run(bundle: &Bundle) -> ConformanceReport {
        let refs: Vec<&PackEntry> = bundle.entries.iter().collect();
        validate_bundle(GRAPH, &bundle.archive, &refs, &bundle.manifest)
    }

    // spec: MEDULLA-TYPED-PACKS-R700.2
    #[test]
    fn single_pass_rules_and_violation_cap_on_10_1000_100000_entries() {
        for n in [10usize, 1_000, 100_000] {
            let good = run(&bundle(n, false));
            assert!(good.is_conformant(), "{n}: {:?}", good.violations.first());
            assert_eq!(good.checked_rules.len(), 4);

            let bad = run(&bundle(n, true));
            assert_eq!(bad.violations.len(), n.min(MAX_PACK_VIOLATIONS), "{n}");
            assert!(bad.violations[0].starts_with("SKILL_GRAPH_HASH_MISMATCH: reference/0.md"));
        }

        // A manifest path with no entry, a front-matter fault, and index drift.
        let mut broken = bundle(3, false);
        broken.manifest.entries.push(SkillGraphEntry {
            path: "reference/ghost.md".into(),
            sha256: Sha256Hex::new("0".repeat(64)).unwrap(),
        });
        let report = run(&broken);
        assert!(report
            .violations
            .iter()
            .any(|v| v.starts_with("SKILL_GRAPH_ENTRY_MISSING: reference/ghost.md")));
        assert!(report
            .violations
            .iter()
            .any(|v| v.starts_with("SKILL_GRAPH_INDEX_MISMATCH: reference/ghost.md")));

        let mut no_front = bundle(1, false);
        let skill = b"no front matter";
        let at = no_front.archive.len() as u64;
        no_front.archive.extend_from_slice(skill);
        no_front.entries[0].body = PackSection {
            offset: at,
            length: skill.len() as u64,
            sha256: Digest256::from_bytes(Sha256::digest(skill).into()),
        };
        no_front.manifest.entries[0].sha256 = Sha256Hex::new(sha256_hex(skill)).unwrap();
        let report = run(&no_front);
        assert_eq!(report.violations.len(), 1);
        assert!(report.violations[0].starts_with("SKILL_GRAPH_FRONT_MATTER: SKILL.md"));

        // No index.json entry at all.
        let mut headless = bundle(1, false);
        headless.entries.pop();
        headless.manifest.entries.pop();
        let report = run(&headless);
        assert!(report.violations[0].starts_with("SKILL_GRAPH_INDEX_MISSING"));
    }
}
