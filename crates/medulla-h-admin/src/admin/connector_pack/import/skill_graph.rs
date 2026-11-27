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

use medulla_catalog_types::connector_pack::skill_graph::{
    ConformanceReport, SkillGraphLayer, SkillGraphManifest, SKILL_GRAPH_LAYER_NS,
};
use medulla_catalog_types::connector_pack::{
    PackEntry, PackEntryKind, PackViolation, PackViolationCode, MAX_PACK_VIOLATIONS,
};
use sha2::{Digest, Sha256};

use super::front_matter::declared_skill_type;
use super::json::parse_bounded_json;
use super::validation::{section_bytes, valid_skill_frontmatter, violation};

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

const SOURCES_PATH: &str = "sources.json";
const GRAPH_SKILL_TYPE: &str = "graph";
/// The name prefix of the pack ontology entry that holds a graph's concept layer.
const LAYER_ENTRY_PREFIX: &str = "skillgraph/";

/// Validates the bundle of every skill graph the pack declares (R700.3): a
/// `Skill` entry whose front matter says `skill_type: graph` names a graph, and
/// the `skill_file` entries named `<graph>/...` are its bundle. Each violation
/// of `validate_bundle` joins the import's violations on the `Skill` entry, so
/// a failing bundle refuses the whole import before anything is committed.
pub(super) fn validate_graph_bundles(
    archive: &[u8],
    all: &[&PackEntry],
    violations: &mut Vec<PackViolation>,
) -> Option<ConformanceReport> {
    // The reports of every graph the pack holds, merged: each rule id once, in the order
    // first checked, and every violation (MEDULLA-TYPED-PACKS-R700.5).
    let mut merged: Option<ConformanceReport> = None;
    for skill in all
        .iter()
        .filter(|entry| entry.kind == PackEntryKind::Skill)
    {
        let declares_graph = section_bytes(archive, &skill.body)
            .ok()
            .and_then(|body| declared_skill_type(body).ok().flatten())
            == Some(GRAPH_SKILL_TYPE);
        if !declares_graph {
            continue;
        }
        let graph = skill.name.as_str();
        let prefix = format!("{graph}/");
        let sources = all.iter().find(|entry| {
            entry.kind == PackEntryKind::SkillFile
                && entry.name.strip_prefix(prefix.as_str()) == Some(SOURCES_PATH)
        });
        let manifest = sources
            .ok_or_else(|| "no sources.json entry".to_string())
            .and_then(|entry| {
                section_bytes(archive, &entry.body)
                    .map_err(|_| "sources.json lies outside the archive".to_string())
            })
            .and_then(parse_bounded_json)
            .and_then(|json| {
                SkillGraphManifest::from_sources_json(&json).map_err(|error| error.to_string())
            });
        let manifest = match manifest {
            Ok(manifest) => manifest,
            Err(detail) => {
                violations.push(violation(
                    PackViolationCode::MalformedBody,
                    Some(&skill.uri),
                    &format!("SKILL_GRAPH_ENTRY_MISSING: {SOURCES_PATH}: {detail}"),
                ));
                continue;
            }
        };
        let report = validate_bundle(graph, archive, all, &manifest);
        violations.extend(
            report.violations.iter().map(|message| {
                violation(PackViolationCode::MalformedBody, Some(&skill.uri), message)
            }),
        );
        let held = merged.get_or_insert_with(ConformanceReport::default);
        for rule in report.checked_rules {
            if !held.checked_rules.contains(&rule) {
                held.checked_rules.push(rule);
            }
        }
        held.violations.extend(report.violations);
    }
    merged
}

/// The concept layers of the pack's skill graphs (MEDULLA-TYPED-PACKS-R701.2).
///
/// For every graph skill the pack holds, in entry order, that has an ontology entry named
/// `skillgraph/<graph>`: the ontology IRI that entry declares must be the layer IRI
/// `SKILL_GRAPH_LAYER_NS` plus the graph name, and each class it declares must reach a
/// parent class through the existing `ontology_orphan_classes` computation (R105), so the
/// IRI and the roll-up are checked in one pass over the entry. A graph with no layer entry
/// contributes nothing; the first violation returns. The cost is O(total classes).
#[cfg(feature = "rdf")]
pub(super) fn bind_layers(
    archive: &[u8],
    all: &[&PackEntry],
) -> Result<Vec<SkillGraphLayer>, PackViolation> {
    // A `skillgraph/<graph>` ontology entry with no graph skill is refused by name
    // (MEDULLA-TYPED-PACKS-R701.4).
    for layer in all
        .iter()
        .filter(|entry| entry.kind == PackEntryKind::Ontology)
    {
        let Some(graph) = layer.name.strip_prefix(LAYER_ENTRY_PREFIX) else {
            continue;
        };
        let has_graph = all.iter().any(|entry| {
            entry.kind == PackEntryKind::Skill
                && entry.name == graph
                && section_bytes(archive, &entry.body)
                    .ok()
                    .and_then(|body| declared_skill_type(body).ok().flatten())
                    == Some(GRAPH_SKILL_TYPE)
        });
        if !has_graph {
            return Err(violation(
                PackViolationCode::SkillGraphLayerWithoutGraph,
                Some(&layer.uri),
                &format!(
                    "SKILL_GRAPH_LAYER_WITHOUT_GRAPH: the ontology entry {} has no graph skill {graph}",
                    layer.name
                ),
            ));
        }
    }
    let mut layers = Vec::new();
    for skill in all
        .iter()
        .filter(|entry| entry.kind == PackEntryKind::Skill)
    {
        let declares_graph = section_bytes(archive, &skill.body)
            .ok()
            .and_then(|body| declared_skill_type(body).ok().flatten())
            == Some(GRAPH_SKILL_TYPE);
        if !declares_graph {
            continue;
        }
        let graph = skill.name.as_str();
        let layer_name = format!("{LAYER_ENTRY_PREFIX}{graph}");
        let Some(layer) = all
            .iter()
            .find(|entry| entry.kind == PackEntryKind::Ontology && entry.name == layer_name)
        else {
            continue;
        };
        let text = section_bytes(archive, &layer.body)
            .ok()
            .and_then(|body| std::str::from_utf8(body).ok())
            .ok_or_else(|| {
                violation(
                    PackViolationCode::OntologyInvalid,
                    Some(&layer.uri),
                    "the skill graph layer ontology is not readable text",
                )
            })?;
        let expected = format!("{SKILL_GRAPH_LAYER_NS}{graph}");
        let (classes, ontologies) = medulla_rdf::pack::declared_classes_and_ontologies(text);
        if ontologies.len() != 1 || !ontologies.contains(&expected) {
            let found = ontologies.iter().cloned().collect::<Vec<_>>().join(", ");
            return Err(violation(
                PackViolationCode::SkillGraphLayerIriMismatch,
                Some(&layer.uri),
                &format!(
                    "SKILL_GRAPH_LAYER_IRI_MISMATCH: graph {graph} declares ontology IRI \
                     [{found}], expected {expected}"
                ),
            ));
        }
        let orphans = medulla_rdf::pack::ontology_orphan_classes(
            &[text],
            super::validation::parent_classes(),
        );
        if !orphans.is_empty() {
            return Err(violation(
                PackViolationCode::OntologyOrphan,
                Some(&layer.uri),
                &super::validation::orphan_detail(&orphans),
            ));
        }
        let class_count = u32::try_from(classes.len()).unwrap_or(u32::MAX);
        layers.push(
            SkillGraphLayer::new(graph, expected, class_count).map_err(|error| {
                violation(
                    PackViolationCode::OntologyInvalid,
                    Some(&layer.uri),
                    &error.to_string(),
                )
            })?,
        );
    }
    Ok(layers)
}

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

    /// A pack of one graph skill and, when `layer` is given, its `skillgraph/<graph>`
    /// ontology entry.
    #[cfg(feature = "rdf")]
    fn layer_pack(layers: &[(&str, Option<&str>)]) -> (Vec<u8>, Vec<PackEntry>) {
        let mut archive = Vec::new();
        let mut entries = Vec::new();
        let mut put = |kind: PackEntryKind, name: &str, body: &[u8]| {
            let offset = archive.len() as u64;
            archive.extend_from_slice(body);
            let mut entry =
                medulla_catalog_types::catalog_fixtures::contract_pack::entry(kind, name);
            entry.body = PackSection {
                offset,
                length: body.len() as u64,
                sha256: Digest256::from_bytes(Sha256::digest(body).into()),
            };
            entries.push(entry);
        };
        for (graph, layer) in layers {
            let skill = format!(
                "---\nname: {graph}\ndescription: demo graph\nskill_type: graph\n---\nbody\n"
            );
            put(PackEntryKind::Skill, graph, skill.as_bytes());
            if let Some(turtle) = layer {
                put(
                    PackEntryKind::Ontology,
                    &format!("skillgraph/{graph}"),
                    turtle.as_bytes(),
                );
            }
        }
        (archive, entries)
    }

    #[cfg(feature = "rdf")]
    fn layer_turtle(ontology_iri: &str, parent: &str) -> String {
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             <{ontology_iri}> a owl:Ontology .\n\
             <{ontology_iri}#Concept> a owl:Class ; rdfs:subClassOf <{parent}> .\n\
             <{ontology_iri}#Topic> a owl:Class ; rdfs:subClassOf <{ontology_iri}#Concept> .\n"
        )
    }

    // spec: MEDULLA-TYPED-PACKS-R701.2
    #[cfg(feature = "rdf")]
    #[test]
    fn layer_iri_and_rollup_checked_in_one_pass() {
        let layer_iri = format!("{SKILL_GRAPH_LAYER_NS}{GRAPH}");
        let core_class = medulla_types::ontology_iri!("core#CreativeWork");
        let bind = |turtle: Option<&str>| {
            let (archive, entries) = layer_pack(&[(GRAPH, turtle)]);
            let refs: Vec<&PackEntry> = entries.iter().collect();
            bind_layers(&archive, &refs)
        };

        // A graph with no layer entry binds nothing.
        assert_eq!(bind(None), Ok(Vec::new()));

        // The right IRI and a roll-up to a core class bind the layer with its class count.
        let good = layer_turtle(&layer_iri, core_class);
        let layers = bind(Some(good.as_str())).unwrap();
        assert_eq!(layers.len(), 1);
        let layer = &layers[0];
        assert_eq!(layer.graph, GRAPH);
        assert_eq!(layer.ontology_iri, layer_iri);
        assert_eq!(layer.class_count, 2);

        // A foreign ontology IRI is refused by name with the found IRI.
        let foreign = layer_turtle("https://example.org/ontology/other", core_class);
        let refused = bind(Some(foreign.as_str())).unwrap_err();
        assert_eq!(refused.code, PackViolationCode::SkillGraphLayerIriMismatch);
        assert!(refused.detail.starts_with("SKILL_GRAPH_LAYER_IRI_MISMATCH"));
        assert!(refused
            .detail
            .contains("https://example.org/ontology/other"));

        // A class that does not roll up to the core is refused as an orphan.
        let orphan = layer_turtle(&layer_iri, "https://example.org/ontology/unrelated#Thing");
        let refused = bind(Some(orphan.as_str())).unwrap_err();
        assert_eq!(refused.code, PackViolationCode::OntologyOrphan);

        // Two graphs: both layers are checked and both are returned.
        let second_iri = format!("{SKILL_GRAPH_LAYER_NS}other");
        let second_good = layer_turtle(&second_iri, core_class);
        let (archive, entries) = layer_pack(&[
            (GRAPH, Some(good.as_str())),
            ("other", Some(second_good.as_str())),
        ]);
        let refs: Vec<&PackEntry> = entries.iter().collect();
        let layers = bind_layers(&archive, &refs).unwrap();
        assert_eq!(
            layers
                .iter()
                .map(|layer| layer.graph.as_str())
                .collect::<Vec<_>>(),
            vec![GRAPH, "other"]
        );

        // A wrong IRI on the second graph's layer is refused by name.
        let second_bad = layer_turtle("https://example.org/ontology/second", core_class);
        let (archive, entries) = layer_pack(&[
            (GRAPH, Some(good.as_str())),
            ("other", Some(second_bad.as_str())),
        ]);
        let refs: Vec<&PackEntry> = entries.iter().collect();
        let refused = bind_layers(&archive, &refs).unwrap_err();
        assert_eq!(refused.code, PackViolationCode::SkillGraphLayerIriMismatch);
        assert!(refused.detail.contains("graph other declares"));
    }
}
