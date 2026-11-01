use super::trait_capabilities::persistence_capabilities;
use super::trait_envelopes::persistence_envelopes;
use super::trait_graph::persistence_graph;
use super::trait_graph_reads::persistence_graph_reads;
use super::trait_mutations::persistence_mutations;
use super::trait_native::persistence_native;
use super::trait_outbox::persistence_outbox;
use super::*;

use std::sync::Arc;
use tokio::sync::{oneshot, RwLock};

use crate::writer_reply::await_writer_reply;
use medulla_mutation_types::mutation_scope::MutationOutboxLease;
use medulla_mutation_types::mutation_scope::MutationOutboxRecord;
use medulla_mutation_types::mutation_scope::MutationProjectionCursor;
use medulla_protocol_types::change_envelope::ChangeCursor;
use medulla_protocol_types::change_envelope::ChangeEnvelope;
use medulla_protocol_types::change_envelope::ChangeEnvelopeCommit;
use medulla_protocol_types::change_envelope::ChangeEnvelopeRecord;
use medulla_protocol_types::change_envelope::ContentVersion;
use medulla_protocol_types::mutation_batch::MutationBatch;
use medulla_protocol_types::mutation_batch::MutationBatchCommit;
use medulla_protocol_types::mutation_batch::MutationBatchRecord;
use medulla_protocol_types::protocol::GraphType;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::state::ServerState;
use medulla_store::redb_store::durable_node_presence as read_durable_node_presence;
use medulla_store::redb_store::read_change_cursor as read_change_cursor_record;
use medulla_store::redb_store::read_change_envelope as read_change_envelope_record;
use medulla_store::redb_store::read_content_version as read_content_version_record;
use medulla_store::redb_store::read_mutation_batch_for_graph as read_mutation_batch_record;
use medulla_store::redb_store::read_mutation_graph_version as read_mutation_graph_version_record;
use medulla_store::redb_store::read_mutation_outbox as read_mutation_outbox_records;
use medulla_store::redb_store::read_one_node;
use medulla_store::redb_store::read_resource_reservation as read_resource_reservation_record;
use medulla_store::redb_store::read_resource_reservation_status as read_resource_reservation_status_record;
use medulla_transaction::{OutboxClaimBudget, OutboxClaimOutcome};

// `async_trait` must see the concrete methods when it expands. Invoking method
// macros directly inside its annotated impl leaves those methods untransformed,
// so their lifetimes no longer match the erased trait signatures. Each split
// fragment appends its methods to this token accumulator; only the final callback
// emits the annotated impl.
macro_rules! with_capabilities {
    ($($methods:tt)*) => {
        persistence_graph_reads!(with_graph_reads { $($methods)* });
    };
}

macro_rules! with_graph_reads {
    ($($methods:tt)*) => {
        persistence_mutations!(with_mutations { $($methods)* });
    };
}

macro_rules! with_mutations {
    ($($methods:tt)*) => {
        persistence_outbox!(with_outbox { $($methods)* });
    };
}

macro_rules! with_outbox {
    ($($methods:tt)*) => {
        persistence_envelopes!(with_envelopes { $($methods)* });
    };
}

macro_rules! with_envelopes {
    ($($methods:tt)*) => {
        persistence_native!(with_native { $($methods)* });
    };
}

macro_rules! with_native {
    ($($methods:tt)*) => {
        persistence_graph!(finish_persistence_impl { $($methods)* });
    };
}

macro_rules! finish_persistence_impl {
    ($($methods:tt)*) => {
        #[async_trait::async_trait]
        impl PersistenceBackend for RedbBackend {
            $($methods)*
        }
    };
}

persistence_capabilities!(with_capabilities {});
