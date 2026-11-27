//! MEDULLA-TYPED-PACKS-R806: the decisions of the served schema drift methods.
//!
//! The router hands this module the quarantine rows the caller can see (each a decoded
//! `SourceQuarantine` node) and reads back typed answers. Nothing here touches the graph:
//! list and get shape the stored drift evidence, and propose and approve derive the repair
//! candidate from it and check the digest the approval binds. The write that activates a
//! candidate is the router's, through the one `GraphSchema` gateway path.

use serde_json::{Map, Value};

use medulla_catalog_types::graph_schema::approval::APPROVED_SOURCE_PREFIX;
use medulla_catalog_types::graph_schema::drift::RepairProposal;
use medulla_source_types::schema_drift_methods::{
    DriftEvidence, DriftEvidenceRecord, DriftRecordView, DriftStreamView, SchemaDriftListAnswer,
    SchemaRepairTarget, DRIFT_EVIDENCE_PROPERTY, DRIFT_NOT_FOUND, MAX_DRIFT_LIST,
    REPAIR_DIGEST_MISMATCH,
};
use medulla_types::contract::ResourceId;

/// One quarantine row: its node id and decoded properties.
pub type QuarantineRow = (String, Map<String, Value>);

/// The drift evidence of a quarantine row of `tenant`, when it is a drift quarantine. A row of
/// another tenant, or one without evidence, is not a drift.
fn evidence_of(row: &QuarantineRow, tenant: &str) -> Option<DriftEvidenceRecord> {
    let (_, node) = row;
    if node.get("tenant_id").and_then(Value::as_str) != Some(tenant) {
        return None;
    }
    let evidence: DriftEvidenceRecord =
        serde_json::from_value(node.get(DRIFT_EVIDENCE_PROPERTY)?.clone()).ok()?;
    (!evidence.streams.is_empty()).then_some(evidence)
}

fn view_of(row: &QuarantineRow, evidence: DriftEvidenceRecord) -> DriftRecordView {
    DriftRecordView {
        drift_id: row.0.clone(),
        connector: evidence.connector,
        source_digest: row
            .1
            .get("source_digest")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        streams: evidence
            .streams
            .into_iter()
            .map(|drift| DriftStreamView {
                stream: drift.stream,
                report: drift.report,
            })
            .collect(),
    }
}

/// The drifts of `tenant`, ordered by id, optionally of one stream. `SchemaDriftList`.
pub fn list(
    rows: &[QuarantineRow],
    tenant: &str,
    stream: Option<&ResourceId>,
) -> SchemaDriftListAnswer {
    let mut drifts: Vec<DriftRecordView> = rows
        .iter()
        .filter_map(|row| Some((row, evidence_of(row, tenant)?)))
        .map(|(row, evidence)| view_of(row, evidence))
        .filter(|view| {
            stream.is_none_or(|wanted| view.streams.iter().any(|drift| &drift.stream == wanted))
        })
        .collect();
    drifts.sort_by(|left, right| left.drift_id.cmp(&right.drift_id));
    drifts.truncate(MAX_DRIFT_LIST);
    SchemaDriftListAnswer { drifts }
}

fn not_found(drift_id: &str) -> String {
    format!("{DRIFT_NOT_FOUND}: {drift_id}")
}

/// One drift of `tenant`. Another tenant's drift and an unknown id refuse identically.
/// `SchemaDriftGet`.
pub fn get(
    rows: &[QuarantineRow],
    tenant: &str,
    drift_id: &str,
) -> Result<DriftRecordView, String> {
    rows.iter()
        .filter(|row| row.0 == drift_id)
        .find_map(|row| Some((row, evidence_of(row, tenant)?)))
        .map(|(row, evidence)| view_of(row, evidence))
        .ok_or_else(|| not_found(drift_id))
}

/// The stored evidence of one stream of one drift of `tenant`.
fn stream_evidence(
    rows: &[QuarantineRow],
    tenant: &str,
    target: &SchemaRepairTarget,
) -> Result<DriftEvidence, String> {
    rows.iter()
        .filter(|row| row.0 == target.drift_id)
        .find_map(|row| evidence_of(row, tenant))
        .and_then(|evidence| {
            evidence
                .streams
                .into_iter()
                .find(|drift| drift.stream == target.stream)
        })
        .ok_or_else(|| not_found(&target.drift_id))
}

/// The `approved:<stream>` key a stream's contract lives under.
pub fn source_id_of(stream: &ResourceId) -> String {
    format!("{APPROVED_SOURCE_PREFIX}{}", stream.as_str())
}

/// The repair candidate a drift derives: the inferred shape as the typed contract, the shapes
/// the engine renders for it and the digest an approval binds. Deterministic: the same
/// evidence always derives the same candidate and digest.
pub fn repair_proposal(
    rows: &[QuarantineRow],
    tenant: &str,
    target: &SchemaRepairTarget,
) -> Result<RepairProposal, String> {
    let evidence = stream_evidence(rows, tenant, target)?;
    RepairProposal::new(&source_id_of(&target.stream), &evidence.shape)
}

/// The proposal an approval activates: derived afresh from the stored evidence, then compared
/// with the digest the approval names. `REPAIR_DIGEST_MISMATCH` when they differ, so an
/// approval of one candidate never activates another.
pub fn approved_proposal(
    rows: &[QuarantineRow],
    tenant: &str,
    target: &SchemaRepairTarget,
    proposal_digest: &str,
) -> Result<RepairProposal, String> {
    let proposal = repair_proposal(rows, tenant, target)?;
    if proposal.candidate_digest != proposal_digest {
        return Err(format!(
            "{REPAIR_DIGEST_MISMATCH}: the approved digest is not the digest of the candidate \
             this drift derives"
        ));
    }
    Ok(proposal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::graph_schema::drift::{
        classify_delivery, EvolutionPolicy, InferredRecordShape,
    };
    use medulla_catalog_types::graph_schema::repair::RecordContract;

    fn contract() -> RecordContract {
        serde_json::from_value(serde_json::json!({
            "fields": {
                "id": {"required": true, "types": ["integer"]},
                "name": {"required": true, "types": ["string"]},
            }
        }))
        .unwrap()
    }

    fn row(id: &str, tenant: &str, stream: &str, record: Value) -> QuarantineRow {
        let approved = contract();
        let shape = InferredRecordShape::infer(&[record]).unwrap();
        let source_id = format!("approved:{stream}");
        let report = classify_delivery(&source_id, &approved, &shape, EvolutionPolicy::Quarantine);
        let evidence = DriftEvidenceRecord {
            connector: ResourceId::new("conn").unwrap(),
            streams: vec![DriftEvidence {
                stream: ResourceId::new(stream).unwrap(),
                report,
                shape,
            }],
        };
        let mut node = Map::new();
        node.insert("type".into(), Value::String("SourceQuarantine".into()));
        node.insert("tenant_id".into(), Value::String(tenant.into()));
        node.insert("source_digest".into(), Value::String("ab".repeat(32)));
        node.insert(
            DRIFT_EVIDENCE_PROPERTY.into(),
            serde_json::to_value(&evidence).unwrap(),
        );
        (id.to_owned(), node)
    }

    fn target(id: &str, stream: &str) -> SchemaRepairTarget {
        SchemaRepairTarget {
            drift_id: id.to_owned(),
            stream: ResourceId::new(stream).unwrap(),
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R806
    #[test]
    fn a_tenant_reads_only_its_own_drifts_and_an_unknown_id_refuses_alike() {
        let rows = vec![
            row("q1", "tenant-a", "items", serde_json::json!({"name": 7})),
            row("q2", "tenant-b", "items", serde_json::json!({"name": 7})),
        ];
        let listed = list(&rows, "tenant-a", None);
        assert_eq!(listed.drifts.len(), 1);
        assert_eq!(listed.drifts[0].drift_id, "q1");
        assert!(
            list(&rows, "tenant-a", Some(&ResourceId::new("other").unwrap()))
                .drifts
                .is_empty()
        );
        assert!(get(&rows, "tenant-a", "q1").is_ok());
        // Another tenant's drift and an unknown id answer the same body but for the id asked.
        let other = get(&rows, "tenant-a", "q2").unwrap_err();
        let unknown = get(&rows, "tenant-a", "q9").unwrap_err();
        assert_eq!(other.replace("q2", "q9"), unknown);
        assert!(other.starts_with(DRIFT_NOT_FOUND));
    }

    // spec: MEDULLA-TYPED-PACKS-R806
    #[test]
    fn the_candidate_is_derived_from_the_stored_shape_and_the_approval_must_bind_its_digest() {
        let rows = vec![row(
            "q1",
            "tenant-a",
            "items",
            serde_json::json!({"name": 7}),
        )];
        let proposal = repair_proposal(&rows, "tenant-a", &target("q1", "items")).unwrap();
        assert_eq!(proposal.source_id, "approved:items");
        assert!(proposal.candidate.fields.contains_key("name"));
        let again = repair_proposal(&rows, "tenant-a", &target("q1", "items")).unwrap();
        assert_eq!(again.candidate_digest, proposal.candidate_digest);
        assert!(approved_proposal(
            &rows,
            "tenant-a",
            &target("q1", "items"),
            &proposal.candidate_digest
        )
        .is_ok());
        let refused = approved_proposal(&rows, "tenant-a", &target("q1", "items"), &"0".repeat(64))
            .unwrap_err();
        assert!(refused.starts_with(REPAIR_DIGEST_MISMATCH), "{refused}");
        assert!(repair_proposal(&rows, "tenant-a", &target("q1", "missing"))
            .unwrap_err()
            .starts_with(DRIFT_NOT_FOUND));
        assert!(repair_proposal(&rows, "tenant-b", &target("q1", "items"))
            .unwrap_err()
            .starts_with(DRIFT_NOT_FOUND));
    }
}
