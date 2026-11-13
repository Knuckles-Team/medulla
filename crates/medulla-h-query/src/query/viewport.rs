//! `Method::ViewportQuery` and `Method::ResolveViewportSelection`
//! (MEDULLA-FEDERATED-QUERY-R081.103.3): bounded bins and token resolution over one
//! indexed source of the caller.
//!
//! A source is a time series scoped to the verified caller: `Line` reads one field against
//! the timestamp, `Scatter` reads two fields of the same series as a point table. The
//! handler binds the verified tenant authority first and reads only inside that scope, so
//! another caller's series does not exist for it. The source revision is the scope version
//! of the series store. A request may pin it; a moved revision refuses
//! `STALE_SOURCE_REVISION`. The kernels in `medulla_decision_stats::viewport` bound the
//! bin count; the serialized response is measured against the effective byte ceiling
//! before it is returned. Nothing here writes.

use super::*;
use medulla_decision_stats::viewport as kernel;
use medulla_protocol_types::result_contract::query::{ResolveViewportSelection, ViewportQuery};
use medulla_types::viewport::{
    ResolvedPoint, ResolvedSelection, SelectionToken, ViewportBins, ViewportError, ViewportKind,
    ViewportRequest, ViewportResult, ViewportSpec, BINNING_REVISION, MAX_RESOLVED_POINTS,
};

fn refusal(req_id: u64, error: ViewportError) -> Response {
    Response::err(req_id, error.to_string())
}

/// One source read: the points of the window and the pinned revision.
#[cfg(feature = "tsdb")]
struct Source {
    points: Vec<medulla_tsdb::point::Point>,
    revision: u64,
    cardinality: u64,
    x_field: Option<usize>,
    y_field: usize,
}

/// Bind the verified caller, find its series, check the pinned revision and read the points
/// the window needs. Every refusal comes before the scan except the ones that need it.
#[cfg(feature = "tsdb")]
async fn read_source(
    ctx: &QueryHandlerCtx<'_>,
    request: &ViewportRequest,
) -> Result<Source, ViewportError> {
    use medulla_tsdb::store::SeriesKey;
    request.validate()?;
    let carrier = ctx
        .read_authority
        .and_then(|authority| authority.carrier())
        .ok_or(ViewportError::AccessDenied)?;
    let max_bins = max_bins_of(request);
    kernel::effective_ceiling(request.byte_ceiling, max_bins)?;
    let store = ctx.state.read().await.tsdb_store.clone();
    let Some(store) = store else {
        return Err(ViewportError::IndexUnavailable(
            "no time-series store is configured".into(),
        ));
    };
    let tenant = carrier.tenant_scope().to_string();
    let graph = carrier.namespace("timeseries-graph", ctx.graph_name);
    let key = SeriesKey::new(&tenant, &graph, &request.source_id);
    let unavailable = |detail: &str| ViewportError::IndexUnavailable(detail.to_string());
    let meta = store
        .meta_scoped(&key)
        .map_err(|_| unavailable("the source could not be read"))?
        .ok_or_else(|| unavailable("the source is not indexed for this caller"))?;
    let field = |name: &str| meta.field_names.iter().position(|f| f == name);
    let y_field =
        field(&request.y_axis).ok_or_else(|| unavailable("y_axis is not a field of the source"))?;
    let x_field = match request.kind {
        ViewportKind::Line => None,
        ViewportKind::Scatter => Some(
            field(&request.x_axis)
                .ok_or_else(|| unavailable("x_axis is not a field of the source"))?,
        ),
    };
    let revision = store
        .mutation_version(&tenant, &graph)
        .map_err(|_| unavailable("the source revision could not be read"))?;
    if let Some(pinned) = request.source_revision {
        if pinned != revision {
            return Err(ViewportError::StaleSourceRevision {
                pinned,
                current: revision,
            });
        }
    }
    let (from, to) = match request.kind {
        ViewportKind::Line => (
            request.domain.x_min as i64,
            (request.domain.x_max as i64).saturating_add(1),
        ),
        ViewportKind::Scatter => (i64::MIN, i64::MAX),
    };
    let points = store
        .range_scoped(&key, from, to)
        .map_err(|_| unavailable("the source could not be read"))?;
    Ok(Source {
        points,
        revision,
        cardinality: meta.count,
        x_field,
        y_field,
    })
}

/// The bin cap of a request.
#[cfg(feature = "tsdb")]
fn max_bins_of(request: &ViewportRequest) -> u64 {
    match request.kind {
        ViewportKind::Scatter => {
            kernel::scatter_max_bins(request.pixel_width, request.pixel_height)
        }
        ViewportKind::Line => kernel::line_max_bins(request.pixel_width),
    }
}

#[cfg(feature = "tsdb")]
fn bin(request: &ViewportRequest, source: &Source) -> Result<ViewportBins, ViewportError> {
    let d = &request.domain;
    match request.kind {
        ViewportKind::Scatter => {
            let x = source.x_field.unwrap_or(0);
            kernel::bin_scatter(
                source
                    .points
                    .iter()
                    .filter_map(|p| Some((*p.values.get(x)?, *p.values.get(source.y_field)?))),
                d,
                request.pixel_width,
                request.pixel_height,
            )
            .map(ViewportBins::Scatter)
        }
        ViewportKind::Line => kernel::bin_line(
            source
                .points
                .iter()
                .filter_map(|p| Some((p.ts, *p.values.get(source.y_field)?))),
            d.x_min as i64,
            d.x_max as i64,
            request.pixel_width,
        )
        .map(ViewportBins::Line),
    }
}

#[cfg(feature = "tsdb")]
pub(crate) async fn handle_viewport_query(
    ctx: &QueryHandlerCtx<'_>,
    request: ViewportRequest,
) -> Response {
    let req_id = ctx.req_id;
    let outcome = async {
        let source = read_source(ctx, &request).await?;
        let bins = bin(&request, &source)?;
        let result = ViewportResult {
            bins,
            source_revision: source.revision,
            viewport: ViewportSpec {
                domain: request.domain,
                pixel_width: request.pixel_width,
                pixel_height: request.pixel_height,
            },
            binning_revision: BINNING_REVISION,
            source_cardinality: source.cardinality,
            query_digest: request.query_digest(),
        };
        let max_bins = max_bins_of(&request);
        let ceiling = kernel::effective_ceiling(request.byte_ceiling, max_bins)?;
        let payload = ResultPayload::of::<ViewportQuery>(result)
            .map_err(|e| ViewportError::Invalid(format!("the result could not be encoded: {e}")))?;
        if let ResultPayload::Raw(bytes) = &payload {
            if bytes.len() as u64 > ceiling {
                return Err(ViewportError::CapExceeded(format!(
                    "the result is {} bytes, past the {ceiling} byte ceiling",
                    bytes.len()
                )));
            }
        }
        Ok::<_, ViewportError>(payload)
    }
    .await;
    match outcome {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => refusal(req_id, error),
    }
}

#[cfg(feature = "tsdb")]
pub(crate) async fn handle_resolve_viewport_selection(
    ctx: &QueryHandlerCtx<'_>,
    request: ViewportRequest,
    token: SelectionToken,
) -> Response {
    let req_id = ctx.req_id;
    let outcome = async {
        // The token pins its own revisions; the request carries the window it came from.
        let pinned = match &token {
            SelectionToken::ExactPoint { point_revision, .. } => *point_revision,
            SelectionToken::AggregateBin {
                source_revision,
                index_revision,
                viewport_digest,
                ..
            } => {
                if *index_revision != BINNING_REVISION {
                    return Err(ViewportError::IndexUnavailable(
                        "the binning revision of the token is not served".into(),
                    ));
                }
                if *viewport_digest != request.query_digest() {
                    return Err(ViewportError::Invalid(
                        "the token does not belong to this viewport".into(),
                    ));
                }
                *source_revision
            }
        };
        let mut pinned_request = request.clone();
        pinned_request.source_revision = Some(pinned);
        let source = read_source(ctx, &pinned_request).await?;
        let d = &request.domain;
        let members: Vec<&medulla_tsdb::point::Point> = match &token {
            SelectionToken::ExactPoint { point_id, .. } => {
                source.points.iter().filter(|p| p.ts == *point_id).collect()
            }
            SelectionToken::AggregateBin { bin_id, .. } => match request.kind {
                ViewportKind::Line => source
                    .points
                    .iter()
                    .filter(|p| {
                        kernel::line_column(
                            p.ts,
                            d.x_min as i64,
                            d.x_max as i64,
                            request.pixel_width,
                        ) == Some(*bin_id)
                    })
                    .collect(),
                ViewportKind::Scatter => {
                    let x = source.x_field.unwrap_or(0);
                    source
                        .points
                        .iter()
                        .filter(|p| match (p.values.get(x), p.values.get(source.y_field)) {
                            (Some(px), Some(py)) => {
                                kernel::scatter_cell(
                                    *px,
                                    *py,
                                    d,
                                    request.pixel_width,
                                    request.pixel_height,
                                ) == Some(*bin_id)
                            }
                            _ => false,
                        })
                        .collect()
                }
            },
        };
        if members.is_empty() {
            return Err(ViewportError::Invalid(
                "the token names no point inside the pinned revision".into(),
            ));
        }
        let total = members.len();
        let points: Vec<ResolvedPoint> = members
            .into_iter()
            .take(MAX_RESOLVED_POINTS)
            .map(|p| ResolvedPoint {
                ts: p.ts,
                values: p.values.clone(),
            })
            .collect();
        let truncated = total > points.len();
        let resolved = ResolvedSelection {
            points,
            total_count: total as u64,
            truncated,
            source_revision: source.revision,
        };
        ResultPayload::of::<ResolveViewportSelection>(resolved)
            .map_err(|e| ViewportError::Invalid(format!("the result could not be encoded: {e}")))
    }
    .await;
    match outcome {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => refusal(req_id, error),
    }
}

/// Without the time-series build no source is indexed.
#[cfg(not(feature = "tsdb"))]
pub(crate) async fn handle_viewport_query(
    ctx: &QueryHandlerCtx<'_>,
    _request: ViewportRequest,
) -> Response {
    refusal(
        ctx.req_id,
        ViewportError::IndexUnavailable("this build serves no time-series source".into()),
    )
}

/// Without the time-series build no source is indexed.
#[cfg(not(feature = "tsdb"))]
pub(crate) async fn handle_resolve_viewport_selection(
    ctx: &QueryHandlerCtx<'_>,
    _request: ViewportRequest,
    _token: SelectionToken,
) -> Response {
    refusal(
        ctx.req_id,
        ViewportError::IndexUnavailable("this build serves no time-series source".into()),
    )
}
