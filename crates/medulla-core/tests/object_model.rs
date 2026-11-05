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
