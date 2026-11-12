//! Belief slices of one served decision (EH-297): `BELIEF AS OF t`.
//!
//! A `Decide` that names `belief_as_of` times gets, besides its decision, the
//! pinned head's distribution over the SAME visible options at each time:
//! the features are recomputed with the feature clock at `t` (every
//! time-dependent feature -- `AgeSeconds` -- reads that clock), the head is
//! read over each slice, and each slice's matrix is stored in the record so
//! the log's verify-replay re-derives the belief exactly. A slice re-reads
//! the options the decision already has; it never adds one. Outcome-rate
//! features are read at the decision's clock, not sliced.

use medulla_decision_stats::decision::candidate::CandidateView;
use medulla_decision_stats::decision::quant::q32;
use medulla_decision_stats::{
    decision::features::feature_matrix, decision::features::FeatureInputs,
    decision::features::MatrixOutcome,
};
use medulla_decision_stats::{decision::trajectory::belief, decision::trajectory::Prefix};
use medulla_decision_types::{
    decision::statistical::belief::BeliefPoint, decision::statistical::belief::BeliefSliceInputs,
};
use medulla_decision_types::{
    decision::statistical::DecideRequest, decision::statistical::StatisticalErrorCode,
};
use medulla_types::contract::BoundedVec;

use super::stat_executor::{ExecutionContext, Pinned};
use super::stat_support::refusal;

/// A decision's belief slices: their stored inputs and the head's belief.
#[derive(Debug, Clone, Default)]
pub(super) struct Slices {
    pub(super) inputs: Vec<BeliefSliceInputs>,
    pub(super) points: Vec<BeliefPoint>,
}

fn check_request(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    pinned: &Pinned,
) -> Result<(), String> {
    let times = request.belief_as_of.as_slice();
    if pinned.head.is_none() {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "belief slices read a head: pin one",
        ));
    }
    let increasing = times.windows(2).all(|w| w[0] < w[1]);
    if !increasing || times.last().is_some_and(|&t| t > ctx.now_ms) {
        return Err(refusal(
            StatisticalErrorCode::ParameterInvalid,
            "belief_as_of must be strictly increasing and not after the decision's clock",
        ));
    }
    Ok(())
}

/// The graph a graph-sourced decision reads its candidates from.
#[derive(Clone, Copy)]
pub(super) struct GraphSource<'a> {
    #[cfg(feature = "epistemic")]
    view: &'a medulla_graph_core::graph::GraphView,
    #[cfg(feature = "epistemic")]
    plan: &'a medulla_types::wire::Plan,
    #[cfg(not(feature = "epistemic"))]
    _marker: std::marker::PhantomData<&'a ()>,
}

/// The graph source of a request, when it has one: library and declared
/// sources hold no transaction-time history and recompute on the clock.
#[cfg(feature = "epistemic")]
fn graph_source<'a>(
    request: &'a DecideRequest,
    view: Option<&'a medulla_graph_core::graph::GraphView>,
) -> Option<GraphSource<'a>> {
    match (&request.candidates, view) {
        (
            medulla_decision_types::decision::statistical::CandidateSource::Graph { plan, .. },
            Some(view),
        ) => Some(GraphSource { view, plan }),
        _ => None,
    }
}

#[cfg(not(feature = "epistemic"))]
fn graph_source<'a>(
    _request: &'a DecideRequest,
    _view: Option<&'a medulla_graph_core::graph::GraphView>,
) -> Option<GraphSource<'a>> {
    None
}

/// The ids a graph candidate plan returns when it ends in `BELIEF AS OF
/// as_of_ms`, sorted and de-duplicated, over the caller's RLS-filtered view
/// (MEDULLA-DECISION-ENGINE-R088.1).
#[cfg(feature = "epistemic")]
pub(super) fn belief_rows_at(
    view: &medulla_graph_core::graph::GraphView,
    plan: &medulla_types::wire::Plan,
    as_of_ms: u64,
) -> Result<Vec<String>, String> {
    let mut plan = plan.clone();
    // BELIEF AS OF over an empty input is a source of every live row; the
    // candidate plan's own rows are the only ones a slice may read.
    plan.ops.push(medulla_types::wire::Op::BeliefAsOf {
        ts: as_of_ms as f64,
    });
    let semantic = medulla_semantic_index::semantic::SemanticStore::new();
    let rows =
        medulla_exec::exec::execute(&plan, &medulla_exec::exec::PlanCtx::new(view, &semantic))
            .map_err(|detail| refusal(StatisticalErrorCode::CandidatePlanRefused, detail))?;
    let mut ids = rows.ids();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

fn slice_inputs(
    request: &DecideRequest,
    pinned: &Pinned,
    views: &[CandidateView],
    as_of_ms: u64,
    graph: Option<GraphSource<'_>>,
) -> Result<BeliefSliceInputs, String> {
    let (rows_digest, present) = match graph {
        #[cfg(feature = "epistemic")]
        Some(source) => {
            let ids = if views.is_empty() {
                Vec::new()
            } else {
                belief_rows_at(source.view, source.plan, as_of_ms)?
            };
            let present = views.iter().all(|v| ids.binary_search(&v.id).is_ok());
            let digest = medulla_query_types::solve_scalar::digest_text(
                "eg/decide-belief-slice-rows/v1",
                &(as_of_ms, &ids),
            );
            (Some(digest), present)
        }
        _ => (None, true),
    };
    let features = FeatureInputs {
        params: request.params.as_slice(),
        now_ms: as_of_ms,
    };
    let values = if !present {
        Vec::new() // a candidate row absent at t: UnknownFact for the slice
    } else {
        match feature_matrix(&pinned.schema, views, &features).map_err(|r| r.render())? {
            MatrixOutcome::Complete(matrix) => matrix.values,
            MatrixOutcome::UnknownFact { .. } => Vec::new(),
        }
    };
    Ok(BeliefSliceInputs {
        as_of_ms,
        values: BoundedVec::new(values)
            .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?,
        rows_digest,
    })
}

fn point(as_of_ms: u64, probabilities: Option<Vec<f64>>) -> Result<BeliefPoint, String> {
    let probabilities = probabilities
        .map(|p| {
            let quantised = p
                .into_iter()
                .map(q32)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|r| r.render())?;
            BoundedVec::new(quantised)
                .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))
        })
        .transpose()?;
    Ok(BeliefPoint {
        as_of_ms,
        probabilities,
    })
}

/// The pinned head's belief over `options` options at every stored slice:
/// the served path and the verify-replay both answer through here.
pub(super) fn points(
    pinned: &Pinned,
    inputs: &[BeliefSliceInputs],
    options: usize,
) -> Result<Vec<BeliefPoint>, String> {
    let Some((head, _)) = &pinned.head else {
        return Ok(Vec::new());
    };
    let width = pinned.schema.names().len().max(1);
    let matrices: Vec<Vec<&[i64]>> = inputs
        .iter()
        .map(|slice| slice.values.as_slice().chunks(width).collect())
        .collect();
    let prefixes: Vec<Prefix> = inputs
        .iter()
        .zip(&matrices)
        .map(|(slice, rows)| Prefix {
            as_of_ms: slice.as_of_ms,
            rows,
        })
        .collect();
    belief(head, &prefixes, options)
        .map_err(|r| r.render())?
        .into_iter()
        .map(|b| point(b.as_of_ms, b.probabilities))
        .collect()
}

/// The belief slices a request asks for, over the decision's visible options.
pub(super) fn live_slices(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    pinned: &Pinned,
    views: &[CandidateView],
    graph_view: Option<&medulla_graph_core::graph::GraphView>,
) -> Result<Slices, String> {
    if request.belief_as_of.is_empty() {
        return Ok(Slices::default());
    }
    check_request(ctx, request, pinned)?;
    let inputs = request
        .belief_as_of
        .iter()
        .map(|&t| slice_inputs(request, pinned, views, t, graph_source(request, graph_view)))
        .collect::<Result<Vec<_>, _>>()?;
    let points = points(pinned, &inputs, views.len())?;
    Ok(Slices { inputs, points })
}
