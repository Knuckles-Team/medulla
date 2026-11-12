//! Closed storage-provider set (MEDULLA-ENGINE-PARITY-R019.1): the only
//! durable backends are the two `StorageBackendSpec` variants, `local` and
//! `object_store`. Any other variant name fails to deserialize, and the store
//! admission names the failure `STORAGE_BACKEND_UNKNOWN`.

use medulla_storage::{BackendSpecRefusal, StorageBackendSpec};

// spec: MEDULLA-ENGINE-PARITY-R019.1
#[test]
fn provider_set_is_closed() {
    // The two served variants decode.
    let local: StorageBackendSpec = serde_json::from_str(r#"{"variant":"local"}"#).unwrap();
    assert_eq!(local, StorageBackendSpec::Local);
    let object: StorageBackendSpec = serde_json::from_str(r#"{"variant":"object_store"}"#).unwrap();
    assert!(matches!(object, StorageBackendSpec::ObjectStore { .. }));

    // Every outside database name is a third variant and is refused.
    for name in [
        "postgres",
        "postgresql",
        "mysql",
        "sqlite",
        "jdbc",
        "s3",
        "",
    ] {
        let body = format!(r#"{{"variant":"{name}"}}"#);
        assert!(
            serde_json::from_str::<StorageBackendSpec>(&body).is_err(),
            "variant `{name}` must not deserialize"
        );
        let wire = rmp_serde::to_vec_named(&serde_json::json!({ "variant": name })).unwrap();
        assert!(matches!(
            StorageBackendSpec::from_wire(&wire),
            Err(BackendSpecRefusal::UnknownVariant { .. })
        ));
    }

    // Unknown fields on a served variant are refused too.
    assert!(
        serde_json::from_str::<StorageBackendSpec>(r#"{"variant":"local","dsn":"x"}"#).is_err()
    );
}
