//! The graph-scoped routes of `MeetingTranscriptRevision`,
//! `MeetingSummaryContext` and `MeetingSummaryCommit`
//! (MEDULLA-MEDIA-VOCAB-R001.101.3 / .4 / .5).
//!
//! Each arrives after graph ACL, tenant binding, lazy-open and placement
//! lookup, reads the meeting's transcript head (and, for a commit, the named
//! `SummaryContext`) from the request graph, lowers one write-set off the pure
//! `medulla_h_ingest::meeting_transcript` functions and commits it through the
//! existing ChangeEnvelope authority, exactly like `EnrichDocument`. A refusal
//! answers before any envelope exists.

use super::repository_index::commit_context;
use super::*;
use medulla_h_ingest::meeting_transcript as transcript;

/// Route the three meeting transcript methods; hand any other method back.
pub async fn route_meeting_transcript(
    ctx: GraphOpRouting<'_>,
    method: Method,
) -> Result<Response, Method> {
    match method {
        Method::MeetingTranscriptRevision { request } => Ok(revision(ctx, *request).await),
        Method::MeetingSummaryContext { request } => Ok(summary_context(ctx, *request).await),
        Method::MeetingSummaryCommit { request } => Ok(summary_commit(ctx, *request).await),
        other => Err(other),
    }
}

fn visible_properties(ctx: &GraphOpRouting<'_>, node_id: &str) -> Option<Vec<u8>> {
    let visible = ctx
        .read_authority
        .as_ref()
        .is_none_or(|authority| authority.node_visible(ctx.core, node_id));
    ctx.core.get_node_properties(node_id).filter(|_| visible)
}

fn state(
    ctx: &GraphOpRouting<'_>,
    meeting_id: &str,
) -> Result<medulla_core::meeting_transcript::TranscriptState, String> {
    let head = visible_properties(ctx, &transcript::transcript_node_id(meeting_id));
    transcript::load_state(head.as_deref())
}

async fn commit(
    ctx: GraphOpRouting<'_>,
    write_set: medulla_h_ingest::repository_index::IndexWriteSet,
    answer: Response,
) -> Response {
    let (placement_epoch, fencing_token) = match source_ingestion_route_fence(&ctx).await {
        Ok(fence) => fence,
        Err(response) => return response,
    };
    let commit_ctx = commit_context(&ctx, placement_epoch, fencing_token);
    let envelope = match medulla_h_ingest::repository_index::build_labelled_envelope(
        &commit_ctx,
        write_set,
        &transcript::LABELS,
    ) {
        Ok(envelope) => envelope,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let committed =
        crate::dispatch::change_envelope::apply_one_change_envelope(ctx, envelope).await;
    match committed.refusal_text() {
        Some(refusal) => Response::err(committed.id, refusal),
        None => answer,
    }
}

async fn revision(
    ctx: GraphOpRouting<'_>,
    request: medulla_types::meeting_transcript::MeetingTranscriptRevision,
) -> Response {
    let req_id = ctx.req_id;
    let lowered = state(&ctx, &request.meeting_id)
        .and_then(|state| transcript::commit_revision(&state, &request));
    let (write_set, receipt) = match lowered {
        Ok(lowered) => lowered,
        Err(error) => return Response::err(req_id, error),
    };
    let answer = Response::ok(
        req_id,
        ResultPayload::of::<
            medulla_protocol_types::result_contract::ingestion::MeetingTranscriptRevision,
        >(receipt),
    );
    match write_set {
        Some(write_set) => commit(ctx, write_set, answer).await,
        None => answer,
    }
}

async fn summary_context(
    ctx: GraphOpRouting<'_>,
    request: medulla_types::meeting_transcript::MeetingSummaryContextRequest,
) -> Response {
    let req_id = ctx.req_id;
    let built = state(&ctx, &request.meeting_id)
        .and_then(|state| transcript::context::build_context(&state, &request));
    let (write_set, context) = match built {
        Ok(built) => built,
        Err(error) => return Response::err(req_id, error),
    };
    let answer = Response::ok(
        req_id,
        ResultPayload::of::<
            medulla_protocol_types::result_contract::ingestion::MeetingSummaryContext,
        >(context),
    );
    commit(ctx, write_set, answer).await
}

async fn summary_commit(
    ctx: GraphOpRouting<'_>,
    request: medulla_types::meeting_transcript::MeetingSummaryCommitRequest,
) -> Response {
    let req_id = ctx.req_id;
    let context_id = transcript::context::context_node_id(&request.receipt.input_context_digest);
    let lowered = state(&ctx, &request.meeting_id).and_then(|state| {
        let stored = visible_properties(&ctx, &context_id)
            .map(|properties| transcript::context::stored_context(&properties))
            .transpose()?;
        transcript::commit::commit_summary(&state, stored.as_ref(), &request)
    });
    let (write_set, receipt) = match lowered {
        Ok(lowered) => lowered,
        Err(error) => return Response::err(req_id, error),
    };
    let answer = Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::MeetingSummaryCommit>(
            receipt,
        ),
    );
    commit(ctx, write_set, answer).await
}
