use super::graph_pipeline::dispatch_graph_op;
#[cfg(feature = "raft")]
use super::request_boundary::dispatch_with_context;
use super::*;

mod fleet_catalog;
#[cfg(all(feature = "raft", feature = "jobs"))]
mod publication;
mod registry;
mod replicated;
#[cfg(feature = "raft")]
mod routing;
#[cfg(feature = "raft")]
mod sanitization;
#[cfg(feature = "raft")]
mod transaction;

pub(super) use fleet_catalog::handle_fleet_catalog;
#[cfg(all(feature = "raft", feature = "jobs"))]
use publication::{execute_consensus_job_publication, JobPublicationExecution};
#[cfg(feature = "redb")]
pub(crate) use registry::verified_served_registration;
pub(super) use registry::{
    handle_list_registered_servers, handle_register_server, ServerRegistration,
};
#[cfg(feature = "raft")]
use replicated::capability_authority_unavailable;
#[cfg(not(feature = "raft"))]
pub(super) use replicated::replicated_identity_bootstrap_authorized;
#[cfg(feature = "raft")]
pub(super) use replicated::replicated_identity_bootstrap_authorized;
#[cfg(feature = "raft")]
pub(super) use routing::propose_native_mutation;
#[cfg(feature = "raft")]
use sanitization::sanitize_native_proposal;
#[cfg(feature = "raft")]
use transaction::{execute_consensus_transaction, TransactionExecution};

#[cfg(feature = "raft")]
use medulla_kernel::kernel::dispatch_support::ReplicatedApplyScope;
use medulla_kernel::kernel::dispatch_support::REPLICATED_APPLY;

#[cfg(all(feature = "raft", feature = "jobs"))]
pub(crate) use replicated::{
    apply_replicated_job_publication_commit, apply_replicated_job_publication_finalize,
};
#[cfg(feature = "raft")]
pub(crate) use replicated::{
    apply_replicated_native, apply_replicated_transaction_decision,
    apply_replicated_transaction_finalize, apply_replicated_transaction_participant,
    apply_replicated_transaction_prepare,
};
