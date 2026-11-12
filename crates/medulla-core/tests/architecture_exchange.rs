//! ArchiMate Open Exchange reader, mapping and writer (MEDULLA-CQ-ORCH-R705.3,
//! R705.4, R705.5).

use medulla_core::architecture_exchange::graph::{exchange_mutations, ExchangeMutation};
use medulla_core::architecture_exchange::read::parse_exchange;
use medulla_core::architecture_exchange::write::write_exchange;
use medulla_core::architecture_exchange::{ArchimateTypeMap, ExchangeView, ExportScope};
use medulla_core::process_exchange::xml_tree::XmlLimits;
use medulla_graph_core::graph::GraphCore;
use medulla_types::archimate_exchange::ArchimateExchangeError;
use serde_json::json;

const DOC: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://www.opengroup.org/xsd/archimate/3.0/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" identifier="id-model">
  <name xml:lang="en">Estate</name>
  <elements>
    <element identifier="id-proc" xsi:type="BusinessProcess"><name xml:lang="en">Order to cash</name>
      <properties><property propertyDefinitionRef="pd-tier"><value xml:lang="en">gold</value></property></properties>
    </element>
    <element identifier="id-app" xsi:type="ApplicationComponent"><name xml:lang="en">Billing</name></element>
    <element identifier="id-obj" xsi:type="DataObject"><name xml:lang="en">Invoice</name></element>
  </elements>
  <relationships>
    <relationship identifier="id-r1" source="id-app" target="id-proc" xsi:type="Serving"/>
    <relationship identifier="id-r2" source="id-app" target="id-obj" xsi:type="Access"/>
  </relationships>
  <organizations><item><label xml:lang="en">Business</label><item identifierRef="id-proc"/></item></organizations>
  <propertyDefinitions><propertyDefinition identifier="pd-tier" type="string"><name xml:lang="en">tier</name></propertyDefinition></propertyDefinitions>
  <views><diagrams><view identifier="id-view" xsi:type="Diagram"><name xml:lang="en">Main</name>
    <node identifier="n1" elementRef="id-proc" xsi:type="Element" x="10" y="20" w="120" h="55"/>
    <node identifier="n2" elementRef="id-app" xsi:type="Element" x="200" y="20" w="120" h="55"/>
    <connection identifier="c1" relationshipRef="id-r1" xsi:type="Relationship" source="n2" target="n1"/>
  </view></diagrams></views>
</model>"##;

fn types() -> ArchimateTypeMap {
    let pairs = [
        ("BusinessProcess", "BusinessProcess"),
        ("ApplicationComponent", "ApplicationComponent"),
        ("DataObject", "DataObject"),
        ("BusinessActor", "BusinessActor"),
    ];
    ArchimateTypeMap {
        classes: pairs
            .iter()
            .map(|(a, c)| (a.to_string(), c.to_string()))
            .collect(),
        edges: [("Serving".to_string(), "usesTool".to_string())].into(),
        parents: [("BusinessTask".to_string(), "BusinessProcess".to_string())].into(),
    }
}

fn node(core: &GraphCore, id: &str, props: serde_json::Value) {
    core.add_node(id.to_owned(), rmp_serde::to_vec_named(&props).unwrap());
}

// spec: MEDULLA-CQ-ORCH-R705.3
#[test]
fn reader_keeps_views_and_refuses_invalid_relationship() {
    let doc = parse_exchange(DOC.as_bytes(), XmlLimits::default()).unwrap();
    assert_eq!(doc.model_id, "id-model");
    assert_eq!(doc.name, "Estate");
    assert_eq!(doc.elements.len(), 3);
    assert_eq!(doc.elements[0].properties["pd-tier"], "gold");
    assert_eq!(doc.relationships[1].relationship_type, "Access");
    assert_eq!(doc.organizations[0].label, "Business");
    assert_eq!(doc.organizations[0].children[0].item_refs, vec!["id-proc"]);
    assert_eq!(doc.property_definitions[0].name, "tier");
    let view = &doc.views[0];
    assert_eq!((view.nodes.len(), view.connections.len()), (2, 1));
    assert_eq!(view.nodes[0].w, 120);
    assert_eq!(
        view.connections[0].relationship_ref.as_deref(),
        Some("id-r1")
    );

    // Access onto a business actor is forbidden by the relationship table.
    let invalid = DOC.replace(
        r#"identifier="id-r2" source="id-app" target="id-obj""#,
        r#"identifier="id-r2" source="id-proc" target="id-app""#,
    );
    assert_eq!(
        parse_exchange(invalid.as_bytes(), XmlLimits::default()),
        Err(ArchimateExchangeError::RelationshipInvalid {
            identifier: "id-r2".into()
        })
    );
    let dangling = DOC.replace(r#"target="id-obj""#, r#"target="id-missing""#);
    assert!(matches!(
        parse_exchange(dangling.as_bytes(), XmlLimits::default()),
        Err(ArchimateExchangeError::RelationshipInvalid { .. })
    ));
    let planted = r#"<!DOCTYPE m [<!ENTITY x "y">]><model xmlns="http://www.opengroup.org/xsd/archimate/3.0/"/>"#;
    assert_eq!(
        parse_exchange(planted.as_bytes(), XmlLimits::default()),
        Err(ArchimateExchangeError::EntityRefused)
    );
    assert!(matches!(
        parse_exchange(b"<model xmlns=\"urn:other\"/>", XmlLimits::default()),
        Err(ArchimateExchangeError::NamespaceUnknown { .. })
    ));
    let small = XmlLimits {
        max_bytes: 40,
        max_depth: 64,
    };
    assert!(matches!(
        parse_exchange(DOC.as_bytes(), small),
        Err(ArchimateExchangeError::TooLarge { .. })
    ));
}

// spec: MEDULLA-CQ-ORCH-R705.4
#[test]
fn mapping_updates_known_and_proposes_name_matches() {
    let doc = parse_exchange(DOC.as_bytes(), XmlLimits::default()).unwrap();
    let types = types();
    let core = GraphCore::new();
    // A node already imported under id-app, and a native node named like id-proc.
    node(
        &core,
        "known-app",
        json!({"type": "ApplicationComponent", "archimateIdentifier": "id-app", "name": "Billing"}),
    );
    node(
        &core,
        "native-proc",
        json!({"type": "BusinessProcess", "name": "order to cash"}),
    );
    let view = ExchangeView {
        core: &core,
        types: &types,
    };
    let mutations = exchange_mutations(&view, &doc);
    assert_eq!(mutations, exchange_mutations(&view, &doc), "deterministic");

    let upserts: Vec<(&str, &str)> = mutations
        .iter()
        .filter_map(|m| match m {
            ExchangeMutation::UpsertNode { id, label, .. } => Some((id.as_str(), label.as_str())),
            _ => None,
        })
        .collect();
    assert!(
        upserts.contains(&("known-app", "ApplicationComponent")),
        "known node updated"
    );
    assert!(upserts.contains(&("archimate:id-model:id-proc", "BusinessProcess")));
    assert!(upserts.contains(&("archimate:id-model:id-r1", "ArchimateRelationship")));
    assert!(upserts.contains(&("archimate:id-model:id-view", "ArchimateView")));
    // The name match is a proposal for review, never a sameAs.
    assert!(upserts.contains(&("proposal:id-model:id-proc", "EntityResolutionProposal")));
    assert!(mutations.contains(&ExchangeMutation::Link {
        source: "proposal:id-model:id-proc".into(),
        target: "native-proc".into(),
        predicate: "PROPOSES_RESOLUTION".into(),
    }));
    assert!(!format!("{mutations:?}").contains("sameAs"));
    // The relationship is reified and also a direct edge with the mapped predicate.
    assert!(mutations.contains(&ExchangeMutation::Link {
        source: "known-app".into(),
        target: "archimate:id-model:id-proc".into(),
        predicate: "usesTool".into(),
    }));
    // An element that matched by identifier produces no proposal.
    assert!(!upserts
        .iter()
        .any(|(id, _)| *id == "proposal:id-model:id-app"));
}

fn apply(core: &GraphCore, mutations: &[ExchangeMutation]) {
    for m in mutations {
        match m {
            ExchangeMutation::UpsertNode { id, properties, .. } => {
                node(core, id, serde_json::to_value(properties).unwrap())
            }
            ExchangeMutation::Link {
                source,
                target,
                predicate,
            } => {
                core.add_edge(
                    source.clone(),
                    target.clone(),
                    rmp_serde::to_vec_named(&json!({"relationship": predicate})).unwrap(),
                )
                .unwrap();
            }
        }
    }
}

// spec: MEDULLA-CQ-ORCH-R705.5
#[test]
fn write_then_read_keeps_digest() {
    let doc = parse_exchange(DOC.as_bytes(), XmlLimits::default()).unwrap();
    let types = types();
    let source = GraphCore::new();
    apply(
        &source,
        &exchange_mutations(
            &ExchangeView {
                core: &source,
                types: &types,
            },
            &doc,
        ),
    );
    // A node outside the architecture classes is not exported.
    node(&source, "other", json!({"type": "Person", "name": "Ada"}));
    let scope = ExportScope {
        tenant: "t1".into(),
        graph: None,
    };
    let bytes = write_exchange(
        &ExchangeView {
            core: &source,
            types: &types,
        },
        &scope,
    )
    .unwrap();
    assert_eq!(
        bytes,
        write_exchange(
            &ExchangeView {
                core: &source,
                types: &types
            },
            &scope
        )
        .unwrap(),
        "equal graphs give equal bytes"
    );
    let back = parse_exchange(&bytes, XmlLimits::default()).unwrap();
    assert_eq!(back.elements.len(), 3);
    assert_eq!(back.relationships.len(), 2);
    assert_eq!(back.views.len(), 1);
    // Identifiers survive; the canonical content of elements, relationships and
    // the view is the same after a write and a read.
    let ids = |d: &medulla_types::archimate_exchange::ArchimateExchange| {
        d.elements
            .iter()
            .map(|e| (e.identifier.clone(), e.element_type.clone(), e.name.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&back), ids(&doc));
    assert_eq!(back.views, doc.views);
    // Writing the re-read document into an empty graph and exporting again is stable.
    let target = GraphCore::new();
    apply(
        &target,
        &exchange_mutations(
            &ExchangeView {
                core: &target,
                types: &types,
            },
            &back,
        ),
    );
    let again = write_exchange(
        &ExchangeView {
            core: &target,
            types: &types,
        },
        &scope,
    )
    .unwrap();
    assert_eq!(
        parse_exchange(&again, XmlLimits::default())
            .unwrap()
            .canonical_digest(),
        back.canonical_digest()
    );
}
