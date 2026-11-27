//! Row-count and sampled-key parity of a registered foreign source
//! (MEDULLA-UNIFIED-DATA-PLANE-R770).
//!
//! An attached source is checked against its prior connector by reading the same tables
//! from both and comparing two things: the row count of each table and one digest over the
//! first `sample` keys in sorted `(table, key)` order. The key never carries the source
//! name, so two sources that hold the same rows produce the same digest. The answer holds
//! counts and a digest only; no row content leaves.
//!
//! A table `t` of source `name` is the registered source `name` when `t == name` and the
//! registered source `name.t` otherwise. A table that no registered source answers refuses
//! `SOURCE_NOT_FOUND`, the same code an invisible or unregistered source gets.

use std::collections::BTreeMap;

use medulla_catalog_types::foreign_source_catalog::{
    ForeignSourceParityAnswer, MAX_PARITY_SAMPLE, MAX_PARITY_TABLES, PARITY_SAMPLE_INVALID,
    PARITY_TABLES_INVALID, SOURCE_NOT_FOUND,
};
use medulla_federation::federation::ForeignSourceRegistry;
use medulla_types::contract::Digest256;

/// The domain the sample digest is framed under.
const PARITY_DIGEST_DOMAIN: &[u8] = b"eg/foreign-source-parity/v1";

/// The registered source that holds table `table` of source `name`.
pub fn table_source(name: &str, table: &str) -> String {
    if table == name {
        name.to_string()
    } else {
        format!("{name}.{table}")
    }
}

/// Validate the shape of a parity request before any source is read.
pub fn validate_request(tables: &[String], sample: u32) -> Result<(), String> {
    if tables.is_empty()
        || tables.len() > MAX_PARITY_TABLES
        || tables.iter().any(|table| table.trim().is_empty())
    {
        return Err(format!(
            "{PARITY_TABLES_INVALID}: a parity request names 1 to {MAX_PARITY_TABLES} non-blank tables"
        ));
    }
    if sample == 0 || sample > MAX_PARITY_SAMPLE {
        return Err(format!(
            "{PARITY_SAMPLE_INVALID}: a sample is 1 to {MAX_PARITY_SAMPLE} keys"
        ));
    }
    Ok(())
}

/// Count the rows of every table and digest the first `sample` keys.
///
/// `registry` is the caller's own owner-scoped registry, so a source the caller cannot see
/// is simply absent from it. The read is a full fetch of each table, which blocks on the
/// source: the caller runs it off the reactor.
pub fn parity(
    registry: &ForeignSourceRegistry,
    name: &str,
    tables: &[String],
    sample: u32,
) -> Result<ForeignSourceParityAnswer, String> {
    validate_request(tables, sample)?;
    let mut counts = BTreeMap::new();
    let mut keys: Vec<(String, String)> = Vec::new();
    for table in tables {
        let source = table_source(name, table);
        if registry.get(&source).is_none() {
            return Err(format!(
                "{SOURCE_NOT_FOUND}: no source {name} is visible to the caller"
            ));
        }
        let rows = registry.resolve(&source)?;
        counts.insert(table.clone(), rows.len() as u64);
        keys.extend(rows.rows().iter().map(|row| (table.clone(), row.id.clone())));
    }
    keys.sort();
    keys.dedup();
    keys.truncate(sample as usize);
    let mut framed = Vec::new();
    for (table, key) in &keys {
        for part in [table, key] {
            framed.extend_from_slice(&(part.len() as u64).to_be_bytes());
            framed.extend_from_slice(part.as_bytes());
        }
    }
    let digest = Digest256::framed(PARITY_DIGEST_DOMAIN, &[&framed])?;
    Ok(ForeignSourceParityAnswer {
        counts,
        sample_digest: format!("sha256:{}", digest.to_hex()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(entries: &[(&str, &[&str])]) -> ForeignSourceRegistry {
        let mut registry = ForeignSourceRegistry::new();
        for (name, ids) in entries {
            registry.register_table(
                name.to_string(),
                ids.iter().map(|id| (id.to_string(), None)),
            );
        }
        registry
    }

    fn table_names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R770
    #[test]
    fn a_source_and_its_prior_connector_over_the_same_rows_agree() {
        let attached = registry(&[("crm.accounts", &["b", "a", "c"]), ("crm.deals", &["x"])]);
        let prior = registry(&[("legacy.accounts", &["c", "a", "b"]), ("legacy.deals", &["x"])]);
        let requested = table_names(&["accounts", "deals"]);
        let left = parity(&attached, "crm", &requested, 10).expect("attached parity");
        let right = parity(&prior, "legacy", &requested, 10).expect("prior parity");
        assert_eq!(left, right);
        assert_eq!(left.counts["accounts"], 3);
        assert_eq!(left.counts["deals"], 1);
        assert!(left.sample_digest.starts_with("sha256:"));
        assert_eq!(left.sample_digest.len(), "sha256:".len() + 64);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R770
    #[test]
    fn a_missing_key_or_a_different_count_changes_the_answer() {
        let requested = table_names(&["accounts"]);
        let base = parity(&registry(&[("crm.accounts", &["a", "b"])]), "crm", &requested, 10)
            .expect("base");
        let missing = parity(&registry(&[("crm.accounts", &["a"])]), "crm", &requested, 10)
            .expect("missing");
        assert_ne!(base.counts, missing.counts);
        assert_ne!(base.sample_digest, missing.sample_digest);
        let swapped = parity(&registry(&[("crm.accounts", &["a", "z"])]), "crm", &requested, 10)
            .expect("swapped");
        assert_eq!(base.counts, swapped.counts);
        assert_ne!(base.sample_digest, swapped.sample_digest);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R770
    #[test]
    fn the_sample_covers_only_the_first_keys_in_sorted_order() {
        let requested = table_names(&["accounts"]);
        let small = parity(&registry(&[("crm.accounts", &["a", "b", "c"])]), "crm", &requested, 2)
            .expect("small");
        let other_tail =
            parity(&registry(&[("crm.accounts", &["a", "b", "z"])]), "crm", &requested, 2)
                .expect("other tail");
        assert_eq!(small.sample_digest, other_tail.sample_digest);
        assert_eq!(small.counts["accounts"], 3);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R770
    #[test]
    fn a_table_equal_to_the_source_name_reads_the_source_itself() {
        let one = registry(&[("orders", &["1", "2"])]);
        let answer = parity(&one, "orders", &table_names(&["orders"]), 5).expect("self table");
        assert_eq!(answer.counts["orders"], 2);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R770
    #[test]
    fn a_bad_request_or_an_unknown_table_refuses_with_its_code() {
        let reg = registry(&[("crm.accounts", &["a"])]);
        let refused = |names: &[&str], sample: u32| {
            parity(&reg, "crm", &table_names(names), sample).expect_err("refused")
        };
        assert!(refused(&[], 5).starts_with(PARITY_TABLES_INVALID));
        assert!(refused(&[" "], 5).starts_with(PARITY_TABLES_INVALID));
        assert!(refused(&["accounts"], 0).starts_with(PARITY_SAMPLE_INVALID));
        assert!(refused(&["accounts"], MAX_PARITY_SAMPLE + 1).starts_with(PARITY_SAMPLE_INVALID));
        assert!(refused(&["contacts"], 5).starts_with(SOURCE_NOT_FOUND));
    }
}
