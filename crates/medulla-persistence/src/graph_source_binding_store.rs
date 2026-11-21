//! The durable registry of graph-sourced semantic bindings (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! One kernel-owned owner file, `graph_source_bindings.redb`, of layout
//! `OwnerLayout::GraphSourceBindings`, modelled on the scheduler policy store. One row per
//! graph-sourced binding, keyed `(tenant, binding_id)` and valued with the JSON of the binding's
//! graph text property selector. The bound-source registry the graph commit chokepoint reads is
//! rebuilt from this table at startup. This type is the only caller of the
//! `graph_source_bindings` table.

use std::path::Path;
use std::sync::Arc;

use redb::ReadableTable;

/// The owner file name under the persist dir.
pub const GRAPH_SOURCE_BINDINGS_FILE: &str = "graph_source_bindings.redb";
/// The physical authority of [`GRAPH_SOURCE_BINDINGS_FILE`].
pub const GRAPH_SOURCE_BINDINGS_PHYSICAL_STORE: &str = "epistemic-graph:graph-source-bindings";
const GRAPH_SOURCE_BINDINGS_SCOPE_RESOURCE: &str = "graph-source-bindings";
const GRAPH_SOURCE_BINDINGS_SCOPE_INCARNATION: &str = "graph-source-bindings:v1";

type Durable =
    medulla_store::sidecar_store::SidecarStore<medulla_storage::GraphSourceBindingsOwner>;

/// One stored binding: its tenant, its id and the selector JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSourceBindingRow {
    pub tenant: String,
    pub binding_id: String,
    pub selector_json: String,
}

/// The graph-sourced binding registry owner file.
pub struct GraphSourceBindingStore {
    durable: Durable,
}

impl std::fmt::Debug for GraphSourceBindingStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphSourceBindingStore")
            .finish_non_exhaustive()
    }
}

impl GraphSourceBindingStore {
    /// Open (or create) `graph_source_bindings.redb` under `persist_dir`.
    pub fn open(persist_dir: &str) -> Result<Self, String> {
        let path = Path::new(persist_dir).join(GRAPH_SOURCE_BINDINGS_FILE);
        let durable = Durable::open(
            &path,
            GRAPH_SOURCE_BINDINGS_PHYSICAL_STORE,
            GRAPH_SOURCE_BINDINGS_SCOPE_RESOURCE,
            GRAPH_SOURCE_BINDINGS_SCOPE_INCARNATION,
            medulla_kernel::store_authority::process_authority(),
        )?;
        Ok(Self { durable })
    }

    /// Every stored binding, ordered by tenant then binding id.
    pub fn list(&self) -> Result<Vec<GraphSourceBindingRow>, String> {
        let read = self.durable.read()?;
        let table = read.open_owner_table(medulla_storage::GRAPH_SOURCE_BINDINGS)?;
        let mut rows = Vec::new();
        for row in table.iter().map_err(|error| error.to_string())? {
            let (key, value) = row.map_err(|error| error.to_string())?;
            let (tenant, binding_id) = key.value();
            rows.push(GraphSourceBindingRow {
                tenant: tenant.to_string(),
                binding_id: binding_id.to_string(),
                selector_json: String::from_utf8(value.value().to_vec())
                    .map_err(|_| "a stored graph source selector is not text".to_string())?,
            });
        }
        Ok(rows)
    }

    /// Record the selector of `(tenant, binding_id)`. Idempotent: a row already holding
    /// `selector_json` writes nothing.
    pub fn put(&self, tenant: &str, binding_id: &str, selector_json: &str) -> Result<(), String> {
        {
            let read = self.durable.read()?;
            let table = read.open_owner_table(medulla_storage::GRAPH_SOURCE_BINDINGS)?;
            let stored = table
                .get((tenant, binding_id))
                .map_err(|error| error.to_string())?;
            if stored.is_some_and(|value| value.value() == selector_json.as_bytes()) {
                return Ok(());
            }
        }
        self.durable.maintain("graph_source_binding_put", |owner| {
            let mut table = owner.open_table(medulla_storage::GRAPH_SOURCE_BINDINGS)?;
            table
                .insert((tenant, binding_id), selector_json.as_bytes())
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }
}

/// Open (once) the shared registry handle `ServerState` retains, under the configured persist
/// directory. redb admits one opener per file.
pub fn shared_graph_source_binding_store(
    state: &medulla_server_api::state::ServerState,
) -> Result<Arc<GraphSourceBindingStore>, String> {
    let mut guard = state.graph_source_binding_store.lock();
    if let Some(store) = guard
        .as_ref()
        .and_then(|handle| Arc::clone(handle).downcast::<GraphSourceBindingStore>().ok())
    {
        return Ok(store);
    }
    let persist_dir = state.persist_dir.as_deref().ok_or_else(|| {
        "the graph source binding registry requires a configured persist directory".to_string()
    })?;
    let store = Arc::new(GraphSourceBindingStore::open(persist_dir)?);
    *guard = Some(Arc::clone(&store) as medulla_server_api::state::ErasedHandle);
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-MEDIA-VOCAB-R001.102.2
    #[test]
    fn bindings_survive_reopen_and_a_repeated_put_writes_one_row() {
        let dir = tempfile::tempdir().unwrap();
        let persist = dir.path().to_str().unwrap();
        {
            let store = GraphSourceBindingStore::open(persist).unwrap();
            assert!(store.list().unwrap().is_empty());
            store.put("t", "bookmark", "{\"a\":1}").unwrap();
            store.put("t", "bookmark", "{\"a\":1}").unwrap();
            store.put("t", "other", "{\"a\":2}").unwrap();
        }
        let store = GraphSourceBindingStore::open(persist).unwrap();
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].binding_id, "bookmark");
        assert_eq!(rows[1].selector_json, "{\"a\":2}");
    }
}
