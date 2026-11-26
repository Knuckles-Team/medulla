//! `Method::SetPathGrant` (MEDULLA-SCM-R007): set one principal's read and write
//! access to the paths of a repository under a prefix.
//!
//! The grant row is written into the branch registry through the admitted
//! native mutation `path_grant`; the tenant is always the verified tenant. A
//! grant with both bits false is stored as an explicit deny and never deleted, so
//! it still wins as the longest matching prefix. Source methods deny by default,
//! so a repository is unreadable and unwritable until a grant exists.

use super::*;

/// The wire field of `Method::SetPathGrant`.
pub struct SetPathGrantFields {
    pub request: Box<medulla_source_types::source_path_grant::SetPathGrantRequest>,
}

#[cfg(feature = "redb")]
mod served {
    use super::*;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_persistence::branch_registry_store::shared_branch_registry;
    use medulla_source_types::source_path_grant::{PathGrant, PathGrantRefusal};

    fn receipt_response(req_id: u64, grant: PathGrant) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::SetPathGrant>(
            grant,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Serve `Method::SetPathGrant`.
    pub async fn handle_set_path_grant(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: SetPathGrantFields,
    ) -> Response {
        let grant = match fields.request.validate() {
            Ok(grant) => grant,
            Err(field) => {
                return Response::err(
                    req.id,
                    format!("{}: {field}", PathGrantRefusal::Invalid.code()),
                )
            }
        };
        let registry = {
            let s = state.read().await;
            match shared_branch_registry(&s) {
                Ok(registry) => registry,
                Err(error) => return Response::err(req.id, error),
            }
        };
        let tenant = verified.tenant().to_string();
        let written = grant.clone();
        match tokio::task::spawn_blocking(move || registry.set_path_grant(&tenant, &written)).await
        {
            Ok(Ok(())) => receipt_response(req.id, grant),
            Ok(Err(error)) => ops::append_error_response(req.id, error),
            Err(error) => Response::err(req.id, error.to_string()),
        }
    }
}

#[cfg(feature = "redb")]
pub use served::handle_set_path_grant;

/// `SetPathGrant` needs the `redb` registry owner file.
#[cfg(not(feature = "redb"))]
pub async fn handle_set_path_grant(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: SetPathGrantFields,
) -> Response {
    Response::err(req.id, "SetPathGrant requires the `redb` feature")
}
