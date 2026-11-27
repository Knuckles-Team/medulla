//! The graph-scoped route of `TranscriptRead` and `QuarantineRead`
//! (MEDULLA-INGEST-WORKER-R003.17.1).
//!
//! Arrives after graph ACL, tenant binding, lazy-open and placement lookup. Tenant and
//! graph come from verified context only. Each handler checks its stored policy action
//! (`source:transcript:read`, `source:quarantine:read`) and reads rows under the caller's row
//! authority; the check never reads the request, and a refusal returns no text.

use super::*;
use medulla_source_types::source_transcript::TranscriptReadRequest;

/// Route the two stored-evidence reads; hand any other method back.
pub async fn route_source_reads(
    ctx: GraphOpRouting<'_>,
    method: Method,
) -> Result<Response, Method> {
    match method {
        Method::TranscriptRead {
            connector,
            stream,
            session_id,
            after_sequence,
            limit,
        } => Ok(handle_transcript_read(
            ctx,
            TranscriptReadRequest {
                connector,
                stream,
                session_id,
                after_sequence,
                limit,
            },
        )),
        Method::QuarantineRead {
            connector,
            stream,
            batch_id,
        } => Ok(handle_quarantine_read(ctx, &connector, &stream, &batch_id)),
        other => Err(other),
    }
}

fn visible_rows<'a>(ctx: &'a GraphOpRouting<'_>) -> impl Fn(&str) -> bool + 'a {
    move |node_id: &str| {
        ctx.read_authority
            .as_ref()
            .is_none_or(|authority| authority.node_visible(ctx.core, node_id))
    }
}

fn handle_transcript_read(ctx: GraphOpRouting<'_>, request: TranscriptReadRequest) -> Response {
    let req_id = ctx.req_id;
    let visible = visible_rows(&ctx);
    match medulla_h_ingest::source_ingestion::transcript::read_transcript(
        ctx.verified_context,
        ctx.core.as_ref(),
        &visible,
        &request,
    ) {
        Ok(view) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::TranscriptRead>(
                view,
            ),
        ),
        Err(refusal) => Response::err(req_id, refusal),
    }
}

fn handle_quarantine_read(
    ctx: GraphOpRouting<'_>,
    connector: &medulla_types::contract::ResourceId,
    stream: &medulla_types::contract::ResourceId,
    batch_id: &str,
) -> Response {
    let req_id = ctx.req_id;
    let visible = visible_rows(&ctx);
    match medulla_h_ingest::source_ingestion::documents::read_quarantine(
        ctx.verified_context,
        ctx.core.as_ref(),
        &visible,
        connector,
        stream,
        batch_id,
    ) {
        Ok(view) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::QuarantineRead>(
                view,
            ),
        ),
        Err(refusal) => Response::err(req_id, refusal),
    }
}
