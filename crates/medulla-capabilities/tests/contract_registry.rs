//! The engine contract is a BIJECTION with the `Method` enum (RF-RULING-003).
//!
//! `tests/invariants.rs` can only check the registry against itself: a real `Method`
//! value cannot be constructed generically for every variant (most carry required,
//! non-`Default` fields), so no runtime enumeration exists. This file closes the loop
//! the way the deleted `tests/test_protocol_parity.py` did — by reading the wire truth,
//! `crates/medulla-protocol-types/src/protocol.rs`, as text — but with NO baseline file: a variant
//! without a descriptor, or a descriptor without a variant, is a hard failure.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Names enforced ABSENT from the wire: they were retired, and re-adding one silently
/// would restore a surface an earlier decision removed.
const RETIRED_METHODS: &[&str] = &[
    "BatchCosineSimilarity",
    "SpectralCluster",
    "HypergraphEncodeInteraction",
    "FindSimilarPairs",
    // EH-434: one query-text surface — UQL text is `Uql` (`TxnUql` inside a txn).
    "UnifiedQueryText",
    "TxnUnifiedQueryText",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/medulla-capabilities is two levels below the repo root")
        .to_path_buf()
}

/// The chunk files that build `pub enum Method { .. }`, discovered from
/// `crates/medulla-protocol-types/src/protocol/method/mod.rs` rather than hardcoded.
///
/// `crates/medulla-protocol-types/src/protocol.rs` is now a facade (`mod method;`): the enum is
/// assembled by a `__eg_method_chunk_0..N` macro chain, each chunk file appending its
/// own variants to an `@acc` token-tree accumulator before handing off to the next, with
/// `__eg_method_finish` wrapping the final accumulator in `pub enum Method { $($variants)* }`
/// (`crates/medulla-protocol-types/src/protocol/method/method_finish.rs`). That means the literal text
/// `pub enum Method {` still exists, but its body there is only the macro placeholder
/// `$($variants)*` -- reading just that file finds the wrapper and none of the variants.
/// Discovering the chunk list from `mod.rs`'s own `mod method_NN;` declarations (instead
/// of hardcoding `method_00..method_10`) means a future chunk added to the chain is
/// picked up automatically rather than silently skipped.
fn method_chunk_files() -> Vec<PathBuf> {
    let dir = repo_root().join("crates/medulla-protocol-types/src/protocol/method");
    let mod_path = dir.join("mod.rs");
    let text = std::fs::read_to_string(&mod_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", mod_path.display()));
    let files: Vec<PathBuf> = text
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("mod ")?.strip_suffix(';'))
        .filter(|name| {
            name.strip_prefix("method_").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .map(|name| dir.join(format!("{name}.rs")))
        .collect();
    assert!(
        !files.is_empty(),
        "could not locate any `mod method_<N>;` declaration in {} -- the Method enum's \
         macro chain moved again and this gate needs to follow it there",
        mod_path.display()
    );
    files
}

/// Top-level variant identifiers of `pub enum Method { .. }`, walked across its chunk
/// files (see [`method_chunk_files`]).
///
/// Within each chunk file, variants sit at exactly 4-space indent; struct fields are
/// deeper, and attributes, doc comments and the macro's own syntax (`macro_rules!`,
/// `(@acc [$($variants:tt)*])`, the `$($variants)*` re-embed of earlier chunks) start
/// with something other than an uppercase ASCII letter, so the same column-based filter
/// that used to read the literal enum body reads a chunk body just as well.
fn wire_method_variants() -> BTreeSet<String> {
    method_chunk_files()
        .iter()
        .flat_map(|path| chunk_file_variants(path))
        .collect()
}

/// The `Method` variants one chunk file contributes (see [`wire_method_variants`]).
///
/// Fails loudly, naming the file, on either way this can go silently wrong: the file no
/// longer defining a chunk macro at all (the chain restructured again), or defining one
/// that -- at exactly 4-space indent, uppercase-starting -- contributes zero variants
/// (the accumulator's layout changed under this filter).
fn chunk_file_variants(path: &std::path::Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    assert!(
        text.contains("macro_rules! __eg_method_chunk_"),
        "{} no longer defines an `__eg_method_chunk_*` macro -- the Method enum's macro \
         chain moved again and this gate needs to follow it there",
        path.display()
    );
    let found: BTreeSet<String> = text
        .lines()
        .filter_map(variant_name_at_enum_indent)
        .collect();
    assert!(
        !found.is_empty(),
        "{} contributed no Method variants -- confirm its variant lines are still at \
         4-space indent",
        path.display()
    );
    found
}

/// A variant name if `line` is a top-level variant row (exactly 4-space indent,
/// uppercase-starting) -- `None` for struct fields (deeper), attributes, doc comments,
/// and the macro's own syntax (`macro_rules!`, `(@acc [$($variants:tt)*])`, the
/// `$($variants)*` re-embed of earlier chunks), none of which starts this way.
fn variant_name_at_enum_indent(line: &str) -> Option<String> {
    let rest = line.strip_prefix("    ")?;
    if !rest.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    Some(
        rest.chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect(),
    )
}

fn descriptor_ids() -> Vec<String> {
    medulla_capabilities::method_descriptors()
        .map(|d| d.id.as_str().to_string())
        .collect()
}

/// The bijection can only hold under the CANONICAL profile, and that is a property of the
/// two sides, not a convenience: the text scan is cfg-BLIND (it sees all 413 variants,
/// 139 of them behind a `#[cfg(feature = ...)]`) while `method_descriptors()` is
/// cfg-CONDITIONAL (7 rows behind `jobs`/`statechart`/`knowledge-batch`/
/// `modality-serving`/`quantum`/`asr-native`/`viz`). `canonical-ledger` selects exactly
/// those 7 and medulla-capabilities force-enables every other Method-gating `medulla-types`
/// feature, so the two sides can agree. A variant added behind a NEW feature that
/// `canonical-ledger` does not select fails this test loudly, which is the correct
/// outcome -- the canonical ledger would no longer be the complete inventory.
// spec: MEDULLA-CONTRACT-R044, MEDULLA-TYPED-PACKS-R067
#[cfg(feature = "canonical-ledger")]
#[test]
fn every_wire_variant_has_exactly_one_descriptor_and_vice_versa() {
    let ids = descriptor_ids();
    let unique: BTreeSet<String> = ids.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "the contract registry declares a duplicate method id"
    );
    let variants = wire_method_variants();
    let missing: Vec<_> = variants.difference(&unique).collect();
    let extra: Vec<_> = unique.difference(&variants).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "contract registry ⇄ `Method` enum are not a bijection\n\
         missing descriptors ({}): {missing:?}\n\
         descriptors with no wire variant ({}): {extra:?}",
        missing.len(),
        extra.len(),
    );
    // No exact count: the bijection above is the census. A hard-coded count made every
    // two method-adding changes conflict on one line (MEDULLA-SDLC-R020).
}

// spec: MEDULLA-FEDERATED-QUERY-R020
#[test]
fn retired_methods_never_reappear() {
    let variants = wire_method_variants();
    let ids: BTreeSet<String> = descriptor_ids().into_iter().collect();
    for retired in RETIRED_METHODS {
        assert!(
            !variants.contains(*retired),
            "{retired} was retired from the wire but is back in `Method`"
        );
        assert!(
            !ids.contains(*retired),
            "{retired} was retired from the wire but is back in the contract registry"
        );
    }
}

#[test]
fn every_descriptor_names_a_domain_and_an_authz_scope() {
    for descriptor in medulla_capabilities::method_descriptors() {
        let id = descriptor.id.as_str();
        assert!(
            !descriptor.domain.is_empty(),
            "{id}: descriptor has no domain"
        );
        assert!(
            descriptor.policy.authz_action.contains(':'),
            "{id}: authz_action must be `primitive:verb`"
        );
        assert!(
            !descriptor.error_set.is_empty(),
            "{id}: every method declares at least one error code"
        );
    }
}

#[test]
fn internal_methods_declare_no_consumer_and_stable_methods_declare_one() {
    for descriptor in medulla_capabilities::method_descriptors() {
        let id = descriptor.id.as_str();
        match descriptor.stability {
            medulla_capabilities::Stability::Internal => assert!(
                descriptor.consumer_profiles.is_empty(),
                "{id}: an internal method must generate no consumer surface"
            ),
            _ => assert!(
                !descriptor.consumer_profiles.is_empty(),
                "{id}: a published method must name at least one consumer profile"
            ),
        }
    }
}

#[test]
fn session_usage_requires_the_shared_usage_read_scope() {
    let descriptor = medulla_capabilities::method_descriptors()
        .find(|descriptor| descriptor.id.as_str() == "GetSessionUsage")
        .unwrap();
    assert_eq!(descriptor.policy.authz_action, "usage:read");
}

/// Verifies MEDULLA-UNIFIED-DATA-PLANE-R760.3: Wiring: five served methods over the incident authority
// spec: MEDULLA-UNIFIED-DATA-PLANE-R760.3
#[test]
fn incident_methods_listed() {
    let variants = wire_method_variants();
    let expected = [
        ("IncidentRecordAnomaly", "incident:write", true),
        ("IncidentCorrelate", "incident:write", true),
        ("IncidentEvidence", "incident:read", false),
        ("IncidentRelatedClaims", "incident:read", false),
        ("IncidentSetStatus", "incident:write", true),
    ];
    for (id, scope, mutates) in expected {
        assert!(variants.contains(id), "{id} is not a wire `Method` variant");
        let descriptor = medulla_capabilities::method_descriptors()
            .find(|descriptor| descriptor.id.as_str() == id)
            .unwrap_or_else(|| panic!("{id} has no contract descriptor"));
        assert_eq!(descriptor.policy.authz_action, scope, "{id}: scope");
        assert_eq!(descriptor.policy.mutates, mutates, "{id}: mutates");
        assert_eq!(descriptor.domain, "ingestion", "{id}: domain");
    }
}

// spec: MEDULLA-TYPED-PACKS-R063
/// The agent-layer shared schema version constants, each with its pinned value
/// and every record type that embeds it.
///
/// R063's rule: a change to a shared schema version names the agent-layer
/// record types it changes, in the change's `Spec` trailer. This table IS that
/// enumeration, and the test below fails in every drift direction:
///   * a new `pub const *_SCHEMA_VERSION: u16` in `medulla-agent-types` that is
///     not listed here (a new shared version nobody named);
///   * a record type with a `pub schema_version: u16` field that is not named
///     under its constant (a missing name in the enumeration);
///   * a constant renamed, or a pinned value that no longer matches the live
///     constant (a bump that skipped re-enumerating the table).
const AGENT_LAYER_SCHEMA_BUMPS: &[(&str, u16, &[&str])] = &[
    (
        "AGENT_LIBRARY_ENTRY_SCHEMA_VERSION",
        5,
        &["AgentLibraryEntry"],
    ),
    (
        "AGENT_LIBRARY_OUTBOX_SCHEMA_VERSION",
        1,
        &["AgentLibraryOutboxEvent"],
    ),
    (
        "AGENT_LIBRARY_RESULT_SCHEMA_VERSION",
        1,
        &["AgentLibraryCommittedResult"],
    ),
    (
        "AGENT_GRAPH_ENTRY_SCHEMA_VERSION",
        2,
        &[
            "AgentGraphEntry",
            "AgentGraphCommittedResult",
            "AgentGraphOutboxEvent",
        ],
    ),
    (
        "AGENT_COMPONENT_SCHEMA_VERSION",
        3,
        &[
            "AgentComponentEntry",
            "AgentComponentCommittedResult",
            "AgentComponentOutboxEvent",
        ],
    ),
    (
        "AGENT_TEMPLATE_SCHEMA_VERSION",
        2,
        &[
            "AgentTemplateEntry",
            "AgentTemplateCommittedResult",
            "AgentTemplateOutboxEvent",
        ],
    ),
    (
        "GUARDRAIL_ENTAILMENT_RESULT_SCHEMA_VERSION",
        1,
        &["GuardrailEntailmentResult"],
    ),
    (
        "COMPONENT_CONTENT_SCHEMA_VERSION",
        1,
        &["AgentComponentContentResult"],
    ),
];

fn agent_layer_rust_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![repo_root().join("crates/medulla-agent-types/src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
        {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    out
}

/// Every `pub const <NAME>_SCHEMA_VERSION: u16 = <n>;` in the agent-layer crate,
/// with its value, read as text the same way the Method enum is read above --
/// no baseline file, so a new shared version constant cannot appear silently.
fn agent_layer_schema_constants() -> BTreeSet<(String, u16)> {
    let mut found = BTreeSet::new();
    for path in agent_layer_rust_files() {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        for line in text.lines() {
            let rest = match line.trim().strip_prefix("pub const ") {
                Some(rest) => rest,
                None => continue,
            };
            let (name, rest) = match rest.split_once(':') {
                Some((name, rest)) => (name, rest),
                None => continue,
            };
            if !name.ends_with("_SCHEMA_VERSION") || !rest.trim().starts_with("u16 = ") {
                continue;
            }
            let value = rest
                .trim()
                .strip_prefix("u16 = ")
                .and_then(|v| v.strip_suffix(';'))
                .and_then(|v| v.parse::<u16>().ok())
                .unwrap_or_else(|| {
                    panic!(
                        "{}: unparseable shared schema version value: {rest:?}",
                        path.display()
                    )
                });
            found.insert((name.to_string(), value));
        }
    }
    found
}

/// Every agent-layer record type that embeds a `pub schema_version: u16` field:
/// a `pub struct` name that owns such a field before its closing brace at
/// column 0. A struct with no version field contributes nothing.
fn agent_layer_versioned_record_types() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for path in agent_layer_rust_files() {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let mut current: Option<String> = None;
        for line in text.lines() {
            if let Some(name) = line.trim().strip_prefix("pub struct ").map(|rest| {
                rest.chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect::<String>()
            }) {
                if name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                    current = Some(name);
                    continue;
                }
            }
            if line.starts_with('}') {
                current = None;
                continue;
            }
            if current.is_some() && line.trim() == "pub schema_version: u16," {
                if let Some(name) = current.take() {
                    found.insert(name);
                }
            }
        }
    }
    found
}

fn agent_layer_schema_constant_value(name: &str) -> u16 {
    match name {
        "AGENT_LIBRARY_ENTRY_SCHEMA_VERSION" => {
            medulla_agent_types::agent_library::AGENT_LIBRARY_ENTRY_SCHEMA_VERSION
        }
        "AGENT_LIBRARY_OUTBOX_SCHEMA_VERSION" => {
            medulla_agent_types::agent_library::AGENT_LIBRARY_OUTBOX_SCHEMA_VERSION
        }
        "AGENT_LIBRARY_RESULT_SCHEMA_VERSION" => {
            medulla_agent_types::agent_library::AGENT_LIBRARY_RESULT_SCHEMA_VERSION
        }
        "AGENT_GRAPH_ENTRY_SCHEMA_VERSION" => {
            medulla_agent_types::agent_graph::AGENT_GRAPH_ENTRY_SCHEMA_VERSION
        }
        "AGENT_COMPONENT_SCHEMA_VERSION" => {
            medulla_agent_types::agent_component::AGENT_COMPONENT_SCHEMA_VERSION
        }
        "AGENT_TEMPLATE_SCHEMA_VERSION" => {
            medulla_agent_types::agent_template::AGENT_TEMPLATE_SCHEMA_VERSION
        }
        "GUARDRAIL_ENTAILMENT_RESULT_SCHEMA_VERSION" => {
            medulla_agent_types::agent_guardrail::GUARDRAIL_ENTAILMENT_RESULT_SCHEMA_VERSION
        }
        "COMPONENT_CONTENT_SCHEMA_VERSION" => {
            medulla_agent_types::agent_component::content::COMPONENT_CONTENT_SCHEMA_VERSION
        }
        other => panic!("enumeration names unknown constant {other}"),
    }
}

#[test]
fn schema_bump_enumerates_agent_layer_types() {
    let constants = agent_layer_schema_constants();
    let records = agent_layer_versioned_record_types();

    let enumerated_constants: BTreeSet<String> = AGENT_LAYER_SCHEMA_BUMPS
        .iter()
        .map(|(name, _, _)| name.to_string())
        .collect();
    let unenumerated: Vec<&String> = constants
        .iter()
        .map(|(name, _)| name)
        .filter(|name| !enumerated_constants.contains(*name))
        .collect();
    assert!(
        unenumerated.is_empty(),
        "unenumerated shared schema version constant(s) {unenumerated:?}: add each to \
         AGENT_LAYER_SCHEMA_BUMPS with the record types it changes, named in the \
         change's `Spec` trailer"
    );
    let stale_constants: Vec<&String> = enumerated_constants
        .iter()
        .filter(|name| !constants.iter().any(|(n, _)| n == *name))
        .collect();
    assert!(
        stale_constants.is_empty(),
        "enumerated constant(s) {stale_constants:?} no longer exist in medulla-agent-types"
    );

    // A bump changes a pinned value: it fails here until the table is
    // re-enumerated for the new value.
    for (name, pinned, _) in AGENT_LAYER_SCHEMA_BUMPS {
        let actual = agent_layer_schema_constant_value(name);
        assert_eq!(
            actual, *pinned,
            "{name} moved {pinned} -> {actual}: the bump must name the record types it \
             changes in its `Spec` trailer and update this table"
        );
    }

    let enumerated_records: BTreeSet<String> = AGENT_LAYER_SCHEMA_BUMPS
        .iter()
        .flat_map(|(_, _, records)| records.iter().map(|name| name.to_string()))
        .collect();
    let unenumerated_records: Vec<&String> = records
        .iter()
        .filter(|record| !enumerated_records.contains(*record))
        .collect();
    assert!(
        unenumerated_records.is_empty(),
        "record type(s) {unenumerated_records:?} embed a `schema_version: u16` field but \
         are not named in AGENT_LAYER_SCHEMA_BUMPS -- a missing name fails"
    );
    let stale_records: Vec<&String> = enumerated_records
        .iter()
        .filter(|record| !records.contains(*record))
        .collect();
    assert!(
        stale_records.is_empty(),
        "enumerated record type(s) {stale_records:?} no longer embed a \
         `schema_version: u16` field"
    );
}
