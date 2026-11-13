//! R002.4, R002.9, R002.10: the state-space series through the served dispatch path.
//! `TsDefineStateSpace`, `TsAppend` and `TsRange` against a real `series.redb`.
#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "tsdb",
    feature = "query"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_tsdb::store::SeriesStore;
use medulla_types::series_expr::{
    DerivedSeriesReceipt, NoisePolicy, StateSpaceModel, StateSpaceProfile,
    STATE_SPACE_KERNEL_REVISION,
};
use medulla_types::wire::{Op, Plan};

const SECRET: &str = "medulla-state-space-served-secret";

struct Served {
    state: test_support::SharedState,
    next_id: u64,
}

impl Served {
    fn over(store: Arc<SeriesStore>) -> Self {
        let (dir, persistence) = common::tempdir_persistence();
        Self {
            state: test_support::state_with_tsdb(
                SECRET,
                common::current_isolation(),
                dir,
                persistence,
                Some(store),
            ),
            next_id: 1,
        }
    }

    async fn call(&mut self, method: Method) -> Response {
        self.next_id += 1;
        let request = test_support::commons_request(SECRET, self.next_id, method);
        test_support::dispatch(&self.state, request).await
    }

    /// The same call from a caller whose verified context carries only `scopes`.
    async fn call_scoped(&mut self, scopes: &[&str], method: Method) -> Response {
        self.next_id += 1;
        let request =
            common::signed_request_with_scopes(SECRET, self.next_id, "__commons__", scopes, method);
        test_support::dispatch(&self.state, request).await
    }

    async fn append(&mut self, series: &str, fields: usize, rows: &[(i64, Vec<f64>)]) -> Response {
        self.call(Method::TsAppend {
            series_id: series.into(),
            n_fields: fields,
            bucket_ns: 1_000_000,
            field_names: (0..fields).map(|i| format!("f{i}")).collect(),
            points_msgpack: rmp_serde::to_vec(&rows.to_vec()).unwrap(),
        })
        .await
    }

    async fn define(&mut self, series: &str, source: &str, model: &StateSpaceModel) -> Response {
        self.call(Method::TsDefineStateSpace {
            series_id: series.into(),
            source: source.into(),
            model: model.clone(),
        })
        .await
    }

    /// The structured-query read: a `TsScan` over the series (seconds on the plan axis).
    async fn scan(&mut self, series: &str) -> Vec<(String, Option<f32>)> {
        let plan = Plan::new(vec![Op::TsScan {
            series: vec![series.into()],
            from: 0.0,
            to: 1.0,
        }]);
        let r = self
            .call(Method::UnifiedQuery {
                plan,
                foreign_execution: None,
                foreign_executions: Default::default(),
                code_map: None,
            })
            .await;
        test_support::raw_rows(&r)
    }

    async fn range(&mut self, series: &str) -> Vec<(i64, Vec<f64>)> {
        let r = self
            .call(Method::TsRange {
                series_id: series.into(),
                from: i64::MIN,
                to: i64::MAX,
            })
            .await;
        raw(&r)
    }
}

fn raw<T: serde::de::DeserializeOwned>(response: &Response) -> T {
    match &response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!(
            "expected a raw result, got {other:?} / {:?}",
            response.error
        ),
    }
}

fn receipt(response: &Response) -> DerivedSeriesReceipt {
    raw(response)
}

fn refusal(response: &Response) -> String {
    response
        .error
        .clone()
        .map(|e| format!("{e:?}"))
        .unwrap_or_default()
}

fn model(profile: StateSpaceProfile, n: usize, revision: u64) -> StateSpaceModel {
    let eye = |k: usize, s: f64| -> Vec<f64> {
        (0..k * k)
            .map(|i| if i / k == i % k { s } else { 0.0 })
            .collect()
    };
    StateSpaceModel {
        profile,
        model_revision: revision,
        kernel_revision: STATE_SPACE_KERNEL_REVISION.into(),
        state_dim: n as u32,
        obs_dim: 1,
        transition: eye(n, 1.0),
        process_noise: eye(n, 0.01),
        measurement_noise: vec![0.5],
        initial_mean: vec![0.0; n],
        initial_covariance: eye(n, 1.0),
        noise_policy: NoisePolicy::Uncorrelated,
        symmetry_tol: 1e-9,
        numeric_policy: "ieee754-f64".into(),
    }
}

/// Seeded volatility returns (`ss-vol-seed-7`): a deterministic LCG, no data download.
fn returns(seed: u64, count: usize) -> Vec<(i64, Vec<f64>)> {
    let mut x = seed;
    (1..=count as i64)
        .map(|i| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u = ((x >> 33) as f64) / (1u64 << 31) as f64 - 0.5;
            (i * 1000, vec![0.01 * (1.0 + u) + 0.002])
        })
        .collect()
}

/// Seeded hedge pairs (`ss-hedge-seed-11`): `y = 0.5 + 1.5 x + noise`, fields `[y, 1, x]`.
fn pairs(seed: u64, count: usize) -> Vec<(i64, Vec<f64>)> {
    let mut x = seed;
    (1..=count as i64)
        .map(|i| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u = ((x >> 33) as f64) / (1u64 << 31) as f64 - 0.5;
            let regressor = (i % 11) as f64 / 10.0;
            (
                i * 1000,
                vec![0.5 + 1.5 * regressor + 0.01 * u, 1.0, regressor],
            )
        })
        .collect()
}

fn store() -> Arc<SeriesStore> {
    test_support::temporary_series("state-space-served")
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.9
#[tokio::test]
async fn served_filter_wiring() {
    let mut s = Served::over(store());
    let vol = returns(7, 40);
    assert!(s.append("ret", 1, &vol[..20]).await.error.is_none());
    let r = receipt(
        &s.define("vol", "ret", &model(StateSpaceProfile::Volatility, 1, 1))
            .await,
    );
    assert_eq!((r.cursor, r.appended, r.caught_up), (20, 20, true));
    assert_eq!(r.model_revision, Some(1));
    assert!(r
        .checkpoint_digest
        .as_deref()
        .is_some_and(|d| d.starts_with("sha256:")));
    assert!(s.append("ret", 1, &vol[20..]).await.error.is_none());
    let out = s.range("vol").await;
    assert_eq!(out.len(), 40);
    assert!(out
        .iter()
        .all(|(_, v)| v.len() == 3 && v[0].is_finite() && v[1] == 0.0));
    // The structured query reads every derived point back with the filtered level as its score.
    let scanned = s.scan("vol").await;
    assert_eq!(scanned.len(), 40);
    let mut got: Vec<f32> = scanned.iter().map(|(_, score)| score.unwrap()).collect();
    let mut want: Vec<f32> = out.iter().map(|(_, v)| v[0] as f32).collect();
    got.sort_by(f32::total_cmp);
    want.sort_by(f32::total_cmp);
    assert_eq!(got, want);
    let again = receipt(
        &s.define("vol", "ret", &model(StateSpaceProfile::Volatility, 1, 1))
            .await,
    );
    assert_eq!((again.cursor, again.appended), (40, 0));

    let hedge = pairs(11, 60);
    assert!(s.append("pair", 3, &hedge).await.error.is_none());
    let h = receipt(
        &s.define("hedge", "pair", &model(StateSpaceProfile::Hedge, 2, 1))
            .await,
    );
    assert_eq!((h.cursor, h.appended), (60, 60));
    let last = s.range("hedge").await.pop().unwrap().1[0];
    assert!(last.is_finite());
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.9
#[tokio::test]
async fn served_filter_scope_denied() {
    let mut s = Served::over(store());
    assert!(s.append("pair", 3, &pairs(11, 5)).await.error.is_none());
    let denied = s
        .define("hedge", "missing", &model(StateSpaceProfile::Hedge, 2, 1))
        .await;
    assert!(
        refusal(&denied).contains("STATE_SCHEMA_UNAVAILABLE"),
        "{}",
        refusal(&denied)
    );
    assert!(s.range("hedge").await.is_empty());

    // A caller without write scope is refused before the handler runs and nothing is
    // created, the source stays readable and a scoped re-definition still succeeds.
    let read_only = s
        .call_scoped(
            &["graph:read"],
            Method::TsDefineStateSpace {
                series_id: "hedge".into(),
                source: "pair".into(),
                model: model(StateSpaceProfile::Hedge, 2, 1),
            },
        )
        .await;
    assert!(
        read_only.result.is_none(),
        "an unscoped caller defined a series"
    );
    assert!(
        refusal(&read_only).contains("ACCESS_DENIED"),
        "{}",
        refusal(&read_only)
    );
    assert!(s.range("hedge").await.is_empty());
    let granted = receipt(
        &s.define("hedge", "pair", &model(StateSpaceProfile::Hedge, 2, 1))
            .await,
    );
    assert_eq!((granted.cursor, granted.appended), (5, 5));
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.9
#[tokio::test]
async fn served_filter_stale_checkpoint() {
    let mut s = Served::over(store());
    assert!(s.append("pair", 3, &pairs(11, 5)).await.error.is_none());
    let first = receipt(
        &s.define("hedge", "pair", &model(StateSpaceProfile::Hedge, 2, 1))
            .await,
    );
    let stale = s
        .define("hedge", "pair", &model(StateSpaceProfile::Hedge, 2, 2))
        .await;
    assert!(
        refusal(&stale).contains("STATE_CHECKPOINT_STALE"),
        "{}",
        refusal(&stale)
    );
    let kept = receipt(
        &s.define("hedge", "pair", &model(StateSpaceProfile::Hedge, 2, 1))
            .await,
    );
    assert_eq!(kept.checkpoint_digest, first.checkpoint_digest);
    assert_eq!(kept.cursor, 5);
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.4
#[tokio::test]
async fn budget_refusal_keeps_prior_checkpoint() {
    let mut s = Served::over(store());
    let vol = returns(7, 4097 + 10);
    assert!(s.append("ret", 1, &vol[..10]).await.error.is_none());
    let before = receipt(
        &s.define("vol", "ret", &model(StateSpaceProfile::Volatility, 1, 1))
            .await,
    );
    assert_eq!(before.cursor, 10);
    // 4097 fresh observations in one append exceed the retained window: the maintenance
    // refuses, and the checkpoint, cursor and lag stay as they were.
    assert!(s.append("ret", 1, &vol[10..]).await.error.is_none());
    assert_eq!(s.range("vol").await.len(), 10);
    let lag = receipt(
        &s.define("vol", "ret", &model(StateSpaceProfile::Volatility, 1, 1))
            .await,
    );
    assert_eq!((lag.cursor, lag.caught_up), (4096 + 10, false));
    // A model over the dimension ceiling is refused at definition, creating nothing.
    let wide = s
        .define("big", "ret", &model(StateSpaceProfile::Factor, 17, 1))
        .await;
    assert!(
        refusal(&wide).contains("KERNEL_STATE_BUDGET"),
        "{}",
        refusal(&wide)
    );
    assert!(s.range("big").await.is_empty());
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.10
#[tokio::test]
async fn stream_restart_refusals_and_hot_path_receipts() {
    let shared = store();
    let rows = returns(7, 600);
    let m = model(StateSpaceProfile::Volatility, 1, 1);

    // Streaming: one point at a time.
    let mut streaming = Served::over(shared.clone());
    streaming.append("ret", 1, &rows[..1]).await;
    streaming.define("stream", "ret", &m).await;
    for row in &rows[1..] {
        assert!(streaming
            .append("ret", 1, std::slice::from_ref(row))
            .await
            .error
            .is_none());
    }
    let streamed = receipt(&streaming.define("stream", "ret", &m).await);
    assert_eq!(streamed.cursor, 600);

    // Chunked at the checkpoint cadence (256 points), defined before the chunks.
    let mut chunked = Served::over(shared.clone());
    chunked.define("chunk", "ret", &m).await;
    let chunked_receipt = receipt(&chunked.define("chunk", "ret", &m).await);
    assert_eq!(chunked_receipt.cursor, 600, "catch-up from the full source");
    let _ = &rows;

    // Restart: a fresh server over the same durable store reads the same checkpoint.
    let mut restarted = Served::over(shared);
    let after = receipt(&restarted.define("stream", "ret", &m).await);
    assert_eq!(after.checkpoint_digest, streamed.checkpoint_digest);
    assert_eq!(
        streamed.checkpoint_digest,
        chunked_receipt.checkpoint_digest
    );
    assert_eq!(
        restarted.range("stream").await,
        restarted.range("chunk").await
    );

    // Refusals stay observable in the responses.
    let late = restarted.append("ret", 1, &[(500, vec![0.123])]).await;
    assert!(late.error.is_none(), "the source append itself commits");
    let still = receipt(&restarted.define("stream", "ret", &m).await);
    assert_eq!(
        still.cursor, 600,
        "a late point is refused by the filter, not applied"
    );
    let bad = restarted
        .define("bad", "ret", &model(StateSpaceProfile::Hedge, 2, 1))
        .await;
    assert!(
        refusal(&bad).contains("STATE_SCHEMA_UNAVAILABLE"),
        "{}",
        refusal(&bad)
    );
}
