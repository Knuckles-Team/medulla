//! Typed durable-store backend specification (MEDULLA-OBJECT-STORAGE-BACKEND-R001).
//!
//! A [`StorageBackendSpec`] declares the durable-store backend for a graph.
//! `local` (redb) is the default; `object_store` is S3-compatible. Endpoint,
//! bucket and root are named by secret reference only, never by value.
//! [`StorageBackendSpec::validate`] is the single admission step: it refuses
//! atomically with a typed [`BackendSpecRefusal`] before any effect.
//! A `local` spec opens redb. An `object_store` spec that validates is
//! admitted by [`StorageBackendSpec::admit`] and opened by the served open
//! path (`medulla_persistence::object_store_backend::open_from_env`,
//! MEDULLA-OBJECT-STORAGE-BACKEND-R002.1); a build without that path refuses
//! it at startup, so nothing silently falls back to redb.

use crate::owner::lineage::{pinned_layout_digest, ALL_LAYOUTS};
use serde::{Deserialize, Serialize};

/// How durable a committed manifest publish is before the client is acked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurabilityClass {
    /// Acked after the local fsync.
    LocalFsync,
    /// Acked after the object store confirms the conditional manifest swap.
    ObjectStoreConditionalPut,
}

/// A reference to a secret held outside the spec; never the secret itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef(pub String);

/// The declared durable-store backend for a graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StorageBackendSpecWire")]
#[serde(tag = "variant", rename_all = "snake_case", deny_unknown_fields)]
pub enum StorageBackendSpec {
    /// redb on local disk: the default.
    #[default]
    Local,
    /// S3-compatible object storage.
    ObjectStore {
        endpoint: Option<SecretRef>,
        bucket: Option<SecretRef>,
        root: Option<SecretRef>,
        durability: Option<DurabilityClass>,
        /// Hex layout digest naming an entry in the owner-store lineage registry.
        layout_digest: Option<String>,
    },
}

/// The decoding shape of [`StorageBackendSpec`]. Serde does not apply
/// `deny_unknown_fields` to a unit variant of an internally tagged enum, so
/// `{"variant":"local","dsn":"x"}` would decode; the empty struct variant here
/// closes the `local` field set, and the object-store fields stay closed too.
#[derive(Deserialize)]
#[serde(tag = "variant", rename_all = "snake_case", deny_unknown_fields)]
enum StorageBackendSpecWire {
    Local {},
    ObjectStore {
        endpoint: Option<SecretRef>,
        bucket: Option<SecretRef>,
        root: Option<SecretRef>,
        durability: Option<DurabilityClass>,
        layout_digest: Option<String>,
    },
}

impl From<StorageBackendSpecWire> for StorageBackendSpec {
    fn from(wire: StorageBackendSpecWire) -> Self {
        match wire {
            StorageBackendSpecWire::Local {} => Self::Local,
            StorageBackendSpecWire::ObjectStore {
                endpoint,
                bucket,
                root,
                durability,
                layout_digest,
            } => Self::ObjectStore {
                endpoint,
                bucket,
                root,
                durability,
                layout_digest,
            },
        }
    }
}

/// Typed, atomic refusal; none of these leaves any state changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum BackendSpecRefusal {
    /// A required secret reference is absent or empty.
    MissingSecretRef { field: String },
    /// The object-store spec declares no durability class.
    MissingDurabilityClass,
    /// The variant tag is not one this engine knows.
    UnknownVariant { variant: String },
    /// The layout digest is absent from the lineage registry.
    LayoutNotInLineage { layout_digest: String },
    /// The spec is valid but this build cannot open it.
    Unsupported { variant: String },
    /// The endpoint names an outside database, which is only ever a source
    /// (MEDULLA-ENGINE-PARITY-R019.2); the engine's records are never placed there.
    ExternalPlacementForbidden { scheme: String },
}

/// Endpoint schemes of outside databases. An engine record is never placed in one;
/// they reach the engine only as attached sources (MEDULLA-ENGINE-PARITY-R019.2).
pub const EXTERNAL_DATABASE_SCHEMES: [&str; 5] =
    ["postgres", "postgresql", "mysql", "sqlite", "jdbc"];

/// The wire-stable code of [`BackendSpecRefusal::ExternalPlacementForbidden`].
pub const EXTERNAL_PLACEMENT_FORBIDDEN: &str = "EXTERNAL_PLACEMENT_FORBIDDEN";

/// The outside-database scheme `endpoint` starts with (`scheme://` or `scheme:`), if any.
/// JDBC urls are `jdbc:<driver>://`, so the first segment decides.
fn external_database_scheme(endpoint: &str) -> Option<&'static str> {
    let head = endpoint.trim().split(':').next()?.to_ascii_lowercase();
    EXTERNAL_DATABASE_SCHEMES
        .iter()
        .copied()
        .find(|scheme| *scheme == head)
}

impl BackendSpecRefusal {
    /// The wire-stable code for this refusal, when it has a dedicated one.
    pub fn wire_code(&self) -> Option<&'static str> {
        match self {
            Self::ExternalPlacementForbidden { .. } => Some(EXTERNAL_PLACEMENT_FORBIDDEN),
            _ => None,
        }
    }
}

impl StorageBackendSpec {
    /// Decode a wire spec (named-field msgpack); an unknown variant tag or
    /// malformed body is a typed refusal.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, BackendSpecRefusal> {
        rmp_serde::from_slice(bytes).map_err(|e| BackendSpecRefusal::UnknownVariant {
            variant: e.to_string(),
        })
    }

    /// Encode to the wire form [`Self::from_wire`] reads.
    pub fn to_wire(&self) -> Vec<u8> {
        rmp_serde::to_vec_named(self).expect("backend spec encodes")
    }

    /// Check the spec is bound and names a registered layout. Pure.
    pub fn validate(&self) -> Result<(), BackendSpecRefusal> {
        let Self::ObjectStore {
            endpoint,
            bucket,
            root,
            durability,
            layout_digest,
        } = self
        else {
            return Ok(());
        };
        for (field, secret) in [("endpoint", endpoint), ("bucket", bucket), ("root", root)] {
            match secret {
                Some(SecretRef(name)) if !name.trim().is_empty() => {}
                _ => {
                    return Err(BackendSpecRefusal::MissingSecretRef {
                        field: field.to_string(),
                    })
                }
            }
        }
        if let Some(SecretRef(reference)) = endpoint {
            // A reference that carries an outside-database url instead of naming a secret.
            self.check_endpoint_scheme(reference)?;
        }
        if durability.is_none() {
            return Err(BackendSpecRefusal::MissingDurabilityClass);
        }
        let digest = layout_digest.clone().unwrap_or_default();
        if !ALL_LAYOUTS
            .iter()
            .any(|l| pinned_layout_digest(*l) == digest)
        {
            return Err(BackendSpecRefusal::LayoutNotInLineage {
                layout_digest: digest,
            });
        }
        Ok(())
    }

    /// Refuse an endpoint, once resolved from its secret reference, whose scheme is an
    /// outside database (`postgres`, `postgresql`, `mysql`, `sqlite`, `jdbc`). Writes
    /// nothing; the refusal names the scheme (MEDULLA-ENGINE-PARITY-R019.2). Pure.
    pub fn check_endpoint_scheme(&self, resolved_endpoint: &str) -> Result<(), BackendSpecRefusal> {
        match external_database_scheme(resolved_endpoint) {
            Some(scheme) => Err(BackendSpecRefusal::ExternalPlacementForbidden {
                scheme: scheme.to_string(),
            }),
            None => Ok(()),
        }
    }

    /// Parse the `MEDULLA_STORAGE_BACKEND` setting: unset, empty or `local` is
    /// the redb default; anything else is a JSON spec. An unreadable body is
    /// `UnknownVariant` (`STORAGE_BACKEND_UNKNOWN` at the server boundary).
    pub fn from_setting(setting: Option<&str>) -> Result<Self, BackendSpecRefusal> {
        match setting.map(str::trim) {
            None | Some("") | Some("local") => Ok(Self::default()),
            Some(json) => serde_json::from_str::<Self>(json).map_err(|e| {
                BackendSpecRefusal::UnknownVariant {
                    variant: e.to_string(),
                }
            }),
        }
    }

    /// Admission for opening: validate. Both variants have an open path
    /// (`local` is redb, `object_store` is the served object-store open).
    pub fn admit(&self) -> Result<(), BackendSpecRefusal> {
        self.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ENGINE-PARITY-R019.2
    #[test]
    fn outside_database_endpoint_schemes_refuse_by_name() {
        let spec = StorageBackendSpec::default();
        for scheme in EXTERNAL_DATABASE_SCHEMES {
            let url = format!("{}://host/db", scheme.to_ascii_uppercase());
            let refusal = spec.check_endpoint_scheme(&url).unwrap_err();
            assert_eq!(
                refusal,
                BackendSpecRefusal::ExternalPlacementForbidden {
                    scheme: scheme.to_string()
                }
            );
            assert_eq!(refusal.wire_code(), Some(EXTERNAL_PLACEMENT_FORBIDDEN));
        }
        assert!(spec
            .check_endpoint_scheme("jdbc:postgresql://h/db")
            .is_err());
        assert!(spec.check_endpoint_scheme("https://s3.example").is_ok());
        let mut with_url = good();
        if let StorageBackendSpec::ObjectStore { endpoint, .. } = &mut with_url {
            *endpoint = Some(SecretRef("postgres://user@host/db".into()));
        }
        assert_eq!(
            with_url.validate().unwrap_err(),
            BackendSpecRefusal::ExternalPlacementForbidden {
                scheme: "postgres".into()
            }
        );
        assert!(good().validate().is_ok());
    }

    fn good() -> StorageBackendSpec {
        StorageBackendSpec::ObjectStore {
            endpoint: Some(SecretRef("bao:eg/endpoint".into())),
            bucket: Some(SecretRef("bao:eg/bucket".into())),
            root: Some(SecretRef("bao:eg/root".into())),
            durability: Some(DurabilityClass::ObjectStoreConditionalPut),
            layout_digest: Some(pinned_layout_digest(ALL_LAYOUTS[0]).to_string()),
        }
    }

    #[test]
    fn default_is_local_and_admits() {
        assert_eq!(StorageBackendSpec::default(), StorageBackendSpec::Local);
        assert_eq!(StorageBackendSpec::Local.admit(), Ok(()));
    }

    // spec: MEDULLA-OBJECT-STORAGE-BACKEND-R001
    #[test]
    fn both_variants_round_trip_over_the_wire() {
        for spec in [StorageBackendSpec::Local, good()] {
            assert_eq!(
                StorageBackendSpec::from_wire(&spec.to_wire()).unwrap(),
                spec
            );
        }
    }

    // spec: MEDULLA-OBJECT-STORAGE-BACKEND-R001
    #[test]
    fn refusals_are_typed_and_pure() {
        assert_eq!(good().validate(), Ok(()));
        let mut spec = good();
        let before = spec.clone();
        if let StorageBackendSpec::ObjectStore { bucket, .. } = &mut spec {
            *bucket = None;
        }
        assert_eq!(
            spec.validate(),
            Err(BackendSpecRefusal::MissingSecretRef {
                field: "bucket".into()
            })
        );
        assert_ne!(spec, before);
        let mut spec = good();
        if let StorageBackendSpec::ObjectStore { durability, .. } = &mut spec {
            *durability = None;
        }
        assert_eq!(
            spec.validate(),
            Err(BackendSpecRefusal::MissingDurabilityClass)
        );
        let mut spec = good();
        if let StorageBackendSpec::ObjectStore { layout_digest, .. } = &mut spec {
            *layout_digest = Some("00".into());
        }
        assert!(matches!(
            spec.validate(),
            Err(BackendSpecRefusal::LayoutNotInLineage { .. })
        ));
        #[derive(serde::Serialize)]
        struct Tape {
            variant: &'static str,
        }
        let tape = rmp_serde::to_vec_named(&Tape { variant: "tape" }).unwrap();
        assert!(matches!(
            StorageBackendSpec::from_wire(&tape),
            Err(BackendSpecRefusal::UnknownVariant { .. })
        ));
        assert!(good().admit().is_ok());
        assert_eq!(
            StorageBackendSpec::from_setting(Some("local")),
            Ok(StorageBackendSpec::Local)
        );
        assert!(matches!(
            StorageBackendSpec::from_setting(Some("{")),
            Err(BackendSpecRefusal::UnknownVariant { .. })
        ));
    }
}
