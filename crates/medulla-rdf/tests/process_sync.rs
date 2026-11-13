//! Process engine data location (MEDULLA-CQ-ORCH-R703.1): the fixture authority rows pass the
//! virtual-graph shapes and a row without its materialisation reason is refused.
#![cfg(feature = "rdf")]

const SHAPES: &str = include_str!("../../medulla-core/ontology/virtual_graph-v1.shapes.ttl");
const AUTHORITY: &str =
    include_str!("../../medulla-core/tests/fixtures/process_sync/authority.ttl");
const REASON: &str =
    "    vg:materialisationReason \"Graph-held models are the system of record.\" .\n";

// spec: MEDULLA-CQ-ORCH-R703.1
#[test]
fn engine_location_row_passes_shapes() {
    let report = medulla_shacl::validate_turtle(SHAPES, AUTHORITY).expect("validates");
    assert!(report.conforms, "{:?}", report.results);
    // The two rows declare the roles the row sets.
    assert_eq!(AUTHORITY.matches("a vg:SourceAuthority").count(), 2);
    for term in [
        "vg:recordRole \"federated_secondary\"",
        "vg:syncDirection \"both\"",
        "vg:conflictRule \"system_of_record_wins\"",
        "vg:materialisation \"accelerated\"",
    ] {
        assert_eq!(AUTHORITY.matches(term).count(), 2, "{term}");
    }

    // Without a reason the shapes refuse the accelerated row.
    assert_eq!(AUTHORITY.matches(REASON).count(), 2);
    let without = AUTHORITY.replacen(REASON, "    vg:materialisationNote \"none\" .\n", 1);
    let refused = medulla_shacl::validate_turtle(SHAPES, &without).expect("validates");
    assert!(!refused.conforms);
    assert!(
        refused.results.iter().any(|r| r
            .message
            .as_deref()
            .is_some_and(|m| m.contains("needs a materialisationReason"))
            && r.focus_node.contains("process-model-location")),
        "{:?}",
        refused.results
    );
}
