//! Source permission and group sync (MEDULLA-INGEST-WORKER-R902.2).
//!
//! `access` batches refresh the access of records the engine already holds;
//! `groups` batches replace the member set of one source group. Both produce
//! methods only: an access update is a `CompareAndSetNodeFields` on the access
//! properties and never touches content, vectors or edges. Refusals are values
//! returned per item, so a refused item writes nothing.

use std::collections::{BTreeMap, BTreeSet};

use medulla_protocol_types::protocol::Method;
use medulla_types::contract::Digest256;
use medulla_types::ingest_rules::{check_access_target, check_group_size, IngestRefusal};
use medulla_types::source_governance::{AccessSnapshot, SourceGroup};

/// Node property: the version of the last applied access sync.
pub const ACL_VERSION_PROPERTY: &str = "source_acl_version";
/// Node property: when the access was last synced, in milliseconds.
pub const ACL_SYNCED_PROPERTY: &str = "acl_synced_at_ms";
/// Prefix that marks a group reference inside `source_access_read_roles`.
pub const GROUP_ROLE_PREFIX: &str = "group:";

const SUBJECT_DOMAIN: &[u8] = b"eg/source-record-subjects";

/// What the engine holds about one record's access.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldRecord {
    pub acl_version: Option<u64>,
    pub acl_synced_at_ms: Option<u64>,
    /// Markings stay governance-owned; they feed the subject digest unchanged.
    pub markings: BTreeSet<String>,
}

/// One accepted access update.
#[derive(Debug, Clone)]
pub struct AccessUpdate {
    pub record_ref: String,
    pub node_id: String,
    pub method: Method,
    /// The replacement `PolicyRecord.subject_set_digest`.
    pub subject_set_digest: String,
}

/// The outcome of one `access` batch.
#[derive(Debug, Default)]
pub struct AccessAdmission {
    pub updates: Vec<AccessUpdate>,
    /// Refused items by record reference; they wrote nothing.
    pub refused: Vec<(String, IngestRefusal)>,
    /// Records whose previous sync was already older than the bound.
    pub stale: u64,
}

/// One accepted group replacement.
#[derive(Debug, Clone)]
pub struct GroupUpdate {
    pub group_id: String,
    pub node_id: String,
    pub method: Method,
}

/// The outcome of one `groups` batch.
#[derive(Debug, Default)]
pub struct GroupAdmission {
    pub updates: Vec<GroupUpdate>,
    pub refused: Vec<(String, IngestRefusal)>,
}

/// A record whose last sync is older than the bound is `acl_stale`.
pub fn acl_stale(acl_synced_at_ms: Option<u64>, now_ms: u64, stale_after_s: u64) -> bool {
    acl_synced_at_ms
        .is_some_and(|synced| now_ms.saturating_sub(synced) > stale_after_s.saturating_mul(1_000))
}

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| format!("access update encoding failed: {error}"))
}

/// Admit one `access` batch against the records the engine holds, keyed by
/// record reference. `node_id_of` is the batch's `source_node_id`.
pub fn admit_access(
    snapshots: &[AccessSnapshot],
    held: &BTreeMap<String, HeldRecord>,
    node_id_of: impl Fn(&AccessSnapshot) -> Result<String, String>,
    now_ms: u64,
    stale_after_s: u64,
) -> Result<AccessAdmission, String> {
    let held_refs: BTreeSet<String> = held.keys().cloned().collect();
    let mut admission = AccessAdmission::default();
    for snapshot in snapshots {
        if let Err(refusal) = check_access_target(&held_refs, &snapshot.record_ref) {
            admission
                .refused
                .push((snapshot.record_ref.clone(), refusal));
            continue;
        }
        let record = &held[&snapshot.record_ref];
        if acl_stale(record.acl_synced_at_ms, now_ms, stale_after_s) {
            admission.stale += 1;
        }
        let mut roles: BTreeSet<String> = snapshot.read_principals.clone();
        roles.extend(
            snapshot
                .read_groups
                .iter()
                .map(|group| format!("{GROUP_ROLE_PREFIX}{group}")),
        );
        let roles_bytes = rmp_serde::to_vec(&roles).map_err(|error| error.to_string())?;
        let markings_bytes =
            rmp_serde::to_vec(&record.markings).map_err(|error| error.to_string())?;
        let subject_set_digest = Digest256::framed(
            SUBJECT_DOMAIN,
            &[
                &[u8::from(snapshot.access_public)][..],
                &roles_bytes[..],
                &markings_bytes[..],
            ],
        )?
        .to_hex();
        let node_id = node_id_of(snapshot)?;
        let conditions = serde_json::json!({ ACL_VERSION_PROPERTY: record.acl_version });
        let updates = serde_json::json!({
            "source_access_public": snapshot.access_public,
            "source_access_read_roles": roles,
            ACL_SYNCED_PROPERTY: snapshot.synced_at,
            ACL_VERSION_PROPERTY: record.acl_version.unwrap_or(0) + 1,
        });
        admission.updates.push(AccessUpdate {
            record_ref: snapshot.record_ref.clone(),
            method: Method::CompareAndSetNodeFields {
                node_id: node_id.clone(),
                conditions_msgpack: encode(&conditions)?,
                updates_msgpack: encode(&updates)?,
            },
            node_id,
            subject_set_digest,
        });
    }
    Ok(admission)
}

/// Admit one `groups` batch. `node_id_of` is the digest of tenant, connector,
/// source and group id under the `source_group:` prefix.
pub fn admit_groups(
    groups: &[SourceGroup],
    node_id_of: impl Fn(&SourceGroup) -> Result<String, String>,
) -> Result<GroupAdmission, String> {
    let mut admission = GroupAdmission::default();
    for group in groups {
        if let Err(refusal) = check_group_size(group) {
            admission.refused.push((group.group_id.clone(), refusal));
            continue;
        }
        let node_id = node_id_of(group)?;
        let properties = serde_json::json!({
            "type": "SourceGroup",
            "source": group.source,
            "group_id": group.group_id,
            "members": group.members,
            "synced_at": group.synced_at,
        });
        admission.updates.push(GroupUpdate {
            group_id: group.group_id.clone(),
            method: Method::AddNode {
                node_id: node_id.clone(),
                properties_msgpack: encode(&properties)?,
            },
            node_id,
        });
    }
    Ok(admission)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::ingest_rules::MAX_GROUP_MEMBERS;
    use serde_json::{json, Map, Value};

    fn snapshot(record_ref: &str, public: bool) -> AccessSnapshot {
        AccessSnapshot {
            source: "wiki".into(),
            record_ref: record_ref.into(),
            access_public: public,
            read_principals: ["alice".to_string()].into(),
            read_groups: ["eng".to_string()].into(),
            synced_at: 5_000_000,
        }
    }

    fn node_id(snapshot: &AccessSnapshot) -> Result<String, String> {
        Ok(format!("source:{}", snapshot.record_ref))
    }

    fn apply(props: &mut Map<String, Value>, method: &Method) {
        let Method::CompareAndSetNodeFields {
            conditions_msgpack,
            updates_msgpack,
            ..
        } = method
        else {
            panic!("an access update is a CompareAndSetNodeFields, got {method:?}");
        };
        let conditions: Map<String, Value> = rmp_serde::from_slice(conditions_msgpack).unwrap();
        let updates: Map<String, Value> = rmp_serde::from_slice(updates_msgpack).unwrap();
        let matches = conditions
            .iter()
            .all(|(key, expected)| props.get(key).unwrap_or(&Value::Null) == expected);
        assert!(matches, "the condition names the current access version");
        props.extend(updates);
    }

    fn content_digest(props: &Map<String, Value>) -> Vec<u8> {
        let content: Map<String, Value> = props
            .iter()
            .filter(|(key, _)| {
                !key.starts_with("source_access_")
                    && key.as_str() != ACL_SYNCED_PROPERTY
                    && key.as_str() != ACL_VERSION_PROPERTY
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        rmp_serde::to_vec_named(&content).unwrap()
    }

    // spec: MEDULLA-INGEST-WORKER-R902.2
    #[test]
    fn access_batch_leaves_content_unchanged() {
        let mut props = json!({
            "type": "Document", "name": "runbook", "source_raw_sha256": "ab",
            "source_access_public": true, "source_access_read_roles": [],
            "source_access_markings": ["pii"]
        })
        .as_object()
        .unwrap()
        .clone();
        let before = content_digest(&props);
        let held = BTreeMap::from([(
            "r1".to_string(),
            HeldRecord {
                markings: ["pii".to_string()].into(),
                ..HeldRecord::default()
            },
        )]);
        let admission =
            admit_access(&[snapshot("r1", false)], &held, node_id, 6_000_000, 3_600).unwrap();
        assert_eq!(admission.updates.len(), 1);
        assert!(admission.refused.is_empty() && admission.stale == 0);
        apply(&mut props, &admission.updates[0].method);
        assert_eq!(content_digest(&props), before);
        assert_eq!(props["source_access_public"], json!(false));
        assert_eq!(
            props["source_access_read_roles"],
            json!(["alice", "group:eng"])
        );
        assert_eq!(props["source_access_markings"], json!(["pii"]));
        assert_eq!(props[ACL_VERSION_PROPERTY], json!(1));
        assert_eq!(props[ACL_SYNCED_PROPERTY], json!(5_000_000));

        // A second sync advances the version from the held one.
        let held = BTreeMap::from([(
            "r1".to_string(),
            HeldRecord {
                acl_version: Some(1),
                acl_synced_at_ms: Some(5_000_000),
                markings: ["pii".to_string()].into(),
            },
        )]);
        let second =
            admit_access(&[snapshot("r1", true)], &held, node_id, 6_000_000, 3_600).unwrap();
        apply(&mut props, &second.updates[0].method);
        assert_eq!(props[ACL_VERSION_PROPERTY], json!(2));
        assert_eq!(content_digest(&props), before);
    }

    #[test]
    fn unknown_record_is_refused_and_stale_is_counted() {
        let held = BTreeMap::from([(
            "r1".to_string(),
            HeldRecord {
                acl_version: Some(3),
                acl_synced_at_ms: Some(1_000),
                markings: BTreeSet::new(),
            },
        )]);
        let admission = admit_access(
            &[snapshot("r1", false), snapshot("ghost", false)],
            &held,
            node_id,
            10_000_000,
            3_600,
        )
        .unwrap();
        assert_eq!(admission.updates.len(), 1);
        assert_eq!(admission.stale, 1);
        assert_eq!(admission.refused.len(), 1);
        assert_eq!(admission.refused[0].1.code(), "SOURCE_RECORD_UNKNOWN");
    }

    #[test]
    fn group_batch_replaces_the_member_set_and_refuses_oversized_groups() {
        let small = SourceGroup {
            source: "wiki".into(),
            group_id: "eng".into(),
            members: ["alice".to_string(), "bob".to_string()].into(),
            synced_at: 1,
        };
        let huge = SourceGroup {
            group_id: "everyone".into(),
            members: (0..=MAX_GROUP_MEMBERS).map(|i| format!("u{i}")).collect(),
            ..small.clone()
        };
        let admission = admit_groups(&[small, huge], |group| {
            Ok(format!("source_group:{}", group.group_id))
        })
        .unwrap();
        assert_eq!(admission.updates.len(), 1);
        assert_eq!(admission.refused[0].1.code(), "SOURCE_GROUP_TOO_LARGE");
        let Method::AddNode {
            node_id,
            properties_msgpack,
        } = &admission.updates[0].method
        else {
            panic!("a group replacement is one AddNode");
        };
        assert_eq!(node_id, "source_group:eng");
        let props: Map<String, Value> = rmp_serde::from_slice(properties_msgpack).unwrap();
        assert_eq!(props["members"], json!(["alice", "bob"]));
    }
}
