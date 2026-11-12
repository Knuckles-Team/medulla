//! The graph-scoped route of `DataPrepDeclare` and `DataPrepDeclarationRead`
//! (MEDULLA-INGEST-WORKER-R001.1007 to R001.1009).
//!
//! Arrives after graph ACL, tenant binding, lazy-open and placement lookup,
//! exactly like `route_ingest_readback`. The request tenant must equal the
//! verified tenant. Declarations are read under the caller's row authority.
//! A changed declaration commits as one ChangeEnvelope; an equal digest and
//! every refusal write nothing.

use super::repository_index::commit_context;
use super::*;
use medulla_h_ingest::data_prep_declaration as prep;
use medulla_types::contract::ServerErrorCode;
use medulla_types::data_prep_declaration::{
    DataPrepDeclarationReadRequest, DataPrepDeclareReceipt, DataPrepDeclareRequest,
};

/// Route the data-prep declaration methods; hand any other method back.
pub async fn route_data_prep(ctx: GraphOpRouting<'_>, method: Method) -> Result<Response, Method> {
    match method {
        Method::DataPrepDeclare { request } => Ok(handle_declare(ctx, *request).await),
        Method::DataPrepDeclarationRead { request } => Ok(handle_read(ctx, *request)),
        other => Err(other),
    }
}

fn tenant_mismatch(ctx: &GraphOpRouting<'_>, tenant: &str) -> Option<String> {
    (tenant != ctx.verified_context.tenant()).then(|| {
        format!(
            "{}: request tenant does not match the verified tenant",
            ServerErrorCode::IngestTenantMismatch.as_str()
        )
    })
}

fn visible_rows<'a>(ctx: &'a GraphOpRouting<'_>) -> impl Fn(&str) -> bool + 'a {
    move |node_id: &str| {
        ctx.read_authority
            .as_ref()
            .is_none_or(|authority| authority.node_visible(ctx.core, node_id))
    }
}

fn handle_read(ctx: GraphOpRouting<'_>, request: DataPrepDeclarationReadRequest) -> Response {
    let req_id = ctx.req_id;
    if let Some(refusal) = tenant_mismatch(&ctx, &request.tenant) {
        return Response::err(req_id, refusal);
    }
    let visible = visible_rows(&ctx);
    let view = prep::CoreDeclarationView {
        core: ctx.core.as_ref(),
        visible: &visible,
    };
    match prep::read_data_prep_declaration(&view, &request.tenant, &request.capability) {
        Ok(found) => Response::ok(
            req_id,
            ResultPayload::of::<
                medulla_protocol_types::result_contract::ingestion::DataPrepDeclarationRead,
            >(found),
        ),
        Err(refusal) => Response::err(req_id, refusal.to_string()),
    }
}

async fn handle_declare(ctx: GraphOpRouting<'_>, request: DataPrepDeclareRequest) -> Response {
    let req_id = ctx.req_id;
    if let Some(refusal) = tenant_mismatch(&ctx, &request.tenant) {
        return Response::err(req_id, refusal);
    }
    let declared = {
        let visible = visible_rows(&ctx);
        let view = prep::CoreDeclarationView {
            core: ctx.core.as_ref(),
            visible: &visible,
        };
        match prep::declare_data_prep(&view, &request.tenant, &request.payload) {
            Ok(declared) => declared,
            Err(refusal) => return Response::err(req_id, refusal.to_string()),
        }
    };
    let (content_digest, write_set, written) = match declared {
        prep::Declared::Unchanged { content_digest } => {
            return declare_ok(req_id, content_digest, 0, 1)
        }
        prep::Declared::Write {
            content_digest,
            write_set,
        } => (content_digest, write_set, 1),
    };
    let (placement_epoch, fencing_token) = match source_ingestion_route_fence(&ctx).await {
        Ok(fence) => fence,
        Err(response) => return response,
    };
    let commit_ctx = commit_context(&ctx, placement_epoch, fencing_token);
    let envelope = match medulla_h_ingest::repository_index::build_labelled_envelope(
        &commit_ctx,
        write_set,
        &prep::LABELS,
    ) {
        Ok(envelope) => envelope,
        Err(error) => return Response::err(req_id, error),
    };
    let committed =
        crate::dispatch::change_envelope::apply_one_change_envelope(ctx, envelope).await;
    match committed.refusal_text() {
        Some(refusal) => Response::err(committed.id, refusal),
        None => declare_ok(committed.id, content_digest, written, 0),
    }
}

fn declare_ok(
    id: u64,
    content_digest: medulla_types::contract::Digest256,
    written: u64,
    unchanged: u64,
) -> Response {
    Response::ok(
        id,
        ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::DataPrepDeclare>(
            DataPrepDeclareReceipt {
                content_digest,
                written,
                unchanged,
            },
        ),
    )
}
