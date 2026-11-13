//! Record governance admission (MEDULLA-INGEST-WORKER-R900.2).
//!
//! A source record may carry the payload member `_governance`. This module
//! parses it into [`RecordGovernance`], runs the refusal rules, and returns the
//! policy values and node access properties the ingestion path attaches to the
//! record. The member is consumed here and never mapped. Parsing is
//! O(properties) and takes no lock.

use medulla_types::contract::Digest256;
use medulla_types::ingest_rules::IngestRefusal;
use medulla_types::source_governance::{Classification, GovernanceReport, RecordGovernance};

/// The payload member that carries a record's governance.
pub const GOVERNANCE_MEMBER: &str = "_governance";
/// Retention label of a record that sent no retention period.
pub const DEFAULT_RETENTION_POLICY: &str = "source-ingestion";

const SUBJECT_DOMAIN: &[u8] = b"eg/source-record-subjects";

/// What governance contributes to one admitted record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedGovernance {
    /// Snake-case classification name for the `PolicyRecord`.
    pub classification: &'static str,
    /// `retention-<seconds>s` or [`DEFAULT_RETENTION_POLICY`].
    pub retention_policy: String,
    pub legal_hold: bool,
    /// SHA-256 over the public flag, sorted roles and sorted markings.
    pub subject_set_digest: String,
    /// Reserved node properties `source_access_*`.
    pub properties: [(&'static str, serde_json::Value); 4],
    /// The record carries at least one marking.
    pub marked: bool,
}

fn classification_name(classification: Classification) -> &'static str {
    match classification {
        Classification::Public => "public",
        Classification::Internal => "internal",
        Classification::Confidential => "confidential",
        Classification::Restricted => "restricted",
    }
}

/// Parse and admit the `_governance` member of one record payload.
///
/// `Ok(None)` means the record sent no governance and takes the default policy.
/// A refusal is a value: nothing has been written when it is returned.
pub fn admit_record_governance(
    payload: &serde_json::Map<String, serde_json::Value>,
    retention_required: bool,
) -> Result<Option<AdmittedGovernance>, IngestRefusal> {
    let Some(member) = payload.get(GOVERNANCE_MEMBER) else {
        return Ok(None);
    };
    let governance: RecordGovernance = serde_json::from_value(member.clone())
        .map_err(|error| IngestRefusal::GovernanceInvalid(error.to_string()))?;
    governance.admit(retention_required)?;
    let roles = rmp_serde::to_vec(&governance.access_read_roles)
        .map_err(|error| IngestRefusal::GovernanceInvalid(error.to_string()))?;
    let markings = rmp_serde::to_vec(&governance.access_markings)
        .map_err(|error| IngestRefusal::GovernanceInvalid(error.to_string()))?;
    let subject_set_digest = Digest256::framed(
        SUBJECT_DOMAIN,
        &[
            &[u8::from(governance.access_public)][..],
            &roles[..],
            &markings[..],
        ],
    )
    .map_err(IngestRefusal::GovernanceInvalid)?
    .to_hex();
    let marked = !governance.access_markings.is_empty();
    Ok(Some(AdmittedGovernance {
        classification: classification_name(governance.classification),
        retention_policy: governance.retention.map_or_else(
            || DEFAULT_RETENTION_POLICY.into(),
            |s| format!("retention-{s}s"),
        ),
        legal_hold: governance.legal_hold,
        subject_set_digest,
        properties: [
            (
                "source_access_public",
                serde_json::Value::Bool(governance.access_public),
            ),
            (
                "source_access_read_roles",
                serde_json::json!(governance.access_read_roles),
            ),
            (
                "source_access_markings",
                serde_json::json!(governance.access_markings),
            ),
            (
                "source_classification",
                serde_json::Value::String(classification_name(governance.classification).into()),
            ),
        ],
        marked,
    }))
}

/// Fold one record's outcome into the batch report.
pub fn count_governance(
    report: &mut GovernanceReport,
    outcome: &Result<Option<AdmittedGovernance>, IngestRefusal>,
) {
    match outcome {
        Ok(Some(admitted)) => {
            report.governed += 1;
            report.marked += u64::from(admitted.marked);
        }
        Ok(None) => {}
        Err(_) => {
            report.governed += 1;
            report.refused += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(governance: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        json!({"id": 1, "_governance": governance})
            .as_object()
            .unwrap()
            .clone()
    }

    // spec: MEDULLA-INGEST-WORKER-R900.2
    #[test]
    fn public_restricted_marked_and_hold_records() {
        let public = admit_record_governance(
            &payload(json!({"classification": "public", "access_public": true})),
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(public.classification, "public");
        assert_eq!(public.retention_policy, DEFAULT_RETENTION_POLICY);
        assert!(!public.legal_hold && !public.marked);
        assert_eq!(public.properties[0].1, json!(true));

        let restricted = admit_record_governance(
            &payload(json!({
                "classification": "restricted", "access_public": false,
                "access_read_roles": ["b", "a"], "access_markings": ["pii"],
                "retention": 86400, "legal_hold": true
            })),
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(restricted.classification, "restricted");
        assert_eq!(restricted.retention_policy, "retention-86400s");
        assert!(restricted.legal_hold && restricted.marked);
        assert_eq!(restricted.properties[1].1, json!(["a", "b"]));
        assert_eq!(restricted.properties[2].1, json!(["pii"]));
        assert_ne!(public.subject_set_digest, restricted.subject_set_digest);

        // Role order never changes the subject digest.
        let reordered = admit_record_governance(
            &payload(json!({
                "classification": "restricted", "access_public": false,
                "access_read_roles": ["a", "b"], "access_markings": ["pii"]
            })),
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(reordered.subject_set_digest, restricted.subject_set_digest);

        // No member: the default policy, nothing counted.
        let plain = json!({"id": 1}).as_object().unwrap().clone();
        assert_eq!(admit_record_governance(&plain, true).unwrap(), None);
    }

    // spec: MEDULLA-INGEST-WORKER-R900.5
    #[test]
    fn counts_governed_refused_and_marked() {
        let mut report = GovernanceReport::default();
        for (member, required) in [
            (
                json!({"classification": "internal", "access_public": false, "access_markings": ["x"]}),
                false,
            ),
            (
                json!({"classification": "public", "access_public": false}),
                false,
            ),
            (
                json!({"classification": "internal", "access_public": false}),
                true,
            ),
            (
                json!({"classification": "internal", "access_public": false, "bogus": 1}),
                false,
            ),
        ] {
            count_governance(
                &mut report,
                &admit_record_governance(&payload(member), required),
            );
        }
        count_governance(&mut report, &Ok(None));
        assert_eq!(
            report,
            GovernanceReport {
                governed: 4,
                refused: 3,
                marked: 1
            }
        );
    }

    #[test]
    fn refusals_carry_their_codes() {
        let conflict = admit_record_governance(
            &payload(json!({"classification": "restricted", "access_public": true})),
            false,
        )
        .unwrap_err();
        assert_eq!(conflict.code(), "SOURCE_GOVERNANCE_CONFLICT");
        let missing = admit_record_governance(
            &payload(json!({"classification": "internal", "access_public": false})),
            true,
        )
        .unwrap_err();
        assert_eq!(missing.code(), "SOURCE_RETENTION_MISSING");
        let invalid = admit_record_governance(&payload(json!({"classification": "secret"})), false)
            .unwrap_err();
        assert_eq!(invalid.code(), "SOURCE_GOVERNANCE_INVALID");
    }
}
