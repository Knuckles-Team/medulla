//! The durable registry of graph-sourced semantic bindings (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! The graph commit chokepoint decides which commits wake a semantic index from the
//! bound-source registry of `medulla-index-types`, which lives in memory. The registry's durable
//! form is the owner table `graph_source_bindings` of the engine store (`medulla-persistence`'s
//! `GraphSourceBindingStore`), one row per graph-sourced binding keyed by tenant and binding id
//! and holding the binding's selector. [`record_graph_source_binding`] writes the row when a
//! binding is first seen and [`rebuild_graph_source_registry`] reads every row back into the
//! in-memory registry at startup, so a commit made before the first semantic operation of a
//! restarted process still wakes its index.

use medulla_index_types::semantic_index::{bind_graph_source, GraphTextPropertyRef};
use medulla_persistence::graph_source_binding_store::shared_graph_source_binding_store;
use medulla_server_api::state::ServerState;

/// Record that `binding_id` of `tenant` is built from `selector` in the owner table.
/// Idempotent: a row already holding the selector writes nothing.
pub(crate) fn record_graph_source_binding(
    state: &ServerState,
    tenant: &str,
    binding_id: &str,
    selector: &GraphTextPropertyRef,
) -> Result<(), String> {
    let store = shared_graph_source_binding_store(state)?;
    let encoded = serde_json::to_string(selector)
        .map_err(|_| "graph source selector is unencodable".to_string())?;
    store.put(tenant, binding_id, &encoded)
}

/// Register every stored graph-sourced binding with the bound-source registry. Returns how many
/// bindings were registered. A row whose selector no longer decodes is a hard error: the table is
/// the authority. Run at startup, after the server state is composed.
pub fn rebuild_graph_source_registry(state: &ServerState) -> Result<usize, String> {
    let store = shared_graph_source_binding_store(state)?;
    let mut registered = 0;
    for row in store.list()? {
        let selector: GraphTextPropertyRef = serde_json::from_str(&row.selector_json)
            .map_err(|_| "a stored graph source selector is unreadable".to_string())?;
        bind_graph_source(&row.tenant, &selector);
        registered += 1;
    }
    Ok(registered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_index_types::semantic_index::bound_graph_sources;

    fn state_over(dir: &tempfile::TempDir) -> ServerState {
        let mut state = ServerState::new_for_test("registry-secret", ServerState::test_isolation("admin"));
        state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        state
    }

    fn selector(graph: &str) -> GraphTextPropertyRef {
        GraphTextPropertyRef {
            graph_id: graph.into(),
            type_id: "Bookmark".into(),
            property_id: "text".into(),
        }
    }

    // spec: MEDULLA-MEDIA-VOCAB-R001.102.2
    #[test]
    fn a_restarted_process_rebuilds_the_bound_source_registry_from_the_owner_table() {
        let dir = tempfile::tempdir().unwrap();
        let tenant = "restart-registry-tenant";
        let graph = "restart-registry-graph";
        {
            let state = state_over(&dir);
            record_graph_source_binding(&state, tenant, "bookmark", &selector(graph)).unwrap();
            // Recording twice leaves one row.
            record_graph_source_binding(&state, tenant, "bookmark", &selector(graph)).unwrap();
            let store = shared_graph_source_binding_store(&state).unwrap();
            assert_eq!(store.list().unwrap().len(), 1);
        }
        // The new process starts with an empty in-memory registry; only the owner table holds
        // the binding.
        assert!(bound_graph_sources(tenant, graph).is_empty());
        let restarted = state_over(&dir);
        assert_eq!(rebuild_graph_source_registry(&restarted).unwrap(), 1);
        assert_eq!(
            bound_graph_sources(tenant, graph),
            vec![("Bookmark".to_string(), "text".to_string())]
        );
    }

    // spec: MEDULLA-MEDIA-VOCAB-R001.102.2
    #[test]
    fn an_empty_owner_table_rebuilds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(rebuild_graph_source_registry(&state_over(&dir)).unwrap(), 0);
    }
}
