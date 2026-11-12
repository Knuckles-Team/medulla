//! Served engine-contract registry invariants (MEDULLA-CONTRACT-R054).

use medulla_capabilities::{catalog::catalog_json, contract_catalog_digest, method_policy_entries};

#[test]
fn served_descriptors_cover_all_fields() {
    let served: serde_json::Value =
        serde_json::from_str(&catalog_json()).expect("the served catalog is JSON");

    let methods = served["methods"].as_array().expect("methods array");
    assert!(!methods.is_empty());

    // The registry and the served rows name exactly the same methods.
    let registry: Vec<&str> = method_policy_entries().map(|(name, _, _)| name).collect();
    for row in methods {
        let id = row["id"].as_str().expect("method row carries an id");
        assert!(
            registry.contains(&id),
            "served row {id} has no registry row"
        );
    }
    for name in &registry {
        assert!(
            methods.iter().any(|row| row["id"] == *name),
            "registry row {name} is not served"
        );
    }

    // Every served row carries all of its contract fields, non-null, and its
    // authz_action names a registered scope.
    let scopes: Vec<&str> = served["scopes"]
        .as_array()
        .expect("scopes array")
        .iter()
        .map(|row| row["scope"].as_str().expect("scope row carries a scope"))
        .collect();
    for row in methods {
        for field in [
            "id",
            "domain",
            "is_wire_callable",
            "error_set",
            "policy",
            "replay_class",
            "consumer_profiles",
            "stability",
            "note",
        ] {
            assert!(
                row.get(field).is_some() && !row[field].is_null(),
                "{field} is missing or null on {}",
                row["id"]
            );
        }
        let policy = &row["policy"];
        for field in [
            "authz_action",
            "mutates",
            "idempotent",
            "audited",
            "emits_cdc",
            "durability_domain",
            "txn_participation",
        ] {
            assert!(
                policy.get(field).is_some() && !policy[field].is_null(),
                "policy.{field} is missing or null on {}",
                row["id"]
            );
        }
        assert!(
            scopes.contains(&policy["authz_action"].as_str().expect("authz_action")),
            "unregistered scope for {}",
            row["id"]
        );
    }

    let describe = methods
        .iter()
        .find(|row| row["id"] == "DescribeContract")
        .expect("the DescribeContract descriptor is served");
    // spec: MEDULLA-CONTRACT-R054
    assert_eq!(describe["policy"]["authz_action"], "contract:read");
    // spec: MEDULLA-CONTRACT-R054
    assert_eq!(describe["stability"], "stable");
    assert!(describe["is_wire_callable"].as_bool().expect("bool"));
    assert!(!describe["policy"]["mutates"].as_bool().expect("bool"));
    assert!(describe["consumer_profiles"]
        .as_array()
        .expect("profiles")
        .iter()
        .any(|p| p == "python"));

    // The user-class scope registry row is served.
    let scope_row = served["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["scope"] == "contract:read")
        .expect("contract:read is registered");
    // spec: MEDULLA-CONTRACT-R054
    assert_eq!(scope_row["class"], "user");

    // The digest is 64 lowercase hex and is the one the catalog pins.
    let digest = contract_catalog_digest();
    // spec: MEDULLA-CONTRACT-R054
    assert_eq!(digest.len(), 64);
    assert!(digest
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_eq!(served["catalog_digest"], digest);
}

// ── MEDULLA-CONTRACT-R044: structural Method-variant discovery ─────────────

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Variant names declared by one `method_<N>.rs` chunk. A chunk is a
/// `macro_rules!` whose body hands the variant list to the next macro inside
/// `( ... [ ... ] )`, so variants are the capitalised identifiers that begin a
/// line at bracket depth four. Comments and string literals are blanked first so
/// their braces and capitals never count.
fn chunk_file_variants(text: &str) -> Vec<String> {
    let no_comments: String = text
        .lines()
        .map(|line| line.find("//").map_or(line, |at| &line[..at]))
        .collect::<Vec<_>>()
        .join("\n");
    let mut blanked = String::with_capacity(no_comments.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in no_comments.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
                blanked.push('"');
            } else if ch == '\n' {
                blanked.push('\n');
            }
        } else {
            if ch == '"' {
                in_string = true;
            }
            blanked.push(ch);
        }
    }
    let mut depth: i32 = 0;
    let mut found = Vec::new();
    for line in blanked.lines() {
        let trimmed = line.trim();
        let name: String = trimmed
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let rest = trimmed[name.len()..].trim_start();
        let capitalised = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if depth == 4 && capitalised && (rest.is_empty() || rest.starts_with(['{', '(', ','])) {
            found.push(name);
        }
        for ch in line.chars() {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
        }
    }
    found
}

/// Every `Method` variant, read from the chunk files on disk rather than from a
/// literal enum block.
fn wire_method_variants() -> Vec<String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../medulla-protocol-types/src/protocol/method");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the method chunk directory is readable")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("method_") && n.ends_with(".rs"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no method chunk files under {dir:?}");
    files
        .iter()
        .flat_map(|path| {
            chunk_file_variants(&std::fs::read_to_string(path).expect("chunk file is readable"))
        })
        .collect()
}

fn descriptor_ids() -> Vec<String> {
    method_policy_entries()
        .map(|(name, _, _)| name.to_string())
        .collect()
}

/// `(variants without a descriptor, descriptors without a variant)`.
fn parity_gaps(variants: &[String], ids: &[String]) -> (Vec<String>, Vec<String>) {
    let v: BTreeSet<&String> = variants.iter().collect();
    let d: BTreeSet<&String> = ids.iter().collect();
    (
        v.difference(&d).map(|s| (*s).clone()).collect(),
        d.difference(&v).map(|s| (*s).clone()).collect(),
    )
}

#[test]
fn every_wire_variant_has_exactly_one_descriptor_and_vice_versa() {
    // spec: MEDULLA-CONTRACT-R044
    // spec: MEDULLA-TYPED-PACKS-R067
    let variants = wire_method_variants();
    let ids = descriptor_ids();
    assert!(
        variants.len() > 100,
        "variant discovery found too few variants"
    );
    let unique: BTreeSet<&String> = variants.iter().collect();
    assert_eq!(
        unique.len(),
        variants.len(),
        "a Method variant is declared twice"
    );
    let unique_ids: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique_ids.len(),
        ids.len(),
        "a descriptor id is registered twice"
    );
    let (without_descriptor, without_variant) = parity_gaps(&variants, &ids);
    assert!(
        without_descriptor.is_empty(),
        "variants without a descriptor: {without_descriptor:?}"
    );
    assert!(
        without_variant.is_empty(),
        "descriptors without a variant: {without_variant:?}"
    );
}

#[test]
fn variant_descriptor_parity_fails_in_both_directions() {
    // spec: MEDULLA-CONTRACT-R044
    let chunk = "macro_rules! __eg_method_chunk_9 {\n    () => {\n        __eg_method_chunk_10!(@acc [\n    /// doc { with braces\n    AddNode {\n        node_id: String,\n    },\n    BrandNewVariant { x: u8 },\n    ])\n    };\n}\n";
    let discovered = chunk_file_variants(chunk);
    assert_eq!(discovered, vec!["AddNode", "BrandNewVariant"]);

    let ids = vec!["AddNode".to_string()];
    let (no_descriptor, no_variant) = parity_gaps(&discovered, &ids);
    assert_eq!(no_descriptor, vec!["BrandNewVariant"]);
    assert!(no_variant.is_empty());

    let ids = vec![
        "AddNode".to_string(),
        "BrandNewVariant".to_string(),
        "GhostDescriptor".to_string(),
    ];
    let (no_descriptor, no_variant) = parity_gaps(&discovered, &ids);
    assert!(no_descriptor.is_empty());
    assert_eq!(no_variant, vec!["GhostDescriptor"]);
}
