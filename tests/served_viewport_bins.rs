//! MEDULLA-FEDERATED-QUERY-R725: the served viewport bins match an independent reference
//! and obey the request limits; another tenant reads nothing. The methods are
//! `ViewportQuery` and `ResolveViewportSelection` (MEDULLA-FEDERATED-QUERY-R081.103).
#![cfg(all(feature = "query", feature = "tsdb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;

use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_types::viewport::{
    ViewportBins, ViewportDomain, ViewportKind, ViewportRequest, ViewportResult, BINNING_REVISION,
};

const SECRET: &str = "served-viewport-bins-secret";
const POINTS: i64 = 20_000;

fn state() -> test_support::SharedState {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with_tsdb(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
        Some(test_support::temporary_series("viewport-bins")),
    )
}

async fn send(state: &test_support::SharedState, request: Request) -> Response {
    Box::pin(test_support::dispatch(state, request)).await
}

fn xy(i: i64) -> (i64, i64) {
    ((i * 7919) % 1001, (i * 104_729) % 1001)
}

async fn seed(state: &test_support::SharedState) {
    let points: Vec<(i64, Vec<f64>)> = (0..POINTS)
        .map(|i| {
            let (a, b) = xy(i);
            (i * 1_000_000, vec![a as f64, b as f64])
        })
        .collect();
    let response = send(
        state,
        test_support::commons_request(
            SECRET,
            1,
            Method::TsAppend {
                series_id: "table".into(),
                n_fields: 2,
                bucket_ns: 10_000_000_000,
                field_names: vec!["a".into(), "b".into()],
                points_msgpack: rmp_serde::to_vec_named(&points).unwrap(),
            },
        ),
    )
    .await;
    assert!(
        response.error.is_none(),
        "seed failed: {:?}",
        response.error
    );
}

fn request(kind: ViewportKind, width: u32, height: u32, ceiling: u64) -> ViewportRequest {
    ViewportRequest {
        source_id: "table".into(),
        source_revision: None,
        kind,
        x_axis: if kind == ViewportKind::Line {
            "ts".into()
        } else {
            "a".into()
        },
        y_axis: "b".into(),
        units: String::new(),
        filter_digest: String::new(),
        domain: match kind {
            ViewportKind::Scatter => ViewportDomain {
                x_min: 0.0,
                x_max: 1000.0,
                y_min: 0.0,
                y_max: 1000.0,
            },
            ViewportKind::Line => ViewportDomain {
                x_min: 0.0,
                x_max: ((POINTS - 1) * 1_000_000) as f64,
                y_min: 0.0,
                y_max: 1000.0,
            },
        },
        pixel_width: width,
        pixel_height: height,
        binning_revision: BINNING_REVISION,
        byte_ceiling: ceiling,
    }
}

async fn run(
    state: &test_support::SharedState,
    id: u64,
    request: ViewportRequest,
    tenant: Option<&str>,
) -> Result<(ViewportResult, usize), String> {
    let method = Method::ViewportQuery {
        request: Box::new(request),
    };
    let wire = match tenant {
        None => test_support::commons_request(SECRET, id, method),
        Some(tenant) => {
            let mut context = common::context_for(common::TEST_AGENT);
            context.tenant = tenant.to_string();
            common::signed_request_with_context(SECRET, id, "__commons__", context, method)
        }
    };
    let response = send(state, wire).await;
    if let Some(error) = response.error {
        return Err(format!("{error:?}"));
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            Ok((rmp_serde::from_slice(&bytes).unwrap(), bytes.len()))
        }
        other => panic!("unexpected payload {other:?}"),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R725
#[tokio::test(flavor = "multi_thread")]
async fn bins_match_independent_reference_and_obey_limits() {
    let state = state();
    seed(&state).await;

    // Scatter against a reference that uses integer arithmetic only.
    let (w, h) = (50u32, 20u32);
    let (result, bytes) = run(
        &state,
        2,
        request(ViewportKind::Scatter, w, h, 1 << 24),
        None,
    )
    .await
    .unwrap();
    let mut reference: BTreeMap<u32, (u64, i64, i64, i64, i64)> = BTreeMap::new();
    for i in 0..POINTS {
        let (a, b) = xy(i);
        let col = ((a * i64::from(w)) / 1000).min(i64::from(w) - 1) as u32;
        let row = ((b * i64::from(h)) / 1000).min(i64::from(h) - 1) as u32;
        let cell = reference.entry(row * w + col).or_insert((0, a, a, b, b));
        cell.0 += 1;
        cell.1 = cell.1.min(a);
        cell.2 = cell.2.max(a);
        cell.3 = cell.3.min(b);
        cell.4 = cell.4.max(b);
    }
    let ViewportBins::Scatter(bins) = &result.bins else {
        panic!("scatter expected")
    };
    assert_eq!(bins.len(), reference.len());
    for bin in bins {
        let want = reference[&bin.id];
        assert_eq!(
            (
                bin.count,
                bin.x_min as i64,
                bin.x_max as i64,
                bin.y_min as i64,
                bin.y_max as i64
            ),
            want,
            "cell {}",
            bin.id
        );
    }
    assert!((bytes as u64) <= 65_536 + 96 * u64::from(w * h));

    // Line: the min and max of each pixel column against a reference by point index.
    let width = 40u32;
    let (line, _) = run(
        &state,
        3,
        request(ViewportKind::Line, width, 10, 1 << 24),
        None,
    )
    .await
    .unwrap();
    let ViewportBins::Line(columns) = &line.bins else {
        panic!("line expected")
    };
    let mut extrema: BTreeMap<u32, (i64, i64, u64)> = BTreeMap::new();
    for i in 0..POINTS {
        let col = ((i * i64::from(width)) / (POINTS - 1)).min(i64::from(width) - 1) as u32;
        let value = xy(i).1;
        let e = extrema.entry(col).or_insert((value, value, 0));
        e.0 = e.0.min(value);
        e.1 = e.1.max(value);
        e.2 += 1;
    }
    assert_eq!(columns.len(), extrema.len());
    for column in columns {
        let (min, max, count) = extrema[&column.col];
        assert_eq!(
            (column.min as i64, column.max as i64, column.count),
            (min, max, count)
        );
    }

    // Limits: a ceiling under the envelope refuses before the scan; a ceiling under the
    // result refuses with the cap code; the bins are never truncated to fit.
    let error = run(&state, 4, request(ViewportKind::Scatter, w, h, 64), None)
        .await
        .unwrap_err();
    assert!(error.contains("BYTE_CEILING_TOO_SMALL"), "{error}");
    let error = run(&state, 5, request(ViewportKind::Scatter, w, h, 1024), None)
        .await
        .unwrap_err();
    assert!(error.contains("VIEWPORT_CAP_EXCEEDED"), "{error}");

    // A moved revision is refused, the current one is accepted.
    let mut pinned = request(ViewportKind::Line, width, 10, 1 << 24);
    pinned.source_revision = Some(line.source_revision + 1_000);
    let error = run(&state, 6, pinned, None).await.unwrap_err();
    assert!(error.contains("STALE_SOURCE_REVISION"), "{error}");

    // Another tenant reads nothing.
    let denied = run(
        &state,
        7,
        request(ViewportKind::Scatter, w, h, 1 << 24),
        Some("tenant-b"),
    )
    .await;
    let error = denied.unwrap_err();
    assert!(
        error.contains("INDEX_UNAVAILABLE") || error.contains("ACCESS_DENIED"),
        "{error}"
    );
}
