//! MEDULLA-FEDERATED-QUERY-R081.103.3 and R081.103.4: viewport bins and selection
//! tokens through the real `dispatch` shell, over series written by the served
//! `TsAppend` path. A 1,000,000-point series answers every viewport size with a bounded
//! bin set; a pinned revision, another tenant and a stale token each refuse typed.
#![cfg(all(feature = "query", feature = "tsdb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_types::viewport::{
    LineBin, ResolvedSelection, ScatterBin, SelectionToken, ViewportBins, ViewportDomain,
    ViewportKind, ViewportRequest, ViewportResult, BINNING_REVISION,
};

const SECRET: &str = "viewport-served-secret";
const BIG: i64 = 1_000_000;
const STEP_NS: i64 = 1_000_000;

fn state() -> test_support::SharedState {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with_tsdb(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
        Some(test_support::temporary_series("viewport")),
    )
}

async fn send(state: &test_support::SharedState, request: Request) -> Response {
    Box::pin(test_support::dispatch(state, request)).await
}

fn req(id: u64, method: Method) -> Request {
    test_support::commons_request(SECRET, id, method)
}

fn other_tenant(id: u64, method: Method) -> Request {
    let mut context = common::context_for(common::TEST_AGENT);
    context.tenant = "another-tenant".to_string();
    common::signed_request_with_context(SECRET, id, "__commons__", context, method)
}

/// Append `points` to `series` in requests of at most 100,000 points.
async fn append(
    state: &test_support::SharedState,
    series: &str,
    fields: &[&str],
    points: Vec<(i64, Vec<f64>)>,
    first_id: u64,
) {
    for (n, chunk) in points.chunks(100_000).enumerate() {
        let response = send(
            state,
            req(
                first_id + n as u64,
                Method::TsAppend {
                    series_id: series.into(),
                    n_fields: fields.len(),
                    bucket_ns: 10_000_000_000,
                    field_names: fields.iter().map(|f| f.to_string()).collect(),
                    points_msgpack: rmp_serde::to_vec_named(&chunk.to_vec()).unwrap(),
                },
            ),
        )
        .await;
        assert!(
            response.error.is_none(),
            "append failed: {:?}",
            response.error
        );
    }
}

/// The big line value of point `i`.
fn line_value(i: i64) -> f64 {
    ((i * 31) % 977) as f64
}

/// The big scatter coordinates of point `i`: integers in 0..=1000.
fn scatter_xy(i: i64) -> (f64, f64) {
    (((i * 7919) % 1001) as f64, ((i * 104_729) % 1001) as f64)
}

fn line_request(source: &str, width: u32, span: i64) -> ViewportRequest {
    ViewportRequest {
        source_id: source.into(),
        source_revision: None,
        kind: ViewportKind::Line,
        x_axis: "ts".into(),
        y_axis: "v".into(),
        units: "ms".into(),
        filter_digest: String::new(),
        domain: ViewportDomain {
            x_min: 0.0,
            x_max: span as f64,
            y_min: 0.0,
            y_max: 1000.0,
        },
        pixel_width: width,
        pixel_height: 100,
        binning_revision: BINNING_REVISION,
        byte_ceiling: 16_777_216,
    }
}

fn scatter_request(width: u32, height: u32) -> ViewportRequest {
    ViewportRequest {
        source_id: "points".into(),
        source_revision: None,
        kind: ViewportKind::Scatter,
        x_axis: "a".into(),
        y_axis: "b".into(),
        units: "px".into(),
        filter_digest: String::new(),
        domain: ViewportDomain {
            x_min: 0.0,
            x_max: 1000.0,
            y_min: 0.0,
            y_max: 1000.0,
        },
        pixel_width: width,
        pixel_height: height,
        binning_revision: BINNING_REVISION,
        byte_ceiling: 16_777_216,
    }
}

async fn query(
    state: &test_support::SharedState,
    id: u64,
    request: ViewportRequest,
) -> Result<(ViewportResult, usize), String> {
    let response = send(
        state,
        req(
            id,
            Method::ViewportQuery {
                request: Box::new(request),
            },
        ),
    )
    .await;
    if let Some(error) = response.error {
        return Err(format!("{error:?}"));
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            let result = rmp_serde::from_slice(&bytes).expect("typed viewport result");
            Ok((result, bytes.len()))
        }
        other => panic!("unexpected payload {other:?}"),
    }
}

async fn resolve(
    state: &test_support::SharedState,
    id: u64,
    request: ViewportRequest,
    token: SelectionToken,
    as_other_tenant: bool,
) -> Result<ResolvedSelection, String> {
    let method = Method::ResolveViewportSelection {
        request: Box::new(request),
        token,
    };
    let wire = if as_other_tenant {
        other_tenant(id, method)
    } else {
        req(id, method)
    };
    let response = send(state, wire).await;
    if let Some(error) = response.error {
        return Err(format!("{error:?}"));
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => Ok(rmp_serde::from_slice(&bytes).unwrap()),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn scatter_bins(result: &ViewportResult) -> &[ScatterBin] {
    match &result.bins {
        ViewportBins::Scatter(bins) => bins,
        other => panic!("expected scatter bins, got {other:?}"),
    }
}

fn line_bins(result: &ViewportResult) -> &[LineBin] {
    match &result.bins {
        ViewportBins::Line(bins) => bins,
        other => panic!("expected line bins, got {other:?}"),
    }
}

async fn seed(state: &test_support::SharedState) {
    // 1,000,000 line points, 1,000,000 scatter points, 10,000 line points over the same span.
    let big: Vec<(i64, Vec<f64>)> = (0..BIG)
        .map(|i| (i * STEP_NS, vec![line_value(i)]))
        .collect();
    append(state, "big", &["v"], big, 100).await;
    let table: Vec<(i64, Vec<f64>)> = (0..BIG)
        .map(|i| {
            let (a, b) = scatter_xy(i);
            (i * STEP_NS, vec![a, b])
        })
        .collect();
    append(state, "points", &["a", "b"], table, 200).await;
    let small_step = (BIG - 1) * STEP_NS / 9_999;
    let small: Vec<(i64, Vec<f64>)> = (0..10_000i64)
        .map(|i| (i * small_step, vec![line_value(i)]))
        .collect();
    append(state, "small", &["v"], small, 300).await;
    // A series with a hole in the middle third.
    let gappy: Vec<(i64, Vec<f64>)> = (0..3000i64)
        .filter(|i| !(1000..2000).contains(i))
        .map(|i| (i * STEP_NS, vec![i as f64]))
        .collect();
    append(state, "gappy", &["v"], gappy, 400).await;
}

// spec: MEDULLA-FEDERATED-QUERY-R081.103.4
#[tokio::test(flavor = "multi_thread")]
async fn viewport_bins_are_bounded_and_pinned() {
    let state = state();
    seed(&state).await;
    let span = (BIG - 1) * STEP_NS;

    // Line: three viewport sizes over 1,000,000 points. One bin per pixel column, the
    // formulas hold, and the bin count does not depend on the cardinality.
    for width in [100u32, 800, 4096] {
        let (big, bytes) = query(&state, 1, line_request("big", width, span))
            .await
            .unwrap();
        let bins = line_bins(&big);
        assert_eq!(bins.len(), width as usize, "dense line: one bin per column");
        assert!(bins.iter().all(|b| !b.gap_before));
        assert!(bins.iter().map(|b| b.count).sum::<u64>() == BIG as u64);
        assert_eq!(big.source_cardinality, BIG as u64);
        let max_bins = (4 * u64::from(width)).min(65_536);
        let cap = 16_777_216u64.min(65_536 + 96 * max_bins);
        assert!(bytes as u64 <= cap, "{bytes} bytes past the {cap} cap");
        let (small, _) = query(&state, 2, line_request("small", width, span))
            .await
            .unwrap();
        assert_eq!(small.source_cardinality, 10_000);
        if width <= 800 {
            assert_eq!(
                line_bins(&small).len(),
                width as usize,
                "cardinality does not change the bins"
            );
        }
        // Oracle on the big source: bin counts from the index, no kernel.
        let mut counts = vec![0u64; width as usize];
        for i in 0..BIG {
            let col = ((i128::from(i) * i128::from(width)) / i128::from(BIG - 1))
                .min(i128::from(width) - 1);
            counts[col as usize] += 1;
        }
        let got: Vec<u64> = bins.iter().map(|b| b.count).collect();
        assert_eq!(got, counts, "width {width}");
    }

    // Scatter: three viewport sizes over 1,000,000 points.
    for (w, h) in [(100u32, 50u32), (800, 600), (4096, 4096)] {
        let (result, bytes) = query(&state, 3, scatter_request(w, h)).await.unwrap();
        let bins = scatter_bins(&result);
        let max_bins = (u64::from(w) * u64::from(h)).min(262_144);
        assert!(bins.len() as u64 <= max_bins);
        assert_eq!(bins.iter().map(|b| b.count).sum::<u64>(), BIG as u64);
        let cap = 16_777_216u64.min(65_536 + 96 * max_bins);
        assert!(bytes as u64 <= cap, "{bytes} bytes past the {cap} cap");
        assert_eq!(result.viewport.pixel_width, w);
        assert_eq!(result.binning_revision, BINNING_REVISION);
    }

    // Continuity: the hole in the middle third shows as one flagged column.
    let gap_span = 2999 * STEP_NS;
    let (gappy, _) = query(&state, 4, line_request("gappy", 30, gap_span))
        .await
        .unwrap();
    let flagged: Vec<u32> = line_bins(&gappy)
        .iter()
        .filter(|b| b.gap_before)
        .map(|b| b.col)
        .collect();
    assert_eq!(flagged.len(), 1, "one discontinuity: {flagged:?}");
    assert!(line_bins(&gappy).len() < 30);

    // Pan and zoom change the digest and the bins.
    let base = query(&state, 5, line_request("big", 100, span))
        .await
        .unwrap()
        .0;
    let mut zoomed = line_request("big", 100, span / 10);
    zoomed.domain.x_min = span as f64 / 4.0;
    zoomed.domain.x_max = span as f64 / 4.0 + (span / 10) as f64;
    let zoomed_result = query(&state, 6, zoomed).await.unwrap().0;
    assert_ne!(base.query_digest, zoomed_result.query_digest);
    assert_ne!(line_bins(&base), line_bins(&zoomed_result));
    assert!(zoomed_result.source_cardinality == base.source_cardinality);

    // The revision is pinned: the current one is accepted, a moved one refuses.
    let mut pinned = line_request("small", 50, span);
    pinned.source_revision = Some(base.source_revision);
    assert!(query(&state, 7, pinned.clone()).await.is_ok());
    append(&state, "extra", &["v"], vec![(1, vec![1.0])], 500).await;
    let error = query(&state, 8, pinned).await.unwrap_err();
    assert!(error.contains("STALE_SOURCE_REVISION"), "{error}");
}

// spec: MEDULLA-FEDERATED-QUERY-R081.103.3
#[tokio::test(flavor = "multi_thread")]
async fn selection_tokens_resolve_only_within_pinned_revisions() {
    let state = state();
    let points: Vec<(i64, Vec<f64>)> = (0..2000i64)
        .map(|i| (i * STEP_NS, vec![i as f64]))
        .collect();
    append(&state, "cpu", &["v"], points, 10).await;
    let span = 1999 * STEP_NS;
    let request = line_request("cpu", 10, span);
    let (result, _) = query(&state, 20, request.clone()).await.unwrap();
    let bin = line_bins(&result)[3];
    let token = SelectionToken::AggregateBin {
        bin_id: bin.col,
        source_revision: result.source_revision,
        index_revision: result.binning_revision,
        viewport_digest: result.query_digest.clone(),
    };

    // An aggregate bin resolves to exactly its points.
    let resolved = resolve(&state, 21, request.clone(), token.clone(), false)
        .await
        .unwrap();
    assert_eq!(resolved.total_count, bin.count);
    assert!(!resolved.truncated);
    assert_eq!(resolved.points.len() as u64, bin.count);
    assert_eq!(resolved.points.first().unwrap().values[0], bin.first);
    assert_eq!(resolved.points.last().unwrap().values[0], bin.last);

    // An exact point resolves to its one record.
    let exact = SelectionToken::ExactPoint {
        point_id: 7 * STEP_NS,
        point_revision: result.source_revision,
    };
    let one = resolve(&state, 22, request.clone(), exact, false)
        .await
        .unwrap();
    assert_eq!(one.points.len(), 1);
    assert_eq!(one.points[0].values, vec![7.0]);
    let missing = SelectionToken::ExactPoint {
        point_id: 7 * STEP_NS + 1,
        point_revision: result.source_revision,
    };
    let error = resolve(&state, 23, request.clone(), missing, false)
        .await
        .unwrap_err();
    assert!(error.contains("VIEWPORT_INVALID"), "{error}");

    // A selection never expands to unbounded raw points.
    let wide = line_request("cpu", 1, span);
    let (wide_result, _) = query(&state, 24, wide.clone()).await.unwrap();
    let all = SelectionToken::AggregateBin {
        bin_id: 0,
        source_revision: wide_result.source_revision,
        index_revision: wide_result.binning_revision,
        viewport_digest: wide_result.query_digest.clone(),
    };
    let capped = resolve(&state, 25, wide, all, false).await.unwrap();
    assert_eq!(capped.total_count, 2000);
    assert_eq!(capped.points.len(), 1000);
    assert!(capped.truncated);

    // A token of another viewport, or of an unserved binning revision, refuses.
    let mut foreign = token.clone();
    if let SelectionToken::AggregateBin {
        viewport_digest, ..
    } = &mut foreign
    {
        *viewport_digest = "0".repeat(64);
    }
    let error = resolve(&state, 26, request.clone(), foreign, false)
        .await
        .unwrap_err();
    assert!(error.contains("VIEWPORT_INVALID"), "{error}");
    let mut old_index = token.clone();
    if let SelectionToken::AggregateBin { index_revision, .. } = &mut old_index {
        *index_revision = 99;
    }
    let error = resolve(&state, 27, request.clone(), old_index, false)
        .await
        .unwrap_err();
    assert!(error.contains("INDEX_UNAVAILABLE"), "{error}");

    // An unknown source, and an unknown axis, are not indexed.
    let mut unknown = request.clone();
    unknown.source_id = "nothing".into();
    let error = query(&state, 28, unknown).await.unwrap_err();
    assert!(error.contains("INDEX_UNAVAILABLE"), "{error}");
    let mut bad_axis = request.clone();
    bad_axis.y_axis = "nope".into();
    let error = query(&state, 29, bad_axis).await.unwrap_err();
    assert!(error.contains("INDEX_UNAVAILABLE"), "{error}");

    // Bounds refuse before any scan.
    let mut zero = request.clone();
    zero.pixel_width = 0;
    let error = query(&state, 30, zero).await.unwrap_err();
    assert!(error.contains("VIEWPORT_INVALID"), "{error}");
    let mut tiny = request.clone();
    tiny.byte_ceiling = 10;
    let error = query(&state, 31, tiny).await.unwrap_err();
    assert!(error.contains("BYTE_CEILING_TOO_SMALL"), "{error}");
    let mut small_ceiling = request.clone();
    small_ceiling.byte_ceiling = 256;
    let error = query(&state, 32, small_ceiling).await.unwrap_err();
    assert!(error.contains("VIEWPORT_CAP_EXCEEDED"), "{error}");

    // Once the source moves, the pinned token is stale.
    append(&state, "other", &["v"], vec![(1, vec![1.0])], 600).await;
    let error = resolve(&state, 33, request.clone(), token.clone(), false)
        .await
        .unwrap_err();
    assert!(error.contains("STALE_SOURCE_REVISION"), "{error}");

    // A second tenant reads nothing: the series is not in its scope.
    let error = resolve(&state, 34, request.clone(), token, true)
        .await
        .unwrap_err();
    assert!(
        error.contains("INDEX_UNAVAILABLE") || error.contains("ACCESS_DENIED"),
        "{error}"
    );
    let response = send(
        &state,
        other_tenant(
            35,
            Method::ViewportQuery {
                request: Box::new(request),
            },
        ),
    )
    .await;
    assert!(
        response.error.is_some(),
        "a second tenant must not read the series"
    );
}
