use super::*;

/// Served quality rules (MEDULLA-UNIFIED-DATA-PLANE-R762.3): `QualityRulePut` self-routes
/// here and self-translates into one gateway-routed `BatchUpdate` against the request
/// graph.
///
/// Hands a method it does not own back as `ControlFlow::Continue`.
pub async fn dispatch_quality_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::QualityRulePut { rule } = method else {
        return ControlFlow::Continue(method);
    };
    use super::super::quality::{handle_quality_rule_put, QualityTarget};

    let target = QualityTarget {
        graph: &ctx.req.graph,
        req_id: ctx.req.id,
        caller: ctx.req.agent_id.as_deref(),
        verified: ctx.verified_context,
    };
    ControlFlow::Break(dispatch_boxed(handle_quality_rule_put(ctx.state, target, rule)).await)
}
