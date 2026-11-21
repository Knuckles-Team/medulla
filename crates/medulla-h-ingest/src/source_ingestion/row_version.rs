//! Row-version evidence of a source record (MEDULLA-INGEST-WORKER-R901.2).
//!
//! A record from a table source may carry the payload member `_row_version{table, version}`.
//! The evidence is the record's own raw blob, which `admit_record` already stores, so there is
//! no second artifact write and no separate evidence node: the record node carries the
//! `row_version` property, whose JSON shape is that of `medulla_modality::SourceRowVersion`
//! (`table_ref`, `row_ref`, `version`, `evidence_digest`). All references are opaque. The member
//! is consumed here and never mapped. Admission is O(1) per record plus one node read.

use medulla_types::contract::Digest256;
use medulla_ingestion_types::ingest_rules::{check_row_version, IngestRefusal, RowVersionDecision};

/// The payload member that carries a record's row version.
pub const ROW_VERSION_MEMBER: &str = "_row_version";
/// The record node property that holds the stored row version.
pub const ROW_VERSION_PROPERTY: &str = "row_version";
/// The record node property that names the evidence a newer version replaced.
pub const SUPERSEDED_DIGEST_PROPERTY: &str = "row_version_superseded_digest";

const TABLE_DOMAIN: &[u8] = b"eg/source-row-table/v1";
const TABLE_REF_PREFIX: &str = "eg:source-table:";
const ROW_REF_PREFIX: &str = "eg:source-row:";
const NODE_ID_PREFIX: &str = "source:";

/// The decoded `_row_version` member of one record.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowVersionMember {
    pub table: String,
    /// Kept as sent so a non-integer version is refused by `check_row_version`.
    pub version: serde_json::Value,
}

impl RowVersionMember {
    /// The member of a record payload, or `None` when the record carries none.
    pub fn of_payload(
        payload: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<Self>, IngestRefusal> {
        payload
            .get(ROW_VERSION_MEMBER)
            .map(|member| {
                serde_json::from_value(member.clone())
                    .map_err(|error| IngestRefusal::RowVersionInvalid(error.to_string()))
            })
            .transpose()
    }
}

/// The row version a record node already holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldRowVersion {
    pub version: u64,
    pub evidence_digest: String,
}

impl HeldRowVersion {
    /// The held row version of a record node's properties, or `None` when it holds none.
    pub fn of_properties(properties: &serde_json::Value) -> Option<Self> {
        let held = properties.get(ROW_VERSION_PROPERTY)?;
        Some(Self {
            version: held.get("version")?.as_u64()?,
            evidence_digest: held.get("evidence_digest")?.as_str()?.to_string(),
        })
    }
}

/// The identity a row version is scoped to.
#[derive(Clone, Copy, Debug)]
pub struct RowVersionIds<'a> {
    pub tenant: &'a str,
    pub connector: &'a str,
    pub stream: &'a str,
    /// The `source_node_id` of the record.
    pub node_id: &'a str,
}

/// What to write on the record node.
#[derive(Clone, Debug, PartialEq)]
pub struct RowVersionWrite {
    /// The value of the `row_version` property.
    pub row_version: serde_json::Value,
    /// The evidence a newer version replaced: the value of `row_version_superseded_digest`.
    pub superseded_digest: Option<String>,
}

/// How a record's row version relates to the held one.
#[derive(Clone, Debug, PartialEq)]
pub enum RowVersionOutcome {
    /// No version is held: store the evidence.
    First(RowVersionWrite),
    /// The version is newer than the held one: store the evidence and name the old digest.
    Supersedes(RowVersionWrite),
    /// The version is the held one: the record writes nothing.
    Replay,
}

fn table_ref(ids: &RowVersionIds<'_>, table: &str) -> Result<String, IngestRefusal> {
    let digest = Digest256::framed(
        TABLE_DOMAIN,
        &[
            ids.tenant.as_bytes(),
            ids.connector.as_bytes(),
            ids.stream.as_bytes(),
            table.as_bytes(),
        ],
    )
    .map_err(IngestRefusal::RowVersionInvalid)?;
    Ok(format!("{TABLE_REF_PREFIX}{}", digest.to_hex()))
}

fn row_ref(ids: &RowVersionIds<'_>) -> String {
    let token = ids
        .node_id
        .strip_prefix(NODE_ID_PREFIX)
        .unwrap_or(ids.node_id);
    format!("{ROW_REF_PREFIX}{token}")
}

/// Admit the row version of one record against the held one. `raw_digest_hex` is the digest of
/// the record's raw blob, which is the evidence. A table that is blank, a version that is not a
/// non-negative integer and a version below the held one are refused with `ROW_VERSION_INVALID`
/// and store no evidence.
pub fn admit_row_version(
    held: Option<&HeldRowVersion>,
    member: &RowVersionMember,
    raw_digest_hex: &str,
    ids: &RowVersionIds<'_>,
) -> Result<RowVersionOutcome, IngestRefusal> {
    if member.table.trim().is_empty() {
        return Err(IngestRefusal::RowVersionInvalid("table is empty".into()));
    }
    let decision = check_row_version(held.map(|held| held.version), &member.version)?;
    if decision == RowVersionDecision::Replay {
        return Ok(RowVersionOutcome::Replay);
    }
    let version = member
        .version
        .as_u64()
        .ok_or_else(|| IngestRefusal::RowVersionInvalid("version is not an integer".into()))?;
    let write = RowVersionWrite {
        row_version: serde_json::json!({
            "table_ref": table_ref(ids, &member.table)?,
            "row_ref": row_ref(ids),
            "version": version,
            "evidence_digest": raw_digest_hex,
        }),
        superseded_digest: match (decision, held) {
            (RowVersionDecision::Supersedes, Some(held)) => Some(held.evidence_digest.clone()),
            _ => None,
        },
    };
    Ok(match decision {
        RowVersionDecision::First => RowVersionOutcome::First(write),
        _ => RowVersionOutcome::Supersedes(write),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE: &str = "source:00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn ids() -> RowVersionIds<'static> {
        RowVersionIds {
            tenant: "t1",
            connector: "db",
            stream: "orders",
            node_id: NODE,
        }
    }

    fn member(version: serde_json::Value) -> RowVersionMember {
        RowVersionMember {
            table: "orders".into(),
            version,
        }
    }

    /// The `OpaqueRef` shape: `eg:<namespace>:<16 to 128 lowercase hex>`.
    fn assert_opaque_ref(value: &serde_json::Value, namespace: &str) {
        let text = value.as_str().expect("a reference is text");
        let parts: Vec<&str> = text.split(':').collect();
        assert_eq!(parts.len(), 3, "{text}");
        assert_eq!((parts[0], parts[1]), ("eg", namespace), "{text}");
        assert!((16..=128).contains(&parts[2].len()), "{text}");
        assert!(
            parts[2]
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f')),
            "{text}"
        );
    }

    fn written(outcome: RowVersionOutcome) -> RowVersionWrite {
        match outcome {
            RowVersionOutcome::First(write) | RowVersionOutcome::Supersedes(write) => write,
            RowVersionOutcome::Replay => panic!("a replay writes nothing"),
        }
    }

    fn held_of(write: &RowVersionWrite) -> HeldRowVersion {
        HeldRowVersion::of_properties(&serde_json::json!({
            ROW_VERSION_PROPERTY: write.row_version.clone(),
        }))
        .expect("the stored property reads back")
    }

    // spec: MEDULLA-INGEST-WORKER-R901.2
    #[test]
    fn first_newer_and_replayed_versions() {
        let digest_one = "11".repeat(32);
        let digest_two = "22".repeat(32);

        // Version 1 is the first: it stores its evidence and supersedes nothing.
        let first = written(
            admit_row_version(None, &member(serde_json::json!(1)), &digest_one, &ids()).unwrap(),
        );
        assert_eq!(first.row_version["version"], 1);
        assert_eq!(first.row_version["evidence_digest"], digest_one);
        assert_eq!(first.superseded_digest, None);
        assert_opaque_ref(&first.row_version["table_ref"], "source-table");
        assert_opaque_ref(&first.row_version["row_ref"], "source-row");
        assert_eq!(
            first.row_version["row_ref"],
            NODE.replace("source:", "eg:source-row:")
        );
        let held = held_of(&first);
        assert_eq!(held.version, 1);
        assert_eq!(held.evidence_digest, digest_one);

        // Version 2 supersedes it and names the replaced evidence.
        let outcome = admit_row_version(
            Some(&held),
            &member(serde_json::json!(2)),
            &digest_two,
            &ids(),
        )
        .unwrap();
        assert!(matches!(outcome, RowVersionOutcome::Supersedes(_)));
        let second = written(outcome);
        assert_eq!(second.row_version["version"], 2);
        assert_eq!(second.superseded_digest.as_deref(), Some(digest_one.as_str()));
        assert_eq!(
            first.row_version["table_ref"],
            second.row_version["table_ref"]
        );
        let held = held_of(&second);

        // Version 2 again is a replay: nothing is written for the record.
        assert_eq!(
            admit_row_version(
                Some(&held),
                &member(serde_json::json!(2)),
                &digest_two,
                &ids()
            )
            .unwrap(),
            RowVersionOutcome::Replay
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R901.2
    #[test]
    fn invalid_versions_and_tables_are_refused() {
        let held = HeldRowVersion {
            version: 5,
            evidence_digest: "33".repeat(32),
        };
        let digest = "44".repeat(32);
        for version in [
            serde_json::json!(4),
            serde_json::json!("7"),
            serde_json::json!(1.5),
            serde_json::json!(-1),
        ] {
            let refused = admit_row_version(Some(&held), &member(version), &digest, &ids())
                .expect_err("an invalid version is refused");
            assert_eq!(refused.code(), "ROW_VERSION_INVALID");
        }
        let blank = RowVersionMember {
            table: " ".into(),
            version: serde_json::json!(6),
        };
        assert!(admit_row_version(Some(&held), &blank, &digest, &ids()).is_err());
        // A record without the member is unchanged.
        assert_eq!(
            RowVersionMember::of_payload(&serde_json::Map::new()).unwrap(),
            None
        );
    }
}
