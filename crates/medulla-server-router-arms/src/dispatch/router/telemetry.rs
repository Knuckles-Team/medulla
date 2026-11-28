use super::*;

/// Telemetry facts (EH-408 / EH-409): `TelemetryDerive` self-routes here,
/// reads the in-process observability store and the request graph, and
/// self-translates its facts into one gateway-routed `BatchUpdate` against the
/// request graph. It needs the `obs` store and the `shacl` schema reasoner.
///
/// Hands a method it does not own back as `ControlFlow::Continue`.
pub async fn dispatch_telemetry_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    if let Method::GetTelemetrySnapshot { tenant } = method {
        return ControlFlow::Break(dispatch_get_telemetry_snapshot(ctx, tenant).await);
    }
    let Method::TelemetryDerive {
        from_ms,
        to_ms,
        streams,
        policy_msgpack,
    } = method
    else {
        return ControlFlow::Continue(method);
    };
    #[cfg(all(feature = "obs", feature = "shacl"))]
    {
        use super::super::telemetry::{handle_telemetry_derive, DeriveRequest, DeriveTarget};

        let target = DeriveTarget {
            graph: &ctx.req.graph,
            req_id: ctx.req.id,
            caller: ctx.req.agent_id.as_deref(),
            verified: ctx.verified_context,
        };
        let request = DeriveRequest {
            from_ms,
            to_ms,
            streams,
            policy_msgpack,
        };
        ControlFlow::Break(
            dispatch_boxed(handle_telemetry_derive(ctx.state, target, request)).await,
        )
    }
    #[cfg(not(all(feature = "obs", feature = "shacl")))]
    {
        let _ = (from_ms, to_ms, streams, policy_msgpack);
        ControlFlow::Break(Response::err(
            ctx.req.id,
            "TelemetryDerive requires the `obs` and `shacl` features",
        ))
    }
}

/// `GetTelemetrySnapshot` (MEDULLA-ORCH-VOCAB-R804): the verified tenant's latest
/// stored metric values by name. A tenant other than the verified one refuses.
async fn dispatch_get_telemetry_snapshot(ctx: DispatchCtx<'_>, tenant: String) -> Response {
    let req_id = ctx.req.id;
    if tenant != ctx.verified_context.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: the snapshot tenant is not the verified request tenant",
        );
    }
    #[cfg(all(feature = "obs", feature = "promql"))]
    {
        use medulla_server_obs::obs::ObsStateExt;

        let obs = medulla_server_graph::dispatch::shared::timed_read(ctx.state)
            .await
            .obs();
        let Some(obs) = obs else {
            return Response::err(
                req_id,
                "TELEMETRY_UNAVAILABLE: this engine has no observability store configured",
            );
        };
        let window_secs = medulla_server_api::knob_config::knobs().telemetry_snapshot_window;
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let snapshot = tokio::task::spawn_blocking(move || {
            medulla_server_obs::obs::telemetry_snapshot::tenant_metric_snapshot(
                &obs,
                &tenant,
                now_ms,
                window_secs,
            )
        })
        .await
        .map_err(|_| "telemetry read worker failed".to_string())
        .and_then(|outcome| outcome);
        medulla_server_graph::dispatch::shared::typed_response::<
            medulla_protocol_types::result_contract::ingestion::GetTelemetrySnapshot,
        >(req_id, snapshot)
    }
    #[cfg(not(all(feature = "obs", feature = "promql")))]
    {
        Response::err(
            req_id,
            "GetTelemetrySnapshot requires the `obs` and `promql` features",
        )
    }
}
