//! The Turtle to `ObjectModel` extractor (MEDULLA-TYPED-PACKS-R650.7).
#![cfg(feature = "rdf")]

use medulla_core::ontology::object_model::ObjectModel;
use medulla_rdf::pack::{extract_object_model, ObjectModelError};

const PREFIXES: &str = "\
@prefix : <https://knuckles-team.github.io/ontology/core#> .
@prefix ex: <https://example.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
";

fn doc(body: &str) -> String {
    format!("{PREFIXES}{body}")
}

fn extract(body: &str) -> Result<ObjectModel, ObjectModelError> {
    extract_object_model(&[&doc(body)])
}

const DECLARATIONS: &str = r#"
ex:Sku a :ValueType ;
    sh:datatype xsd:string ;
    sh:pattern "^[A-Z]{3}-[0-9]+$" ;
    sh:maxLength 16 ;
    sh:in ( "ABC-1" "ABC-2" ) .
ex:Qty a :ValueType ;
    sh:datatype xsd:integer ;
    sh:minInclusive 0 ;
    sh:maxInclusive 99 .
ex:sku a owl:DatatypeProperty ; rdfs:domain ex:Asset ; rdfs:range ex:Sku .
ex:qty a owl:DatatypeProperty ; rdfs:domain ex:Asset ; rdfs:range ex:Qty .
ex:plain a owl:DatatypeProperty ; rdfs:domain ex:Asset ; rdfs:range xsd:string .
ex:owns a :LinkType ; rdfs:domain ex:Person ; rdfs:range ex:Asset ;
    sh:minCount 1 ; sh:maxCount 3 .
ex:shares a :JunctionLink ; rdfs:domain ex:Person ; rdfs:range ex:Asset .
ex:Named a :Interface ;
    :hasInterfaceProperty ex:name , ex:owns .
ex:Person :implementsInterface ex:Named .
"#;

// spec: MEDULLA-TYPED-PACKS-R650.7
#[test]
fn extract_reads_declared_value_link_and_interface_terms() {
    let model = extract(DECLARATIONS).expect("well-formed declarations extract");

    let names: Vec<&str> = model.value_types.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        names,
        ["https://example.org/Qty", "https://example.org/Sku"]
    );
    let sku = &model.value_types[1];
    assert_eq!(sku.base, "http://www.w3.org/2001/XMLSchema#string");
    assert_eq!(sku.pattern.as_deref(), Some("^[A-Z]{3}-[0-9]+$"));
    assert_eq!(sku.max_len, Some(16));
    assert_eq!(sku.one_of, ["ABC-1", "ABC-2"]);
    let qty = &model.value_types[0];
    assert_eq!((qty.min, qty.max), (Some(0.0), Some(99.0)));

    // Only a property whose range is a declared value type is a PropertyDecl.
    let properties: Vec<(&str, &str)> = model
        .properties
        .iter()
        .map(|p| (p.property.as_str(), p.value_type.as_deref().unwrap()))
        .collect();
    assert_eq!(
        properties,
        [
            ("https://example.org/qty", "https://example.org/Qty"),
            ("https://example.org/sku", "https://example.org/Sku"),
        ]
    );

    let owns = model
        .link_types
        .iter()
        .find(|l| l.name == "https://example.org/owns")
        .unwrap();
    assert_eq!(owns.from_class, "https://example.org/Person");
    assert_eq!(owns.to_class, "https://example.org/Asset");
    assert_eq!(
        (owns.min_card, owns.max_card.map(|m| m.get())),
        (1, Some(3))
    );
    assert_eq!(owns.junction, None);
    let shares = model
        .link_types
        .iter()
        .find(|l| l.name == "https://example.org/shares")
        .unwrap();
    assert_eq!((shares.min_card, shares.max_card), (0, None));
    assert_eq!(
        shares.junction.as_deref(),
        Some("https://example.org/shares")
    );

    // A member that is a declared link goes to required_links; the rest to properties.
    assert_eq!(model.interfaces.len(), 1);
    assert_eq!(
        model.interfaces[0].required_properties,
        ["https://example.org/name"]
    );
    assert_eq!(
        model.interfaces[0].required_links,
        ["https://example.org/owns"]
    );
    assert_eq!(model.implementations.len(), 1);
    assert_eq!(model.implementations[0].class, "https://example.org/Person");

    // Deterministic across runs and document splits.
    assert_eq!(model, extract(DECLARATIONS).unwrap());
}

// spec: MEDULLA-TYPED-PACKS-R650.7
#[test]
fn each_malformed_declaration_is_one_named_reason() {
    let base_missing = extract("ex:V a :ValueType .").unwrap_err();
    assert!(matches!(
        base_missing,
        ObjectModelError::ValueTypeBaseMissing { .. }
    ));
    assert_eq!(base_missing.subject(), "https://example.org/V");
    let two_bases =
        extract("ex:V a :ValueType ; sh:datatype xsd:string , xsd:integer .").unwrap_err();
    assert!(matches!(
        two_bases,
        ObjectModelError::ValueTypeBaseMissing { .. }
    ));

    let no_range = extract("ex:L a :LinkType ; rdfs:domain ex:A .").unwrap_err();
    assert!(matches!(
        no_range,
        ObjectModelError::LinkEndpointMissing { .. }
    ));
    let two_domains =
        extract("ex:L a :LinkType ; rdfs:domain ex:A , ex:B ; rdfs:range ex:C .").unwrap_err();
    assert!(matches!(
        two_domains,
        ObjectModelError::LinkEndpointMissing { .. }
    ));

    for count in [
        "sh:maxCount 0",
        "sh:maxCount \"many\"",
        "sh:minCount 3 ; sh:maxCount 2",
    ] {
        let body = format!("ex:L a :LinkType ; rdfs:domain ex:A ; rdfs:range ex:B ; {count} .");
        assert!(
            matches!(
                extract(&body).unwrap_err(),
                ObjectModelError::CardinalityInvalid { .. }
            ),
            "{count}"
        );
    }

    let facet = extract("ex:V a :ValueType ; sh:datatype xsd:integer ; sh:minInclusive \"low\" .")
        .unwrap_err();
    assert!(matches!(facet, ObjectModelError::FacetInvalid { .. }));

    let unknown = extract("ex:C :implementsInterface ex:Missing .").unwrap_err();
    assert!(matches!(unknown, ObjectModelError::InterfaceUnknown { .. }));
    assert_eq!(unknown.subject(), "https://example.org/C");
}
