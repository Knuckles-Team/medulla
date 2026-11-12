//! `Method::ReadSpan` (MEDULLA-DECISION-ENGINE-R153.3, R153.5).
//!
//! Reads a bounded line range of one ingested text source. `SourceIngest`
//! stored the source text and its table of contents as content-addressed blobs
//! and named them on the document node (`toc_digest`, `source_digest`,
//! `source_revision`). The read runs over the row-level-security projected core,
//! so a document the caller cannot see (or that belongs to another tenant's
//! graph) is `DOCUMENT_NOT_FOUND`. It locates the span through the table's line
//! index, reads the text blob once and writes nothing.

use super::terminal::GraphOpsContext;
use super::*;
use medulla_document::toc::locate_span;
use medulla_server_api::knob_config::knobs;
use medulla_types::contract::ServerErrorCode;
use medulla_types::source_toc::{
    ReadSpanResult, SourceToc, SOURCE_DIGEST_PROPERTY, SOURCE_REVISION_PROPERTY,
    TOC_DIGEST_PROPERTY,
};

/// Content-addressed read access to the stored source text and table blobs.
pub(super) trait SpanBlobs {
    /// The bytes stored under `digest`, if any.
    fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, String>;
}

/// The arguments of one `ReadSpan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SpanRequest {
    pub(super) document_id: String,
    pub(super) revision: u64,
    pub(super) line_start: u64,
    pub(super) line_end: u64,
}

/// Why a span cannot be served; each variant is one registered refusal code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SpanRefusal {
    DocumentNotFound,
    TocMissing,
    Inverted,
    OutOfBounds,
    TooLarge,
    Unavailable(String),
}

impl SpanRefusal {
    fn text(&self, request: &SpanRequest) -> String {
        match self {
            Self::DocumentNotFound => format!(
                "{}: document {} is not visible in this graph",
                ServerErrorCode::DocumentNotFound.as_str(),
                request.document_id
            ),
            Self::TocMissing => format!(
                "{}: document {} has no table of contents for revision {}",
                ServerErrorCode::TocMissing.as_str(),
                request.document_id,
                request.revision
            ),
            Self::Inverted => format!(
                "{}: line_end {} is below line_start {}",
                ServerErrorCode::SpanInverted.as_str(),
                request.line_end,
                request.line_start
            ),
            Self::OutOfBounds => format!(
                "{}: lines {}..={} fall outside the document",
                ServerErrorCode::SpanOutOfBounds.as_str(),
                request.line_start,
                request.line_end
            ),
            Self::TooLarge => format!(
                "{}: the range exceeds knob toc.max_span_lines",
                ServerErrorCode::SpanTooLarge.as_str()
            ),
            Self::Unavailable(reason) => format!(
                "{}: {reason}",
                ServerErrorCode::SourceIngestionUnavailable.as_str()
            ),
        }
    }
}

/// Route the span read served from the graph.
pub(super) async fn try_handle_read_span(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::ReadSpan {
        document_id,
        revision,
        line_start,
        line_end,
    } = method
    else {
        return ControlFlow::Continue(method);
    };
    let request = SpanRequest {
        document_id,
        revision,
        line_start,
        line_end,
    };
    let response = match state_blobs(&ctx).await {
        Some(blobs) => serve_read_span(ctx.req_id, ctx.core, blobs.as_ref(), &request),
        None => Response::err(
            ctx.req_id,
            SpanRefusal::Unavailable("the blob substrate is not configured".into()).text(&request),
        ),
    };
    ControlFlow::Break(response)
}

#[cfg(feature = "blob")]
struct ChunkBlobs(Arc<dyn medulla_server_blob::blob::store::ChunkStore>);

#[cfg(feature = "blob")]
impl SpanBlobs for ChunkBlobs {
    fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, String> {
        self.0.get_chunk(digest)
    }
}

#[cfg(feature = "blob")]
async fn state_blobs(ctx: &GraphOpsContext<'_>) -> Option<Box<dyn SpanBlobs>> {
    use medulla_server_blob::blob::BlobStateExt;
    let state = ctx.state.read().await;
    let cursors = state.blob_cursors()?;
    Some(Box::new(ChunkBlobs(cursors.store.clone())))
}

#[cfg(not(feature = "blob"))]
async fn state_blobs(_ctx: &GraphOpsContext<'_>) -> Option<Box<dyn SpanBlobs>> {
    None
}

/// Answer one request over the caller's graph view. A refusal carries its
/// registered code and no result; a read adds no sealed record.
pub(super) fn serve_read_span(
    req_id: u64,
    core: &GraphCore,
    blobs: &dyn SpanBlobs,
    request: &SpanRequest,
) -> Response {
    match read_span(core, blobs, request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::graph::ReadSpan>(result),
        ),
        Err(refusal) => Response::err(req_id, refusal.text(request)),
    }
}

/// The three node properties `SourceIngest` set on a text source.
struct StoredText {
    toc_digest: String,
    source_digest: String,
    revision: u64,
}

fn stored_text(core: &GraphCore, document_id: &str) -> Result<StoredText, SpanRefusal> {
    let blob = core
        .get_node_properties(document_id)
        .ok_or(SpanRefusal::DocumentNotFound)?;
    let properties = medulla_types::msgpack::decode_property_value(&blob)
        .map_err(|_| SpanRefusal::TocMissing)?;
    let text = |key: &str| {
        properties
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    match (
        text(TOC_DIGEST_PROPERTY),
        text(SOURCE_DIGEST_PROPERTY),
        properties
            .get(SOURCE_REVISION_PROPERTY)
            .and_then(|value| value.as_u64()),
    ) {
        (Some(toc_digest), Some(source_digest), Some(revision)) => Ok(StoredText {
            toc_digest,
            source_digest,
            revision,
        }),
        _ => Err(SpanRefusal::TocMissing),
    }
}

fn read_span(
    core: &GraphCore,
    blobs: &dyn SpanBlobs,
    request: &SpanRequest,
) -> Result<ReadSpanResult, SpanRefusal> {
    if request.line_end < request.line_start {
        return Err(SpanRefusal::Inverted);
    }
    if request.line_start == 0 {
        return Err(SpanRefusal::OutOfBounds);
    }
    if request.line_end - request.line_start + 1 > knobs().toc_max_span_lines {
        return Err(SpanRefusal::TooLarge);
    }
    let stored = stored_text(core, &request.document_id)?;
    if stored.revision != request.revision {
        return Err(SpanRefusal::TocMissing);
    }
    let unavailable = |error: String| SpanRefusal::Unavailable(error);
    let toc_bytes = blobs
        .get(&stored.toc_digest)
        .map_err(unavailable)?
        .ok_or(SpanRefusal::TocMissing)?;
    let toc = SourceToc::from_json(&toc_bytes).map_err(|_| SpanRefusal::TocMissing)?;
    if toc.document_id != request.document_id
        || toc.revision != request.revision
        || toc.source_digest != stored.source_digest
    {
        return Err(SpanRefusal::TocMissing);
    }
    if request.line_end > toc.total_lines {
        return Err(SpanRefusal::OutOfBounds);
    }
    let source = blobs
        .get(&stored.source_digest)
        .map_err(unavailable)?
        .ok_or(SpanRefusal::TocMissing)?;
    let (byte_start, byte_end) = locate_span(
        &toc,
        &source,
        toc.line_index_stride,
        request.line_start,
        request.line_end,
    )
    .ok_or(SpanRefusal::OutOfBounds)?;
    let text = std::str::from_utf8(&source[byte_start as usize..byte_end as usize])
        .map_err(|_| SpanRefusal::TocMissing)?
        .to_owned();
    Ok(ReadSpanResult {
        document_id: request.document_id.clone(),
        revision: request.revision,
        toc_digest: stored.toc_digest,
        source_digest: stored.source_digest,
        line_start: request.line_start,
        line_end: request.line_end,
        byte_start,
        byte_end,
        text,
    })
}
