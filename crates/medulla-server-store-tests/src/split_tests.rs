//! Modules moved here from `medulla_server::split_tests` (hard cutover, no re-exports).

#[cfg(all(test, feature = "redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod admin_agent_library_security;
#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod impact_watch;
#[cfg(all(test, feature = "redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod mutation_batch_internal_replay;
#[cfg(all(test, feature = "redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod mutation_batch_lifecycle;
#[cfg(all(test, feature = "redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod mutation_redb;
#[cfg(all(test, feature = "redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod persistence_txn;
#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod server_api_mutation_inventory;
#[cfg(all(test, feature = "redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod server_api_saga;
#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod store_run_owner;
#[cfg(all(test, any(feature = "server", feature = "redb")))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod store_terminal_batch;
