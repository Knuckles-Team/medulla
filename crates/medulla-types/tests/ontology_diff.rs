use medulla_catalog_types::graph_schema::{DiffTarget, GraphSchemaOp};
use medulla_types::contract::ResourceId;
use medulla_types::ontology_diff::{ChangeImpact, ImpactItems, OntologyDiff};

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
        instances: vec!["urn:example:a".into()],
        shapes: vec!["urn:example:shape".into()],
        mappings: vec![],
        queries: vec!["urn:example:q".into()],
    };
    assert_eq!(
        serde_json::from_str::<ImpactItems>(&serde_json::to_string(&items).unwrap()).unwrap(),
        items
    );

    for to in [
        DiffTarget::Revision(3),
        DiffTarget::Candidate {
            turtle: "<https://example.test/A> a <http://www.w3.org/2002/07/owl#Class> .".into(),
        },
    ] {
        let op = GraphSchemaOp::DiffOntology {
            connector: ResourceId::new("example").unwrap(),
            ontology_iri: "https://example.test/ontology".into(),
            from_revision: 2,
            to,
        };
        op.validate().unwrap();
        let wire = serde_json::to_value(&op).unwrap();
        assert_eq!(wire["op"], "diff_ontology");
        assert_eq!(serde_json::from_value::<GraphSchemaOp>(wire).unwrap(), op);
    }
}
