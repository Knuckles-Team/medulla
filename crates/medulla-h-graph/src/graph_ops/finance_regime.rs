//! `Method::RegimeForecast` and `Method::RegimeSizingInput`
//! (MEDULLA-FINANCE-PRIMITIVES-R002.11.0 to R002.11.3).
//!
//! Terminal reads over the RLS-projected core. The handler reads the ordered state
//! ids, the posterior `p0` and the transition matrix `T` only from the pinned
//! `RegimeModel` record (`finance:RegimeModel:<model_id>`, written through
//! `FinanceRecordPut`) of the verified tenant, refuses a stale pin, forecasts
//! `p_h = p0 * T^h` through `medulla_numeric::evaluation::regime_forecast`, scores
//! the per-horizon Brier gate when the request carries held-out samples, and for
//! the sizing input applies a versioned policy. Nothing is written and no order
//! is created.

use super::finance_accounting::{datetime_ns, literal};
use super::finance_regime_gate::{calibrate, digest_of, size, GateModel, Refusal};
use super::terminal::GraphOpsContext;
use super::*;

use medulla_numeric::evaluation::regime_forecast::{forecast, RegimeForecastRefusal};
use medulla_types::regime_forecast::{
    RegimeForecastRequest, RegimeForecastResult, RegimeRevisions, RegimeSizingPolicy,
    RegimeSizingResult, SizingAbstainReason, FORECAST_INPUT_SUPPLIED, FORECAST_MODEL_STALE,
    REGIME_MODEL_NOT_FOUND, SIZING_ABSTAIN,
};

const TENANT_SCOPE_KEY: &str = "finance_tenant_scope";

fn refuse(req_id: u64, (code, detail): Refusal) -> Response {
    Response::err(req_id, format!("{code}: {detail}"))
}

fn invalid(req_id: u64, detail: impl std::fmt::Display) -> Response {
    Response::err(req_id, format!("INVALID_ARGUMENT: {detail}"))
}

/// The pinned model as stored.
struct PinnedModel {
    revision: String,
    source_series_id: String,
    source_revision: String,
    as_of: String,
    state_ids: Vec<String>,
    p0: Vec<f64>,
    matrix: Vec<Vec<f64>>,
}

fn read_model(core: &GraphCore, scope: &str, model_id: &str) -> Result<PinnedModel, Refusal> {
    let missing = || {
        (
            REGIME_MODEL_NOT_FOUND,
            format!("no regime model {model_id} for the caller"),
        )
    };
    let node_id = format!("finance:RegimeModel:{model_id}");
    let props = core
        .get_node_properties(&node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
        .filter(|props| props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) == Some(scope))
        .ok_or_else(missing)?;
    let fields = props
        .get("fields")
        .and_then(|fields| fields.as_object())
        .ok_or_else(missing)?;
    let text = |name: &str| {
        fields
            .get(name)
            .and_then(|term| term.as_str())
            .and_then(literal)
    };
    let unreadable = |name: &str| {
        (
            REGIME_MODEL_NOT_FOUND,
            format!("regime model {model_id} has an unreadable {name}"),
        )
    };
    let json = |name: &str| -> Result<serde_json::Value, Refusal> {
        text(name)
            .and_then(|t| serde_json::from_str(&t).ok())
            .ok_or_else(|| unreadable(name))
    };
    let string = |name: &str| text(name).ok_or_else(|| unreadable(name));
    let state_ids = serde_json::from_value(json("regimeStateIds")?)
        .map_err(|_| unreadable("regimeStateIds"))?;
    let p0 = serde_json::from_value(json("regimePosterior")?)
        .map_err(|_| unreadable("regimePosterior"))?;
    let matrix = serde_json::from_value(json("regimeTransitionMatrix")?)
        .map_err(|_| unreadable("regimeTransitionMatrix"))?;
    Ok(PinnedModel {
        revision: string("regimeModelRevision")?,
        source_series_id: string("regimeSourceSeriesId")?,
        source_revision: string("regimeSourceRevision")?,
        as_of: string("regimeAsOf")?,
        state_ids,
        p0,
        matrix,
    })
}

/// A forecast with the model it came from, so the sizing input can reuse it.
struct Forecasted {
    result: RegimeForecastResult,
    top_state_probability: Vec<f64>,
}

fn from_numeric(refusal: RegimeForecastRefusal) -> Refusal {
    let code = match refusal {
        RegimeForecastRefusal::HorizonInvalid(_) => {
            medulla_types::regime_forecast::FORECAST_HORIZON_INVALID
        }
        RegimeForecastRefusal::MatrixInvalid(_) => {
            medulla_types::regime_forecast::FORECAST_MATRIX_INVALID
        }
    };
    (code, refusal.detail().to_owned())
}

fn run_forecast(
    core: &GraphCore,
    scope: &str,
    request: &RegimeForecastRequest,
) -> Result<Forecasted, Refusal> {
    if request.supplied_p0.is_some() || request.supplied_transition_matrix.is_some() {
        return Err((
            FORECAST_INPUT_SUPPLIED,
            "p0 and the transition matrix come only from the pinned model".to_owned(),
        ));
    }
    let knobs = medulla_server_api::knob_config::knobs();
    let max_horizon = u32::try_from(knobs.finance_regime_forecast_max_horizon).unwrap_or(u32::MAX);
    let tolerance = knobs.finance_regime_forecast_sum_tolerance;
    medulla_numeric::evaluation::regime_forecast::validate_horizons(&request.horizons, max_horizon)
        .map_err(from_numeric)?;
    for (name, value) in [
        ("model_id", &request.model_id),
        ("model_revision", &request.model_revision),
        ("source_series_id", &request.source_series_id),
        ("source_revision", &request.source_revision),
        ("horizon_unit", &request.horizon_unit),
    ] {
        if value.is_empty() {
            return Err(("INVALID_ARGUMENT", format!("{name} is empty")));
        }
    }
    let Some(as_of_ns) = datetime_ns(&request.as_of) else {
        return Err((
            "INVALID_ARGUMENT",
            "as_of must be an RFC 3339 date-time".to_owned(),
        ));
    };
    let model = read_model(core, scope, &request.model_id)?;
    let same_instant = datetime_ns(&model.as_of) == Some(as_of_ns);
    if model.revision != request.model_revision
        || model.source_series_id != request.source_series_id
        || model.source_revision != request.source_revision
        || !same_instant
    {
        return Err((
            FORECAST_MODEL_STALE,
            format!(
                "the pinned model is at revision {} (source revision {}, as-of {}) and does not match the request",
                model.revision, model.source_revision, model.as_of
            ),
        ));
    }
    let vectors = forecast(
        &model.state_ids,
        &model.p0,
        &model.matrix,
        &request.horizons,
        max_horizon,
        tolerance,
    )
    .map_err(from_numeric)?;
    let top: Vec<f64> = vectors
        .iter()
        .map(|v| v.iter().copied().fold(0.0, f64::max))
        .collect();
    let calibration = match &request.calibration {
        None => Vec::new(),
        Some(input) => calibrate(
            input,
            &GateModel {
                request,
                state_ids: &model.state_ids,
                as_of_ns,
                top_state_probability: &top,
                max_brier_ceiling: knobs.finance_regime_forecast_max_brier,
            },
        )?,
    };
    let matrix_digest = digest_of(&model.matrix);
    let revisions = RegimeRevisions {
        model_revision: model.revision.clone(),
        source_revision: model.source_revision.clone(),
    };
    let forecast_digest = digest_of(&(
        &request.model_id,
        &revisions,
        &request.as_of,
        &model.state_ids,
        &model.p0,
        &matrix_digest,
        &request.horizons,
        &request.horizon_unit,
        &vectors,
    ));
    Ok(Forecasted {
        top_state_probability: top.clone(),
        result: RegimeForecastResult {
            forecast_digest,
            revisions,
            as_of: request.as_of.clone(),
            horizon_unit: request.horizon_unit.clone(),
            horizons: request.horizons.clone(),
            ordered_state_ids: model.state_ids,
            posterior_p0: model.p0,
            transition_matrix_digest: matrix_digest,
            per_horizon_probability_vectors: vectors,
            per_horizon_top_state_probability: top,
            calibration,
        },
    })
}

fn scope_of(ctx: &GraphOpsContext<'_>) -> Result<String, Response> {
    ctx.read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_owned())
        .ok_or_else(|| Response::err(ctx.req_id, "ACCESS_DENIED: no verified tenant"))
}

fn handle_forecast(ctx: GraphOpsContext<'_>, request: &RegimeForecastRequest) -> Response {
    let scope = match scope_of(&ctx) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    match run_forecast(ctx.core, &scope, request) {
        Ok(forecasted) => Response::ok(
            ctx.req_id,
            ResultPayload::of_ref::<medulla_protocol_types::result_contract::graph::RegimeForecast>(
                &forecasted.result,
            ),
        ),
        Err(("INVALID_ARGUMENT", detail)) => invalid(ctx.req_id, detail),
        Err(refusal) => refuse(ctx.req_id, refusal),
    }
}

fn abstain(req_id: u64, reason: SizingAbstainReason, detail: impl std::fmt::Display) -> Response {
    Response::err(
        req_id,
        format!("{SIZING_ABSTAIN}: {}: {detail}", reason.code()),
    )
}

fn handle_sizing(
    ctx: GraphOpsContext<'_>,
    request: &RegimeForecastRequest,
    policy: &RegimeSizingPolicy,
    forecast_digest: &str,
) -> Response {
    let scope = match scope_of(&ctx) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    if policy.policy_id.is_empty()
        || policy.revision.is_empty()
        || policy.instrument_id.is_empty()
        || policy.unit.is_empty()
        || policy.fixed_point_scale <= 0
        || policy.risk_cap < 0
    {
        return invalid(
            ctx.req_id,
            "the sizing policy needs an id, revision, instrument, unit, a positive scale and a non-negative risk cap",
        );
    }
    let forecasted = match run_forecast(ctx.core, &scope, request) {
        Ok(forecasted) => forecasted,
        Err((FORECAST_MODEL_STALE, detail)) => {
            return abstain(ctx.req_id, SizingAbstainReason::ForecastStale, detail)
        }
        Err(("INVALID_ARGUMENT", detail)) => return invalid(ctx.req_id, detail),
        Err(refusal) => return refuse(ctx.req_id, refusal),
    };
    let result = &forecasted.result;
    if result.forecast_digest != forecast_digest {
        return abstain(
            ctx.req_id,
            SizingAbstainReason::ForecastStale,
            "the forecast digest does not match the forecast recomputed from the request",
        );
    }
    if result.calibration.is_empty() || result.calibration.iter().any(|h| !h.passed) {
        return abstain(
            ctx.req_id,
            SizingAbstainReason::CalibrationFailed,
            "every requested horizon needs a passing calibration gate",
        );
    }
    let per_horizon = match size(
        policy,
        &result.ordered_state_ids,
        &result.horizons,
        &result.per_horizon_probability_vectors,
    ) {
        Ok(per_horizon) => per_horizon,
        Err((reason, detail)) => return abstain(ctx.req_id, reason, detail),
    };
    let sized = RegimeSizingResult {
        forecast_digest: result.forecast_digest.clone(),
        sizing_policy_digest: digest_of(policy),
        instrument_id: policy.instrument_id.clone(),
        unit: policy.unit.clone(),
        fixed_point_scale: policy.fixed_point_scale,
        per_horizon,
    };
    Response::ok(
        ctx.req_id,
        ResultPayload::of_ref::<medulla_protocol_types::result_contract::graph::RegimeSizingInput>(
            &sized,
        ),
    )
}

/// Route the regime forecast and sizing input from the graph terminal read chain.
pub(super) async fn handle_regime(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    match &method {
        Method::RegimeForecast { request } => ControlFlow::Break(handle_forecast(ctx, request)),
        Method::RegimeSizingInput {
            request,
            policy,
            forecast_digest,
        } => ControlFlow::Break(handle_sizing(ctx, request, policy, forecast_digest)),
        _ => ControlFlow::Continue(method),
    }
}
