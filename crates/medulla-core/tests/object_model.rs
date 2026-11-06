//! MEDULLA-TYPED-PACKS-R650.1: object-model declarations round-trip and refuse
//! a zero `max_card` and unknown fields.

use medulla_core::ontology::object_model::{InterfaceDecl, LinkType, ValueType};

// spec: MEDULLA-TYPED-PACKS-R650.1
#[test]
fn declarations_round_trip_and_reject_zero_max_card() {
    let value: ValueType = serde_json::from_value(serde_json::json!({
        "name": "Sku",
        "base": "http://www.w3.org/2001/XMLSchema#string",
        "pattern": "^[A-Z]{3}-[0-9]+$",
        "max_len": 16,
        "one_of": ["ABC-1", "ABC-2"]
    }))
    .expect("value type parses");
    let back: ValueType = serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
    assert_eq!(back, value);

    let link: LinkType = serde_json::from_value(serde_json::json!({
        "name": "owns",
        "from_class": "http://knuckles.team/kg#Person",
        "to_class": "http://knuckles.team/kg#Asset",
        "min_card": 1,
        "max_card": 3
    }))
    .expect("link type parses");
    assert_eq!(link.max_card.map(|n| n.get()), Some(3));
    let back: LinkType = serde_json::from_str(&serde_json::to_string(&link).unwrap()).unwrap();
    assert_eq!(back, link);

    let unbounded: LinkType = serde_json::from_value(serde_json::json!({
        "name": "tags",
        "from_class": "a",
        "to_class": "b"
    }))
    .unwrap();
    assert!(unbounded.max_card.is_none());

    let zero = serde_json::from_value::<LinkType>(serde_json::json!({
        "name": "owns",
        "from_class": "a",
        "to_class": "b",
        "max_card": 0
    }));
    assert!(zero.is_err(), "max_card 0 must be refused");

    let iface: InterfaceDecl = serde_json::from_value(serde_json::json!({
        "name": "Named",
        "required_properties": ["http://schema.org/name"],
        "required_links": ["owns"]
    }))
    .unwrap();
    let back: InterfaceDecl =
        serde_json::from_str(&serde_json::to_string(&iface).unwrap()).unwrap();
    assert_eq!(back, iface);

    let unknown = serde_json::from_value::<InterfaceDecl>(serde_json::json!({
        "name": "Named",
        "extra": true
    }));
    assert!(unknown.is_err(), "unknown fields must be refused");
}

fn model() -> medulla_core::ontology::object_model::ObjectModel {
    serde_json::from_value(serde_json::json!({
        "value_types": [
            {"name": "Sku", "base": "http://www.w3.org/2001/XMLSchema#string",
             "pattern": "^[A-Z]{3}-[0-9]+$", "max_len": 16, "one_of": ["ABC-1", "ABC-2"]},
            {"name": "Qty", "base": "http://www.w3.org/2001/XMLSchema#integer", "min": 0.0, "max": 99.0}
        ],
        "link_types": [
            {"name": "owns", "from_class": "http://k/Person", "to_class": "http://k/Asset",
             "min_card": 1, "max_card": 3, "junction": "http://k/Ownership"}
        ],
        "interfaces": [
            {"name": "Named", "required_properties": ["http://k/name"], "required_links": ["owns"]}
        ],
        "properties": [
            {"class": "http://k/Asset", "property": "http://k/sku", "value_type": "Sku"},
            {"class": "http://k/Asset", "property": "http://k/qty", "value_type": "Qty"}
        ],
        "implementations": [{"class": "http://k/Person", "interface": "Named"}]
    }))
    .expect("model parses")
}

// spec: MEDULLA-TYPED-PACKS-R650.2
#[test]
fn compile_is_deterministic_and_maps_each_constraint() {
    use medulla_core::ontology::object_model::{compile_shapes, interface_gaps, ShapeTerm};

    let model = model();
    let first = compile_shapes(&model);
    assert_eq!(first, compile_shapes(&model), "compile is deterministic");
    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(first, sorted, "output is sorted");

    let has = |predicate: &str, object: &ShapeTerm| {
        first.iter().any(|t| {
            t.predicate == format!("http://www.w3.org/ns/shacl#{predicate}") && &t.object == object
        })
    };
    let xsd = |name: &str| Some(format!("http://www.w3.org/2001/XMLSchema#{name}"));
    let int = |n: &str| ShapeTerm::Literal {
        lexical: n.into(),
        datatype: xsd("integer"),
    };
    assert!(has(
        "datatype",
        &ShapeTerm::Iri("http://www.w3.org/2001/XMLSchema#string".into())
    ));
    assert!(has(
        "pattern",
        &ShapeTerm::Literal {
            lexical: "^[A-Z]{3}-[0-9]+$".into(),
            datatype: xsd("string")
        }
    ));
    assert!(has(
        "minInclusive",
        &ShapeTerm::Literal {
            lexical: "0".into(),
            datatype: xsd("double")
        }
    ));
    assert!(has(
        "maxInclusive",
        &ShapeTerm::Literal {
            lexical: "99".into(),
            datatype: xsd("double")
        }
    ));
    assert!(has("maxLength", &int("16")));
    assert!(first.iter().any(|t| t.predicate.ends_with("shacl#in")));
    assert!(has("class", &ShapeTerm::Iri("http://k/Asset".into())));
    assert!(has("minCount", &int("1")));
    assert!(has("maxCount", &int("3")));
    // The junction class carries two required links.
    let junction = first
        .iter()
        .filter(|t| {
            t.predicate.ends_with("shacl#targetClass")
                && t.object == ShapeTerm::Iri("http://k/Ownership".into())
        })
        .count();
    assert!(junction >= 1);

    // The Person class declares the `owns` link but not `http://k/name`.
    let gaps = interface_gaps(&model);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].member, "http://k/name");
}
