//! The task-plan reading of a request, split out above the family registry it consults.

use medulla_h_decide_base::decide::stat_executor::ExecutionContext;
use medulla_h_decide_base::decide::stat_support::resolve_policy;
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::DecideRequest,
};
use {
    medulla_h_decide_runlog::decide::stat_task_plan::check_skill_binding,
    medulla_h_decide_runlog::decide::stat_task_plan::is_plan_param,
    medulla_h_decide_runlog::decide::stat_task_plan::task_plan,
    medulla_h_decide_runlog::decide::stat_task_plan::with_skill_trees,
    medulla_h_decide_runlog::decide::stat_task_plan::Planned,
};

/// The plan of a request that carries one, resolving the decision policy its
/// knobs are read from. Other requests have no plan and read nothing.
pub fn plan_of_request(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    principal: &str,
    graph_view: Option<&medulla_graph_core::graph::GraphView>,
) -> Result<Option<Planned>, String> {
    if request.question.kind == QuestionKind::ProcessPlan {
        let resolved = resolve_policy(ctx.store, ctx.tenant_id, &request.policy)?;
        return super::family::process_plan::plan(ctx, request, &resolved.policy, graph_view)
            .map(Some);
    }
    if request.question.kind != QuestionKind::TaskPlan
        && !request.params.iter().any(|p| is_plan_param(&p.value))
    {
        return Ok(None);
    }
    let resolved = resolve_policy(ctx.store, ctx.tenant_id, &request.policy)?;
    match task_plan(ctx, request, &resolved.policy)? {
        Some(Planned::Plan(plan, notes)) => {
            check_skill_binding(ctx, request, graph_view, &plan)?;
            let plan = with_skill_trees(ctx, request, principal, &resolved.policy, *plan)?;
            Ok(Some(Planned::Plan(Box::new(plan), notes)))
        }
        other => Ok(other),
    }
}
