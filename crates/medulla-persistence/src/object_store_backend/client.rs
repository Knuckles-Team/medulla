//! The object-store client, built from the spec's secret references
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R002.2).
//!
//! Each [`SecretRef`](medulla_storage::SecretRef) of an `object_store` spec
//! names one environment variable of the engine process; its value is the
//! secret. The endpoint, bucket and root resolve first, the endpoint scheme is
//! checked, and only then is the client built, so every refusal happens before
//! any request to the store. The client is the same `object_store` S3 builder
//! the blob store uses; no second client type exists.

use std::sync::Arc;

use medulla_storage::{SecretRef, StorageBackendSpec};
use object_store::aws::{AmazonS3Builder, S3ConditionalPut};
use object_store::ObjectStore;

use super::ObjectStoreOpenRefusal;

/// Resolve one secret reference from the process environment. An unset or
/// empty variable refuses with the field name; the value is never echoed.
fn resolve_secret(
    field: &'static str,
    reference: &Option<SecretRef>,
) -> Result<String, ObjectStoreOpenRefusal> {
    let Some(SecretRef(name)) = reference else {
        return Err(ObjectStoreOpenRefusal::SecretUnresolved { field });
    };
    match std::env::var(name.trim()) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ObjectStoreOpenRefusal::SecretUnresolved { field }),
    }
}

/// The scheme and host of an `http://` or `https://` endpoint.
fn scheme_and_host(endpoint: &str) -> Option<(String, String)> {
    let (scheme, rest) = endpoint.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    Some((scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// Whether `host` names the loopback interface.
fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}

/// Build the object-store client of an `object_store` spec and return it with
/// the root key prefix. Refusals: `OBJECT_STORE_SECRET_UNRESOLVED`,
/// `EXTERNAL_PLACEMENT_FORBIDDEN`, `OBJECT_STORE_PLAIN_HTTP_REFUSED` and
/// `OBJECT_STORE_CLIENT_INVALID`, each before any request to the store.
pub fn resolve_object_store_client(
    spec: &StorageBackendSpec,
) -> Result<(Arc<dyn ObjectStore>, String), ObjectStoreOpenRefusal> {
    let StorageBackendSpec::ObjectStore {
        endpoint,
        bucket,
        root,
        ..
    } = spec
    else {
        return Err(ObjectStoreOpenRefusal::WrongVariant);
    };
    let endpoint = resolve_secret("endpoint", endpoint)?;
    let bucket = resolve_secret("bucket", bucket)?;
    let root = resolve_secret("root", root)?;
    spec.check_endpoint_scheme(&endpoint)
        .map_err(ObjectStoreOpenRefusal::Spec)?;
    let (scheme, host) = scheme_and_host(&endpoint).ok_or_else(|| {
        ObjectStoreOpenRefusal::ClientBuild {
            reason: "endpoint is not an http or https url".to_string(),
        }
    })?;
    let allow_http = match scheme.as_str() {
        "https" => false,
        "http" if is_loopback_host(&host) => true,
        "http" => return Err(ObjectStoreOpenRefusal::PlainHttpNotLoopback),
        _ => {
            return Err(ObjectStoreOpenRefusal::ClientBuild {
                reason: "endpoint is not an http or https url".to_string(),
            })
        }
    };
    let client = AmazonS3Builder::from_env()
        .with_endpoint(endpoint)
        .with_bucket_name(bucket)
        .with_virtual_hosted_style_request(false)
        .with_conditional_put(S3ConditionalPut::ETagMatch)
        .with_allow_http(allow_http)
        .build()
        .map_err(|error| ObjectStoreOpenRefusal::ClientBuild {
            reason: error.to_string(),
        })?;
    Ok((Arc::new(client), root))
}
