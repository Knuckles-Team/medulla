//! MEDULLA-UNIFIED-DATA-PLANE-R038.6.3: the wire vector the hand-written Python client is
//! tested against. `view` is the one added field of `Method::CypherQuery`; a request without
//! it serializes without the key, so existing signed bodies are unchanged.

use medulla_protocol_types::protocol::{CypherMode, Method};

const VECTOR: &str = include_str!("../../../tests/fixtures/cypher_view_vector.json");

#[test]
fn cypher_view_round_trips() {
    let vector: serde_json::Value = serde_json::from_str(VECTOR).unwrap();

    let method: Method = serde_json::from_value(vector["request"].clone()).unwrap();
    match &method {
        Method::CypherQuery { query, mode, view } => {
            assert_eq!(query, "MATCH (p:Person) RETURN p.name");
            assert_eq!(*mode, CypherMode::Read);
            assert_eq!(view.as_deref(), Some("people"));
        }
        other => panic!("unexpected method {other:?}"),
    }
    assert_eq!(serde_json::to_value(&method).unwrap(), vector["request"]);

    let plain: Method = serde_json::from_value(vector["without_view"].clone()).unwrap();
    assert!(matches!(&plain, Method::CypherQuery { view: None, .. }));
    assert_eq!(
        serde_json::to_value(&plain).unwrap(),
        vector["without_view"]
    );
}
