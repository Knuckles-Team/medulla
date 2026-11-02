//! Shared server and feature-gated imports for private analytics-job modules.

#[cfg(feature = "program-optimization")]
pub(super) use medulla_modality::{Classification, OpaqueRef, PolicyEnvelope};
#[cfg(feature = "program-optimization")]
pub(super) use medulla_program::{
    NativeCompiler, OptimizationRequest, ProgramModality, ProgramRevisionIdentity,
};

pub(super) use medulla_isolation::isolation::AccessLevel;
pub(super) use medulla_protocol_types::protocol::Method;
pub(super) use medulla_protocol_types::protocol::Response;
pub(super) use medulla_protocol_types::protocol::ResultPayload;
pub(super) use medulla_server_auth::access::check_graph_access;
pub(super) use medulla_server_api::state::ServerState;
