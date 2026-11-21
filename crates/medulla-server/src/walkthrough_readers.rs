//! Production walkthrough readers (MEDULLA-ORCH-VOCAB-R013.7.5).
//!
//! `medulla-server` is the one crate that sees every handler crate, so the two read seams of
//! `medulla_server_api::walkthrough_readers` get their production implementations here and are
//! installed once by `seams_install::install_seams`. Each reader holds a clone of the server
//! state and takes its read lock only for the duration of one read; the reader traits stay
//! synchronous. Every read is scoped by tenant: another tenant's repository or blob reads as
//! absent.

use std::sync::Arc;

use medulla_server_api::state::ServerState;
use medulla_server_api::walkthrough_readers::{BlobReader, RepositoryIndexReader};
use medulla_types::contract::Digest256;
use tokio::sync::RwLock;

/// The largest walkthrough artifact a reader loads into memory.
pub const WALKTHROUGH_ARTIFACT_MAX_BYTES: usize = 16 * 1024 * 1024;

const BRANCH_PREFIX: &str = "branch:";

/// Run `read` under the state's read lock, taken with `try_read` for this one read. The reader
/// traits are synchronous and run inside the async `AddNode` path, where a blocking lock panics,
/// so a lock that is not available reads as absent: the check fails closed
/// (`WALKTHROUGH_REVISION_STALE`) and the caller retries.
fn with_state<T>(
    state: &Arc<RwLock<ServerState>>,
    read: impl FnOnce(&ServerState) -> Option<T>,
) -> Option<T> {
    state.try_read().ok().and_then(|guard| read(&guard))
}

/// Reads an artifact body from the engine blob chunk store, owned by the tenant.
pub struct ServerBlobReader {
    state: Arc<RwLock<ServerState>>,
}

impl ServerBlobReader {
    /// A reader over the server state.
    pub fn new(state: Arc<RwLock<ServerState>>) -> Self {
        Self { state }
    }
}

impl BlobReader for ServerBlobReader {
    fn read(&self, tenant: &str, artifact_ref: &str) -> Option<Vec<u8>> {
        #[cfg(feature = "blob")]
        {
            use medulla_server_blob::blob::BlobStateExt;
            let store = with_state(&self.state, |state| {
                state
                    .blob_cursors()
                    .map(|cursors| Arc::clone(&cursors.store))
            })?;
            medulla_server_blob::blob::engine_bodies::read_owned_blob(
                store.as_ref(),
                tenant,
                artifact_ref,
                WALKTHROUGH_ARTIFACT_MAX_BYTES,
            )
            .ok()
        }
        #[cfg(not(feature = "blob"))]
        {
            let _ = (&self.state, tenant, artifact_ref);
            None
        }
    }
}

/// Reads the committed repository index of the tenant's graphs.
pub struct GraphIndexReader {
    state: Arc<RwLock<ServerState>>,
}

impl GraphIndexReader {
    /// A reader over the server state.
    pub fn new(state: Arc<RwLock<ServerState>>) -> Self {
        Self { state }
    }

    /// Run `read` over each resident graph of the tenant (`tenant__<tenant>` and every
    /// `tenant__<tenant>__<base>`), in name order, and return the first answer; a tenant id that
    /// is not valid or a tenant with no graph reads as absent.
    fn with_tenant_graph<T>(
        &self,
        tenant: &str,
        read: impl Fn(&medulla_graph_core::graph::GraphCore) -> Option<T>,
    ) -> Option<T> {
        let tenant = medulla_identity_types::tenancy::TenantId::new(tenant).ok()?;
        let whole = medulla_identity_types::tenancy::tenant_graph(&tenant);
        let prefix = format!("{whole}__");
        with_state(&self.state, |state| {
            let mut names: Vec<String> = state
                .registry
                .list()
                .into_iter()
                .map(|(name, _)| name)
                .filter(|name| *name == whole || name.starts_with(&prefix))
                .collect();
            names.sort();
            names.iter().find_map(|name| {
                state
                    .registry
                    .get(name)
                    .and_then(|entry| read(&entry.core))
            })
        })
    }
}

fn properties_of(
    core: &medulla_graph_core::graph::GraphCore,
    node_id: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let blob = core.node_properties.get(node_id)?;
    match medulla_types::msgpack::decode_property_value(&blob) {
        Ok(serde_json::Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn text<'a>(map: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> &'a str {
    map.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

impl RepositoryIndexReader for GraphIndexReader {
    /// The revision of the live ref of `repository_id` that the current complete generation
    /// advanced to: the first live ref in ref-name order.
    fn current_revision(&self, tenant: &str, repository_id: &str) -> Option<String> {
        self.with_tenant_graph(tenant, |core| {
            let mut live: Vec<(String, String)> = core
                .node_properties
                .iter()
                .filter(|entry| entry.key().starts_with(BRANCH_PREFIX))
                .filter_map(|entry| {
                    let props = properties_of(core, entry.key())?;
                    (text(&props, "type") == "Branch"
                        && text(&props, "repository_id") == repository_id
                        && text(&props, "status") == "live")
                        .then(|| {
                            (
                                text(&props, "ref_name").to_string(),
                                text(&props, "revision_id").to_string(),
                            )
                        })
                })
                .collect();
            live.sort();
            live.into_iter().next().map(|(_, revision)| revision)
        })
    }

    fn snapshot_contains(
        &self,
        tenant: &str,
        repository_id: &str,
        repository_revision: &str,
        entity_id: &str,
        source_revision: &str,
        source_digest: &Digest256,
    ) -> bool {
        if self.current_revision(tenant, repository_id).as_deref() != Some(repository_revision) {
            return false;
        }
        let digest = source_digest.to_hex();
        self.with_tenant_graph(tenant, |core| {
            let props = properties_of(core, entity_id)?;
            Some(
                text(&props, "source_revision") == source_revision
                    && text(&props, "source_digest") == digest,
            )
        })
        .unwrap_or(false)
    }
}
