//! Instance profiles reach the one proposal admission path
//! (MEDULLA-INGEST-WORKER-R002.8, R002.9).
#![cfg(all(feature = "redb", feature = "blob"))]

use medulla_h_ingest::source_ingestion::proposals::{admit_proposals, decode_proposals};
use medulla_ingestion_types::ontology_proposal::ProposalThresholds;
use medulla_source_types::source_ingestion::SourceJson;
use medulla_types::ontology_proposal_machine::ProposalState;

fn item(value: serde_json::Value) -> SourceJson {
    SourceJson::new(value).unwrap()
}

fn thresholds() -> ProposalThresholds {
    ProposalThresholds {
        min_coverage: 0.8,
        max_redundancy: 0.1,
    }
}

// spec: MEDULLA-INGEST-WORKER-R002.9
/// Verifies MEDULLA-INGEST-WORKER-R002.9: a profile item decodes into the same submission as a
/// model's proposal and is judged by `admit_proposals` (so by `judge_proposal`'s checks); the two
/// profile refusals name their codes and leave the batch undecoded.
#[test]
fn profile_refusals_and_admission_wiring() {
    let passing = item(serde_json::json!({
        "proposal_id": "from-profile",
        "profile": {
            "classes": [{"iri": "Thing", "instances": 5}, {"iri": "Owner", "instances": 3}],
            "datatype_properties": [
                {"iri": "name", "class": "Thing", "datatype": "xsd:string", "presence": 1.0},
            ],
            "object_properties": [
                {"iri": "ownedBy", "domain": "Thing", "range": "Owner", "usage": 3},
            ],
        },
    }));
    let thin = item(serde_json::json!({
        "proposal_id": "thin-profile",
        "profile": {
            "classes": [{"iri": "Thing", "instances": 5}],
            "datatype_properties": [
                {"iri": "name", "class": "Thing", "datatype": "xsd:string", "presence": 1.0},
                {"iri": "rare", "class": "Thing", "datatype": "xsd:string", "presence": 0.1},
                {"iri": "rarer", "class": "Thing", "datatype": "xsd:string", "presence": 0.0},
            ],
        },
    }));
    let decoded = decode_proposals(&[passing, thin]).unwrap();
    let admission = admit_proposals(&decoded, &thresholds()).unwrap();
    assert_eq!(
        admission.node_ids,
        [
            "ontology_proposal:from-profile",
            "ontology_proposal:thin-profile"
        ]
    );
    assert_eq!(admission.items[0].state, ProposalState::Admitted);
    assert_eq!(admission.items[1].state, ProposalState::Draft);
    assert_eq!(
        admission.items[1].refusal.as_deref(),
        Some("PROPOSAL_NOT_ADMITTED")
    );
    assert_eq!(
        admission.items[1].failing_check.as_deref(),
        Some("coverage")
    );

    let dangling = item(serde_json::json!({
        "proposal_id": "dangling",
        "profile": {
            "classes": [{"iri": "Thing", "instances": 5}],
            "object_properties": [
                {"iri": "ownedBy", "domain": "Thing", "range": "Ghost", "usage": 3},
            ],
        },
    }));
    assert!(decode_proposals(&[dangling])
        .unwrap_err()
        .starts_with("PROFILE_DANGLING_CLASS"));
    let classes: Vec<serde_json::Value> = (0..10_001)
        .map(|n| serde_json::json!({"iri": format!("C{n}"), "instances": 2}))
        .collect();
    let big = item(serde_json::json!({"proposal_id": "big", "profile": {"classes": classes}}));
    assert!(decode_proposals(&[big])
        .unwrap_err()
        .starts_with("PROFILE_TOO_LARGE"));
}
