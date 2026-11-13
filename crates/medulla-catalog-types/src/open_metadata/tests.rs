use super::*;

fn record() -> OpenMetadataRecord {
    OpenMetadataRecord {
        schema_version: OPEN_METADATA_SCHEMA_VERSION,
        guid: "g-1".into(),
        type_name: "GlossaryTerm".into(),
        version: 7,
        update_time_ms: 1_700_000_000_000,
        properties: BTreeMap::from([("displayName".into(), serde_json::json!("Customer"))]),
        classifications: vec![OpenMetadataClassification {
            schema_version: OPEN_METADATA_SCHEMA_VERSION,
            type_name: "Confidentiality".into(),
            properties: BTreeMap::from([("level".into(), serde_json::json!(2))]),
        }],
        relationships: vec![OpenMetadataRelationship {
            schema_version: OPEN_METADATA_SCHEMA_VERSION,
            guid: "r-1".into(),
            type_name: "TermAnchor".into(),
            end1_guid: "g-1".into(),
            end2_guid: "g-2".into(),
            version: 3,
        }],
    }
}

// spec: MEDULLA-TYPED-PACKS-R110.2
#[test]
fn record_round_trips_and_refuses_unknown_fields() {
    let original = record();
    let wire = serde_json::to_value(&original).unwrap();
    let back: OpenMetadataRecord = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(back, original);

    let mut extra = wire.clone();
    extra["surprise"] = serde_json::json!(true);
    assert!(serde_json::from_value::<OpenMetadataRecord>(extra).is_err());

    let mut nested = wire.clone();
    nested["relationships"][0]["surprise"] = serde_json::json!(1);
    assert!(serde_json::from_value::<OpenMetadataRecord>(nested).is_err());

    let mut classification = wire;
    classification["classifications"][0]["surprise"] = serde_json::json!(1);
    assert!(serde_json::from_value::<OpenMetadataRecord>(classification).is_err());
}

const CAT: &str = "https://knuckles-team.github.io/ontology/catalog#";

fn view() -> OpenMetadataTypeView {
    OpenMetadataTypeView::from_annotations([
        ("GlossaryTerm", format!("{CAT}GlossaryTerm")),
        ("DataSet", format!("{CAT}DataResource")),
        ("DataFlow", format!("{CAT}LineageEdge")),
        (
            "Confidentiality",
            format!("{CAT}ConfidentialityClassification"),
        ),
    ])
}

fn typed(guid: &str, type_name: &str, version: u64) -> OpenMetadataRecord {
    OpenMetadataRecord {
        schema_version: OPEN_METADATA_SCHEMA_VERSION,
        guid: guid.into(),
        type_name: type_name.into(),
        version,
        update_time_ms: 1,
        properties: BTreeMap::new(),
        classifications: vec![],
        relationships: vec![],
    }
}

// spec: MEDULLA-TYPED-PACKS-R110.3
#[test]
fn inbound_maps_term_asset_and_lineage() {
    let view = view();

    let mut term = typed("g-1", "GlossaryTerm", 7);
    term.properties
        .insert("displayName".into(), serde_json::json!("Customer"));
    term.classifications.push(OpenMetadataClassification {
        schema_version: OPEN_METADATA_SCHEMA_VERSION,
        type_name: "Confidentiality".into(),
        properties: BTreeMap::from([("level".into(), serde_json::json!(2))]),
    });
    let writes = inbound_mutations(&view, &term).unwrap();
    assert_eq!(writes.len(), 2);
    match &writes[0] {
        OpenMetadataMutation::UpsertNode {
            id,
            class_iri,
            properties,
        } => {
            assert_eq!(id, "g-1");
            assert_eq!(class_iri, &format!("{CAT}GlossaryTerm"));
            assert_eq!(properties[EXTERNAL_GUID_PROPERTY], serde_json::json!("g-1"));
            assert_eq!(properties[EXTERNAL_VERSION_PROPERTY], serde_json::json!(7));
            assert_eq!(properties["displayName"], serde_json::json!("Customer"));
        }
        other => panic!("expected a node, got {other:?}"),
    }
    assert!(matches!(
        &writes[1],
        OpenMetadataMutation::UpsertNode { id, class_iri, .. }
            if id == "g-1#Confidentiality"
                && class_iri == &format!("{CAT}ConfidentialityClassification")
    ));

    let asset = typed("g-2", "DataSet", 1);
    let writes = inbound_mutations(&view, &asset).unwrap();
    assert!(matches!(
        &writes[0],
        OpenMetadataMutation::UpsertNode { id, class_iri, .. }
            if id == "g-2" && class_iri == &format!("{CAT}DataResource")
    ));

    let mut lineage = typed("g-1", "GlossaryTerm", 7);
    lineage.relationships.push(OpenMetadataRelationship {
        schema_version: OPEN_METADATA_SCHEMA_VERSION,
        guid: "r-1".into(),
        type_name: "DataFlow".into(),
        end1_guid: "g-2".into(),
        end2_guid: "g-3".into(),
        version: 4,
    });
    let writes = inbound_mutations(&view, &lineage).unwrap();
    match &writes[1] {
        OpenMetadataMutation::UpsertEdge {
            id,
            class_iri,
            from,
            to,
            properties,
        } => {
            assert_eq!(
                (id.as_str(), from.as_str(), to.as_str()),
                ("r-1", "g-2", "g-3")
            );
            assert_eq!(class_iri, &format!("{CAT}LineageEdge"));
            assert_eq!(properties[EXTERNAL_VERSION_PROPERTY], serde_json::json!(4));
        }
        other => panic!("expected an edge, got {other:?}"),
    }

    // An unmapped type writes nothing and names the type.
    let err = inbound_mutations(&view, &typed("g-9", "Asset", 1)).unwrap_err();
    assert_eq!(
        err,
        OpenMetadataError::TypeUnmapped {
            type_name: "Asset".into()
        }
    );
}
