//! `Method::ListPathGrants` (MEDULLA-SCM-R007.1): read the path grants of one
//! principal in the verified tenant.
//!
//! The grants are the branch registry rows `SetPathGrant` writes, so this read
//! and the write share one authority. The tenant is always the verified tenant.
//! The caller's own grants need the read scope the method is admitted under;
//! another principal's grants need `graph:admin`. A principal with no grants
//! answers an empty list, whether the tenant never heard of it or it holds
//! nothing, so the answer is not an existence oracle. Reads only.

use super::*;

/// The wire field of `Method::ListPathGrants`.
pub struct ListPathGrantsFields {
    pub request: Box<medulla_source_types::source_path_grant::ListPathGrantsRequest>,
}

#[cfg(feature = "redb")]
mod served {
    use super::*;
    use medulla_persistence::branch_registry_store::shared_branch_registry;
    use medulla_source_types::source_path_grant::{PathGrantList, PathGrantRefusal};

    /// Whether `verified` may read the grants of `principal`: its own always, anyone
    /// else's only with the grant administration authority `SetPathGrant` needs.
    fn may_read(verified: &VerifiedRequestContext, principal: &str) -> bool {
        principal == verified.agent_id() || verified.allows_method("graph:admin", true)
    }

    fn receipt_response(req_id: u64, list: PathGrantList) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::ListPathGrants>(
            list,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Serve `Method::ListPathGrants`.
    pub async fn handle_list_path_grants(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: ListPathGrantsFields,
    ) -> Response {
        let request = fields.request;
        if let Err(field) = request.validate() {
            return Response::err(
                req.id,
                format!("{}: {field}", PathGrantRefusal::Invalid.code()),
            );
        }
        if !may_read(verified, &request.principal) {
            return Response::err(
                req.id,
                "ACCESS_DENIED: another principal's path grants need graph:admin",
            );
        }
        let registry = {
            let s = state.read().await;
            match shared_branch_registry(&s) {
                Ok(registry) => registry,
                Err(error) => return Response::err(req.id, error),
            }
        };
        let tenant = verified.tenant().to_string();
        match tokio::task::spawn_blocking(move || {
            registry.principal_path_grants(&tenant, &request.principal, request.repo.as_deref())
        })
        .await
        {
            Ok(Ok(grants)) => receipt_response(req.id, PathGrantList { grants }),
            Ok(Err(error)) => Response::err(req.id, error),
            Err(error) => Response::err(req.id, error.to_string()),
        }
    }
}

#[cfg(feature = "redb")]
pub use served::handle_list_path_grants;

/// `ListPathGrants` needs the `redb` registry owner file.
#[cfg(not(feature = "redb"))]
pub async fn handle_list_path_grants(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: ListPathGrantsFields,
) -> Response {
    Response::err(req.id, "ListPathGrants requires the `redb` feature")
}
