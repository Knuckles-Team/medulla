//! Modules moved here from `medulla_server::dispatch` (hard cutover, no re-exports).

// GENERATED-USES: the imports the former dispatch root carried; child modules reach them through `use super::*`.

use std::ops::ControlFlow;
use std::sync::Arc;
#[cfg(feature = "ast")]
use std::sync::OnceLock;
use std::time::Instant as DispatchLockInstant;
use tokio::sync::RwLock;
use tracing::info;

use medulla_server_auth::access::check_caller_is_known;
use medulla_server_auth::access::check_graph_access;
use medulla_server_auth::access::is_admin_authz_action;
use medulla_server_auth::access::require_admin_capability;
use medulla_server_auth::access::requires_write;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_auth::auth::verify_multisig_mutation_signatures;
use medulla_server_auth::auth::verify_register_identity_signature;
use medulla_server_api::authority_context::VerifiedRequestContext;
// Only the ast-gated ParseFiles handler offloads to the blocking pool here; the
// graph-op off-lock sites live in handlers/graph_ops.rs.
use crate::handlers;
#[cfg(feature = "ast")]
use medulla_server_api::compute::compute_off_lock;
// NOT `#[cfg(feature = "redb")]`: `server::persistence` is an unconditional module
// (server/mod.rs) and three ungated signatures below -- `reconcile_existing_graph_create`,
// `read_committed_graph_version`, `reconcile_missing_graph_delete` -- name this trait
// unqualified. Gating the import broke every build without `redb`
// (`--no-default-features --features server`) with E0405 while the default full build,
// which enables `redb`, stayed green. See plans/complex/lane-reports/WD10-R-DISPATCH.md.
use medulla_isolation::isolation::AccessLevel;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Request;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::state::{timed_write, ServerState};

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod change_envelope;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod change_envelope_draft;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph_pipeline;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod policy_evolution;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod shared;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sparql_update;

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod request_boundary;
