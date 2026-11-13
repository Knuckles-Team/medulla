//! `ConceptLineage`: known concept ids and their `superseded_by` edges
//! (MEDULLA-DURABLE-KERNEL-R094.7 and R094.7.1).
//!
//! The handler binds the caller's context to the verified request, then hands
//! the claim or edge to the Agent Library store, where `ConceptPathSet::claim`
//! and `ConceptLineage::add_edge` decide it and the admitted pack write commits
//! the rows. Reads come from one committed snapshot.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_agent_types::concept_lineage::ConceptLineageOp;
use medulla_persistence_records::agent_library::context::bind_agent_library_context;
use medulla_persistence_records::agent_library::shared_agent_library;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_protocol_types::result_contract::storage::{
    ConceptLineageAddEdge, ConceptLineageClaim, ConceptLineageRead,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// Serve one `ConceptLineage` operation.
pub async fn handle(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: ConceptLineageOp,
) -> Response {
    if let Err(error) = op.validate() {
        return Response::err(req_id, error);
    }
    if op.tenant_id() != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: concept lineage tenant must match verified request tenant",
        );
    }
    let store = match shared_agent_library(state).await {
        Ok(store) => store,
        Err(error) => return Response::err(req_id, error),
    };
    match op {
        ConceptLineageOp::Claim {
            context,
            concept_id,
            repository,
        } => {
            let context = match bind_agent_library_context(
                &store,
                req_id,
                verified,
                context,
                "concept-lineage:claim",
                true,
            ) {
                Ok(context) => context,
                Err(error) => return Response::err(req_id, error),
            };
            match store.claim_concept(&context, &concept_id, &repository) {
                Ok(record) => Response::ok(
                    req_id,
                    ResultPayload::of_ref::<ConceptLineageClaim>(&record),
                ),
                Err(error) => Response::err(req_id, error),
            }
        }
        ConceptLineageOp::AddEdge {
            context,
            superseded,
            successor,
            repository,
        } => {
            let context = match bind_agent_library_context(
                &store,
                req_id,
                verified,
                context,
                "concept-lineage:add-edge",
                true,
            ) {
                Ok(context) => context,
                Err(error) => return Response::err(req_id, error),
            };
            match store.add_concept_edge(&context, &superseded, &successor, &repository) {
                Ok(record) => Response::ok(
                    req_id,
                    ResultPayload::of_ref::<ConceptLineageAddEdge>(&record),
                ),
                Err(error) => Response::err(req_id, error),
            }
        }
        ConceptLineageOp::Read {
            tenant_id,
            concept_id,
        } => match store.read_concept(&tenant_id, &concept_id) {
            Ok(result) => {
                Response::ok(req_id, ResultPayload::of_ref::<ConceptLineageRead>(&result))
            }
            Err(error) => Response::err(req_id, error),
        },
    }
}
