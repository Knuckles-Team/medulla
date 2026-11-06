use medulla_catalog_types::graph_schema::{DiffTarget, GraphSchemaOp};
use medulla_types::contract::ResourceId;
use medulla_types::ontology_diff::{ChangeImpact, ImpactItems, OntologyDiff};

/// Verifies MEDULLA-TYPED-PACKS-R804.1: Model.
// spec: MEDULLA-TYPED-PACKS-R804.1
#[test]
fn diff_round_trips() {
    let diff = OntologyDiff {
        from_rev: "urn:example:revision:1".into(),
        to_rev: "urn:example:revision:2".into(),
        added: vec!["class:Invoice".into()],
        removed: vec!["property:legacyCode".into()],
        changed: vec!["property:total.range".into()],
        deprecated: vec!["class:OldInvoice".into()],
    };
    let impact = ChangeImpact {
        instances_invalid: 2,
        shapes_broken: 1,
        mappings_broken: 3,
        queries_broken: 4,
    };

    assert_eq!(
        serde_json::from_str::<OntologyDiff>(&serde_json::to_string(&diff).unwrap()).unwrap(),
        diff
    );
    assert_eq!(
        serde_json::from_str::<ChangeImpact>(&serde_json::to_string(&impact).unwrap()).unwrap(),
        impact
    );

    let items = ImpactItems {
        instances: vec!["urn:example:n1".into()],
        shapes: vec!["urn:example:shape".into()],
        mappings: Vec::new(),
        queries: vec!["q1".into(), "q2".into()],
    };
    assert_eq!(
        serde_json::from_str::<ImpactItems>(&serde_json::to_string(&items).unwrap()).unwrap(),
        items
    );

    for to in [
        DiffTarget::Revision(3),
        DiffTarget::Candidate {
            turtle: "<urn:a> <urn:b> <urn:c> .".into(),
        },
    ] {
        let op = GraphSchemaOp::DiffOntology {
            connector: ResourceId::new("sample").unwrap(),
            ontology_iri: "urn:example:onto".into(),
            from_revision: 2,
            to,
        };
        let text = serde_json::to_string(&op).unwrap();
        assert!(text.contains("\"diff_ontology\""));
        assert_eq!(serde_json::from_str::<GraphSchemaOp>(&text).unwrap(), op);
    }
}
