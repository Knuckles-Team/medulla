use super::*;
use medulla_types::series_expr::SeriesFunc;

fn expr() -> SeriesExpr {
    let ewma = SeriesExpr::call(SeriesFunc::Ewma, vec![SeriesExpr::channel("v0")], vec![4.0]);
    SeriesExpr::call(SeriesFunc::Zscore, vec![ewma], vec![5.0])
}

fn source(n: i64) -> Vec<Point> {
    (1..=n)
        .map(|i| Point::single(i * 10, 50.0 + (i % 7) as f64 * 1.5 - (i % 3) as f64))
        .collect()
}

/// The on-the-fly `DERIVE` values over a whole source: `(ts, value)` where defined.
fn on_the_fly(points: &[Point]) -> Vec<(Ts, u64)> {
    let mut program = Program::compile(&expr()).unwrap();
    points
        .iter()
        .filter_map(|p| program.step(&|n| field(p, n)).map(|v| (p.ts, v.to_bits())))
        .collect()
}

fn values(points: &[Point]) -> Vec<(Ts, u64)> {
    latest_versions(points)
        .iter()
        .map(|p| (p.ts, p.values[0].to_bits()))
        .collect()
}

#[test]
fn materialised_equals_on_the_fly_for_every_prefix_and_split() {
    let all = source(700);
    for split in [1, 9, 255, 256, 257, 511, 699] {
        let mut state = DerivedState::define("s", expr()).unwrap();
        let mut stored = state.advance(&all[..split], 1, usize::MAX).points;
        let restored_bytes = state.encode().unwrap();
        let mut state = DerivedState::decode(&restored_bytes).unwrap();
        stored.extend(state.advance(&all, 2, usize::MAX).points);
        assert_eq!(values(&stored), on_the_fly(&all), "split at {split}");
        assert_eq!(values(&stored[..]).len(), on_the_fly(&all).len());
    }
}

// spec: MEDULLA-REPO-INGEST-R004
#[test]
fn the_work_budget_stops_a_step_and_the_next_continues() {
    let all = source(100);
    let mut state = DerivedState::define("s", expr()).unwrap();
    let first = state.advance(&all, 1, 30);
    assert_eq!((first.consumed, first.caught_up), (30, false));
    let second = state.advance(&all, 2, 1_000);
    assert_eq!((second.consumed, second.caught_up), (70, true));
    let mut stored = first.points;
    stored.extend(second.points);
    assert_eq!(values(&stored), on_the_fly(&all));
}

#[test]
fn a_revision_appends_new_versions_and_the_as_of_view_is_unchanged() {
    let original = source(600);
    let mut state = DerivedState::define("s", expr()).unwrap();
    let before = state.advance(&original, 1_000, usize::MAX).points;
    // Correct the source point at ts 4000 (index 399): the store appends a new version.
    let mut revised = original.clone();
    revised[399].values[0] += 7.25;
    let start = state.replay_start(4_000);
    assert_eq!(
        start,
        Some(2_560),
        "restores the checkpoint after 256 points"
    );
    let replay_source: Vec<Point> = revised
        .iter()
        .filter(|p| start.is_none_or(|s| p.ts > s))
        .cloned()
        .collect();
    let step = state
        .replay(4_000, &replay_source, &before, 2_000, usize::MAX)
        .unwrap();
    assert!(!step.points.is_empty());
    assert!(step
        .points
        .iter()
        .all(|p| p.ts >= 4_000 && p.values[1] == 1.0 && p.values[2] == 2_000.0));
    let mut log = before.clone();
    log.extend(step.points);
    assert_eq!(
        values(&log),
        on_the_fly(&revised),
        "latest view = recompute over the revised source"
    );
    assert_eq!(
        values(&as_of(&log, 1_500)),
        values(&before),
        "the earlier view is unchanged"
    );
    // The replay left a live state: new points continue from it.
    let mut more = revised.clone();
    more.extend((601..=620).map(|i| Point::single(i * 10, 40.0 + i as f64 / 9.0)));
    log.extend(state.advance(&more, 3_000, usize::MAX).points);
    assert_eq!(values(&log), on_the_fly(&more));
}

#[test]
fn a_replay_over_the_budget_is_refused_and_checkpoints_stay_bounded() {
    let all = source(MAX_CHECKPOINTS as i64 * 256 + 600);
    let mut state = DerivedState::define("s", expr()).unwrap();
    state.advance(&all, 1, usize::MAX);
    assert_eq!(state.checkpoints.len(), MAX_CHECKPOINTS);
    let err = state.replay(10, &all, &[], 2, 100).unwrap_err();
    assert!(err.contains("work budget"), "{err}");
}

mod state_space_bounds {
    use super::*;
    use crate::derive::state_space::{Advance, SeriesRefusal, StateSpaceState};
    use medulla_numeric::series::limits::{self as lim, Ceiling};
    use medulla_types::series_expr::{
        NoisePolicy, StateSpaceModel, StateSpaceProfile, STATE_SPACE_KERNEL_REVISION,
    };

    fn model(n: usize, m: usize) -> StateSpaceModel {
        let eye = |k: usize, s: f64| -> Vec<f64> {
            (0..k * k)
                .map(|i| if i / k == i % k { s } else { 0.0 })
                .collect()
        };
        StateSpaceModel {
            profile: StateSpaceProfile::Factor,
            model_revision: 1,
            kernel_revision: STATE_SPACE_KERNEL_REVISION.into(),
            state_dim: n as u32,
            obs_dim: m as u32,
            transition: eye(n, 1.0),
            process_noise: eye(n, 0.01),
            measurement_noise: eye(m, 0.5),
            initial_mean: vec![0.0; n],
            initial_covariance: eye(n, 1.0),
            noise_policy: NoisePolicy::Uncorrelated,
            symmetry_tol: 1e-9,
            numeric_policy: "ieee754-f64".into(),
        }
    }

    fn refused(r: Result<(), lim::LimitRefusal>) -> (Ceiling, &'static str) {
        let e = r.unwrap_err();
        (e.ceiling, e.code())
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.4
    #[test]
    fn state_space_resource_bounds() {
        // Exact limit passes, limit plus one is refused, for every ceiling.
        assert!(lim::check_state_dim(16, 16).is_ok());
        assert_eq!(
            refused(lim::check_state_dim(17, 16)),
            (Ceiling::StateDim, "KERNEL_STATE_BUDGET")
        );
        assert!(lim::check_obs_dim(8, 8).is_ok());
        assert_eq!(
            refused(lim::check_obs_dim(9, 8)),
            (Ceiling::ObsDim, "KERNEL_STATE_BUDGET")
        );
        assert!(lim::check_retained(4096, 4096).is_ok());
        assert_eq!(
            refused(lim::check_retained(4097, 4096)),
            (Ceiling::RetainedObs, "KERNEL_STATE_BUDGET")
        );
        assert!(lim::check_checkpoint_bytes(1_048_576, 1_048_576).is_ok());
        assert_eq!(
            refused(lim::check_checkpoint_bytes(1_048_577, 1_048_576)),
            (Ceiling::CheckpointBytes, "KERNEL_STATE_BUDGET")
        );
        assert!(lim::check_update_work(1_000_000, 1_000_000).is_ok());
        assert_eq!(
            refused(lim::check_update_work(1_000_001, 1_000_000)),
            (Ceiling::UpdateWork, "KERNEL_WORK_BUDGET")
        );
        assert!(lim::check_replay_points(100_000, 100_000).is_ok());
        assert_eq!(
            refused(lim::check_replay_points(100_001, 100_000)),
            (Ceiling::ReplayPoints, "KERNEL_WORK_BUDGET")
        );
        assert!(lim::check_hmm_refit(100_000, 100_000).is_ok());
        assert_eq!(
            refused(lim::check_hmm_refit(100_001, 100_000)),
            (Ceiling::HmmRefit, "KERNEL_STATE_BUDGET")
        );
        // A caller may lower a ceiling and never raise it.
        assert!(lim::check_state_dim(17, 99).is_err());
        assert!(lim::check_retained(10, 9).is_err());
        // The largest admitted model's update stays inside the work ceiling.
        assert!(lim::update_work(16, 8) <= lim::MAX_UPDATE_WORK);

        // The same ceilings guard the real series: dimension 16 defines, 17 refuses
        // before any allocation.
        assert!(StateSpaceState::define("s", model(16, 1)).is_ok());
        assert!(matches!(
            StateSpaceState::define("s", model(17, 1)),
            Err(SeriesRefusal::Limit(l)) if l.ceiling == Ceiling::StateDim
        ));

        // A step over the retained window refuses whole on append and keeps the prior
        // state, while a catch-up takes the window and reports the lag.
        let state = StateSpaceState::define("s", model(1, 1)).unwrap();
        let wide = |n: usize| -> Vec<Point> {
            (1..=n as i64)
                .map(|i| Point {
                    ts: i,
                    values: vec![0.5 + (i % 5) as f64, 1.0],
                })
                .collect()
        };
        let at_limit = state.advance(&wide(4096), 7, Advance::Append).unwrap();
        assert_eq!(at_limit.1.consumed, 4096);
        assert!(at_limit.1.caught_up);
        match state.advance(&wide(4097), 7, Advance::Append) {
            Err(SeriesRefusal::Limit(l)) => assert_eq!(l.code(), "KERNEL_STATE_BUDGET"),
            other => panic!("expected a retained-window refusal, got {other:?}"),
        }
        let (caught, step) = state.advance(&wide(4097), 7, Advance::CatchUp).unwrap();
        assert_eq!((step.consumed, step.caught_up), (4096, false));
        assert_eq!(caught.seen, 4096);
        assert_eq!(state.seen, 0, "the prior state is untouched");
    }
}
