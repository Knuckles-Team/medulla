//! MEDULLA-CQ-ORCH-R700.3, MEDULLA-CQ-ORCH-R701.3: the wire vectors the hand-written Python
//! client is tested against. Each request round-trips through `Method` and each answer
//! through its result body, so a field the Rust types lack or add fails here and the Python
//! test (`tests/test_process_wire_vectors.py`) reads the same file.

use medulla_protocol_types::protocol::Method;
use medulla_types::process_import::{
    ProcessCompiled, ProcessModelExported, ProcessModelImported,
};

const VECTORS: &str = include_str!("../../../tests/fixtures/process_wire_vectors.json");

fn vectors() -> serde_json::Value {
    serde_json::from_str(VECTORS).expect("the vector file is JSON")
}

fn round_trip_method(vector: &serde_json::Value, key: &str) -> Method {
    let method: Method = serde_json::from_value(vector[key].clone()).expect(key);
    assert_eq!(serde_json::to_value(&method).unwrap(), vector[key], "{key}");
    method
}

fn round_trip_body<T: serde::Serialize + serde::de::DeserializeOwned>(
    vector: &serde_json::Value,
    key: &str,
) -> T {
    let body: T = serde_json::from_value(vector[key].clone()).expect(key);
    assert_eq!(serde_json::to_value(&body).unwrap(), vector[key], "{key}");
    body
}

// spec: MEDULLA-CQ-ORCH-R700.3
#[test]
fn process_compile_round_trips() {
    let vector = vectors();
    match round_trip_method(&vector, "process_compile") {
        Method::ProcessCompile {
            process_iri,
            expected_version,
        } => {
            assert_eq!(process_iri, "process:invoice");
            assert_eq!(expected_version, Some(1));
        }
        other => panic!("unexpected method {other:?}"),
    }
    assert!(matches!(
        round_trip_method(&vector, "process_compile_first"),
        Method::ProcessCompile {
            expected_version: None,
            ..
        }
    ));
    let compiled: ProcessCompiled = round_trip_body(&vector, "compiled");
    assert_eq!(compiled.version, 2);
}

// spec: MEDULLA-CQ-ORCH-R701.3
#[test]
fn process_model_import_and_export_round_trip() {
    let vector = vectors();
    assert!(matches!(
        round_trip_method(&vector, "process_model_import"),
        Method::ProcessModelImport {
            expected_version: None,
            ..
        }
    ));
    match round_trip_method(&vector, "process_model_export") {
        Method::ProcessModelExport {
            version,
            target,
            strict,
            ..
        } => {
            assert_eq!(version, 1);
            assert_eq!(
                target,
                Some(medulla_types::process_import::ProcessSourceSystem::ArisEpc)
            );
            assert!(strict);
        }
        other => panic!("unexpected method {other:?}"),
    }
    assert!(matches!(
        round_trip_method(&vector, "process_model_export_canonical"),
        Method::ProcessModelExport {
            target: None,
            strict: false,
            ..
        }
    ));
    let imported: ProcessModelImported = round_trip_body(&vector, "imported");
    assert_eq!(imported.version, 1);
    let exported: ProcessModelExported = round_trip_body(&vector, "exported");
    assert!(exported.document.is_none());
}
