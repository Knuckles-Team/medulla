use super::*;

/// Served incidents (MEDULLA-UNIFIED-DATA-PLANE-R760.3): the five incident
/// methods self-route here. Writes self-translate into one gateway-routed
/// `BatchUpdate` against the request graph; reads commit nothing. They need
/// the incident authority of `medulla-stream`.
///
/// Hands a method it does not own back as `ControlFlow::Continue`.
pub async fn dispatch_incident_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    if !matches!(
        method,
        Method::IncidentRecordAnomaly { .. }
            | Method::IncidentCorrelate { .. }
            | Method::IncidentEvidence { .. }
            | Method::IncidentRelatedClaims { .. }
            | Method::IncidentSetStatus { .. }
    ) {
        return ControlFlow::Continue(method);
    }
    #[cfg(any(feature = "obs", feature = "stream"))]
    {
        use super::super::incident::{
            handle_incident_correlate, handle_incident_evidence, handle_incident_record_anomaly,
            handle_incident_related_claims, handle_incident_set_status, IncidentTarget,
        };

        let target = IncidentTarget {
            graph: &ctx.req.graph,
            req_id: ctx.req.id,
            caller: ctx.req.agent_id.as_deref(),
            verified: ctx.verified_context,
        };
        let state = ctx.state;
        let response = match method {
            Method::IncidentRecordAnomaly { anomaly, window_ms } => {
                dispatch_boxed(handle_incident_record_anomaly(
                    state, target, anomaly, window_ms,
                ))
                .await
            }
            Method::IncidentCorrelate {
                anomalies,
                window_ms,
            } => {
                dispatch_boxed(handle_incident_correlate(
                    state, target, anomalies, window_ms,
                ))
                .await
            }
            Method::IncidentEvidence { incident_id } => {
                dispatch_boxed(handle_incident_evidence(state, target, incident_id)).await
            }
            Method::IncidentRelatedClaims { incident_id, limit } => {
                dispatch_boxed(handle_incident_related_claims(
                    state,
                    target,
                    incident_id,
                    limit,
                ))
                .await
            }
            Method::IncidentSetStatus {
                incident_id,
                status,
            } => {
                dispatch_boxed(handle_incident_set_status(
                    state,
                    target,
                    incident_id,
                    status,
                ))
                .await
            }
            other => return ControlFlow::Continue(other),
        };
        ControlFlow::Break(response)
    }
    #[cfg(not(any(feature = "obs", feature = "stream")))]
    {
        ControlFlow::Break(Response::err(
            ctx.req.id,
            "ENGINE_UNAVAILABLE: the incident methods require the `obs` or `stream` feature",
        ))
    }
}
