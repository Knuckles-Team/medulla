//! State-space derived series (R002.9): a versioned model plus its checkpoint, advanced
//! one observation at a time by the shared `medulla_numeric` filter. No store here.
//!
//! The state lives in the series metadata beside the ordinary derived state, prefixed by
//! [`MAGIC`] so the two layouts never mix (an ordinary derived state starts with a
//! MessagePack array marker and can never start with the magic). Source point `i` of
//! the model's `obs_dim = m` and `state_dim = n` reads `values[..m]` as the measurement
//! `z` and, for every profile but volatility, `values[m..m + m*n]` as the design row `H`
//! (hedge: `[y, 1, x]` is `z = y`, `H = [1, x]`). Volatility reads one return and
//! observes `ln(r^2) + 1.2704` against `H = [1]`.

use medulla_numeric::series::limits::{self, LimitRefusal};
use medulla_numeric::series::state_space::{
    FilterRefusal, StateSpaceFilter, LOG_CHI2_BIAS, VOLATILITY_FLOOR,
};
use medulla_types::series_expr::{
    RetryOutcome, StateSpaceCheckpoint, StateSpaceModel, StateSpaceObservation, StateSpaceProfile,
    StateSpaceRefusal, STATE_SPACE_KERNEL_REVISION,
};
use serde::{Deserialize, Serialize};

use super::maintain::{Step, DERIVED_FIELDS};
use crate::point::{Point, Ts};

/// Prefix of an encoded [`StateSpaceState`].
pub const MAGIC: &[u8; 4] = b"MSS1";

/// Why one step refused; every refusal leaves the prior state untouched.
#[derive(Clone, Debug, PartialEq)]
pub enum SeriesRefusal {
    State(StateSpaceRefusal),
    Filter(FilterRefusal),
    Limit(LimitRefusal),
    /// The checkpoint belongs to another model than the one offered.
    CheckpointStale,
    /// The source cannot carry the model's observation layout.
    SchemaUnavailable(String),
}

impl SeriesRefusal {
    /// `CODE: detail`, the declared wire form.
    pub fn render(&self) -> String {
        match self {
            Self::State(r) => format!("{}: state-space input refused", r.code().as_str()),
            Self::Filter(r) => format!("{}: the filter refused the observation", r.code()),
            Self::Limit(r) => r.to_string(),
            Self::CheckpointStale => {
                "STATE_CHECKPOINT_STALE: the series holds a checkpoint of another model".into()
            }
            Self::SchemaUnavailable(why) => format!("STATE_SCHEMA_UNAVAILABLE: {why}"),
        }
    }
}

/// How many fresh observations one step may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advance {
    /// A source append: more than the retained window refuses the whole step.
    Append,
    /// A (re-)definition: takes the window and reports the lag.
    CatchUp,
}

/// A state-space series' model, checkpoint and cursor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateSpaceState {
    pub source: String,
    pub model: StateSpaceModel,
    pub checkpoint: StateSpaceCheckpoint,
    pub last: Option<StateSpaceObservation>,
    pub last_ts: Option<Ts>,
    pub seen: u64,
    pub digest: String,
}

/// The source fields a model reads per point.
pub fn input_width(model: &StateSpaceModel) -> usize {
    let (n, m) = (model.state_dim as usize, model.obs_dim as usize);
    match model.profile {
        StateSpaceProfile::Volatility => m,
        _ => m + m * n,
    }
}

/// Whether encoded metadata is a state-space state.
pub fn is_state_space(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

fn state(r: StateSpaceRefusal) -> SeriesRefusal {
    SeriesRefusal::State(r)
}

impl StateSpaceState {
    /// A fresh series over `source`: the model admitted and every ceiling checked first.
    pub fn define(source: &str, model: StateSpaceModel) -> Result<Self, SeriesRefusal> {
        model.check().map_err(state)?;
        let (n, m) = (model.state_dim as usize, model.obs_dim as usize);
        limits::check_state_dim(n, limits::MAX_STATE_DIM).map_err(SeriesRefusal::Limit)?;
        limits::check_obs_dim(m, limits::MAX_OBS_DIM).map_err(SeriesRefusal::Limit)?;
        limits::check_update_work(limits::update_work(n, m), limits::MAX_UPDATE_WORK)
            .map_err(SeriesRefusal::Limit)?;
        let json =
            rmp_serde::to_vec(&model).map_err(|_| state(StateSpaceRefusal::InputNonfinite))?;
        let digest = medulla_finance_market::market::digest::framed(
            "medulla/state-space-series",
            &[&json, STATE_SPACE_KERNEL_REVISION.as_bytes()],
        );
        let checkpoint = model.initial_checkpoint();
        let fresh = Self {
            source: source.to_string(),
            checkpoint,
            model,
            last: None,
            last_ts: None,
            seen: 0,
            digest,
        };
        fresh.encode()?;
        Ok(fresh)
    }

    /// The bytes the store keeps, refused over the checkpoint ceiling.
    pub fn encode(&self) -> Result<Vec<u8>, SeriesRefusal> {
        let mut bytes = MAGIC.to_vec();
        bytes
            .extend(rmp_serde::to_vec(self).map_err(|_| state(StateSpaceRefusal::InputNonfinite))?);
        limits::check_checkpoint_bytes(bytes.len(), limits::MAX_CHECKPOINT_BYTES)
            .map_err(SeriesRefusal::Limit)?;
        Ok(bytes)
    }

    /// Restore from [`Self::encode`] bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let body = bytes.strip_prefix(MAGIC).ok_or("not a state-space state")?;
        rmp_serde::from_slice(body).map_err(|e| e.to_string())
    }

    /// The digest of the checkpoint as stored: what the streaming, chunked and restart
    /// runs of one source must agree on.
    pub fn checkpoint_digest(&self) -> String {
        let bytes = rmp_serde::to_vec(&self.checkpoint).unwrap_or_default();
        medulla_finance_market::market::digest::framed(
            "medulla/state-space-checkpoint",
            &[&bytes, &self.seen.to_le_bytes()],
        )
    }

    /// Re-defining with another model leaves the old checkpoint stale and refused.
    pub fn same_definition(
        &self,
        source: &str,
        model: &StateSpaceModel,
    ) -> Result<(), SeriesRefusal> {
        if self.source == source && &self.model == model {
            Ok(())
        } else {
            Err(SeriesRefusal::CheckpointStale)
        }
    }

    /// The observation a source point carries at ordinal `sequence`.
    fn observation(&self, point: &Point, sequence: u64, known_at_ms: u64) -> StateSpaceObservation {
        let m = self.model.obs_dim as usize;
        let split = m.min(point.values.len());
        StateSpaceObservation {
            sequence,
            event_time_ms: point.ts,
            known_at_ms: known_at_ms as i64,
            source_revision: format!("ts:{}", point.ts),
            values: point.values[..split].to_vec(),
            design: point.values[split..].to_vec(),
        }
    }

    /// Consume the source points after `last_ts`. Returns the advanced state and the
    /// derived points; `self` is untouched on a refusal.
    pub fn advance(
        &self,
        source: &[Point],
        known_at_ms: u64,
        mode: Advance,
    ) -> Result<(Self, Step), SeriesRefusal> {
        limits::check_replay_points(source.len(), limits::MAX_REPLAY_POINTS)
            .map_err(SeriesRefusal::Limit)?;
        let width = input_width(&self.model);
        let mut next = self.clone();
        let mut fresh: Vec<&Point> = Vec::new();
        for p in source {
            if p.values.len() < width {
                return Err(SeriesRefusal::SchemaUnavailable(format!(
                    "a source point carries {} fields; the model reads {width}",
                    p.values.len()
                )));
            }
            match self.last_ts {
                Some(t) if p.ts < t => return Err(state(StateSpaceRefusal::RetryConflict)),
                Some(t) if p.ts == t => {
                    let prior = self.last.as_ref();
                    let again = self.observation(p, self.seen, known_at_ms);
                    let same =
                        prior.is_some_and(|o| o.values == again.values && o.design == again.design);
                    if !same {
                        return Err(state(StateSpaceRefusal::RetryConflict));
                    }
                }
                _ => fresh.push(p),
            }
        }
        let lag = fresh.len();
        if mode == Advance::Append {
            limits::check_retained(lag, limits::MAX_RETAINED_OBS).map_err(SeriesRefusal::Limit)?;
        }
        let take = lag.min(limits::MAX_RETAINED_OBS);
        let mut points = Vec::with_capacity(take);
        for p in &fresh[..take] {
            let obs = next.observation(p, next.seen + 1, known_at_ms);
            let value = next.apply(&obs)?;
            next.last = Some(obs);
            next.last_ts = Some(p.ts);
            next.seen += 1;
            points.push(Point {
                ts: p.ts,
                values: vec![value, 0.0, known_at_ms as f64],
            });
        }
        debug_assert_eq!(DERIVED_FIELDS.len(), 3);
        next.encode()?;
        Ok((
            next,
            Step {
                points,
                consumed: take,
                caught_up: take == lag,
            },
        ))
    }

    /// One predict-update; returns the filtered level (`mean[0]`).
    fn apply(&mut self, obs: &StateSpaceObservation) -> Result<f64, SeriesRefusal> {
        let model = &self.model;
        obs.check(model).map_err(state)?;
        if let Some(prior) = &self.last {
            if obs.retry_against(Some(prior)).map_err(state)? == RetryOutcome::Replay {
                return Ok(self.checkpoint.mean[0]);
            }
        }
        let (n, m) = (model.state_dim as usize, model.obs_dim as usize);
        let (z, h) = match model.profile {
            StateSpaceProfile::Volatility => {
                let square = obs.values[0] * obs.values[0];
                if square < VOLATILITY_FLOOR {
                    return Err(SeriesRefusal::Filter(FilterRefusal::TransformInvalid));
                }
                (vec![square.ln() + LOG_CHI2_BIAS], vec![1.0])
            }
            _ => (obs.values.clone(), obs.design.clone()),
        };
        let mut filter = StateSpaceFilter::new(
            n,
            m,
            model.transition.clone(),
            model.process_noise.clone(),
            model.measurement_noise.clone(),
            self.checkpoint.mean.clone(),
            self.checkpoint.covariance.clone(),
            model.symmetry_tol,
        )
        .map_err(SeriesRefusal::Filter)?;
        let step = filter.update(&h, &z, None).map_err(SeriesRefusal::Filter)?;
        let mut checkpoint = self.checkpoint.clone();
        checkpoint.mean = step.mean;
        checkpoint.covariance = step.covariance;
        checkpoint.cursor = obs.sequence;
        checkpoint.check(model).map_err(state)?;
        self.checkpoint = checkpoint;
        Ok(self.checkpoint.mean[0])
    }
}
