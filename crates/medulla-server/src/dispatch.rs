//! The request dispatch shell: authentication, service-level methods, and the
//! graph-operation routing chain. Per-domain mutation kernels own their atomic
//! durability, audit, CDC, and projection publication.


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
use medulla_server_graph::handlers;
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


#[cfg(test)]
pub mod dispatch_e2e_tests;

pub mod request_boundary;
pub mod router;
pub mod seam;
#[cfg(test)]
pub mod test_support;

// EH-408 / EH-409: stored telemetry → ontology-bound facts. Needs the
// observability store (`obs`) and the schema reasoner (`shacl`).
#[cfg(all(feature = "obs", feature = "shacl"))]
pub mod telemetry;

