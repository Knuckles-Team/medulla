//! The repository-graph read of the planning-admission family, split out above its readers.

use medulla_h_decide_base::decide::family::planning_admission::plan_of;
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::DecideRequest,
};

/// Whether the question reads the repository graph: a `planning_admission`
/// question, or a `task_plan` question that carries a plan to admit or a skill
/// binding to check (MEDULLA-REPO-INGEST-R002.1.3).
pub fn reads_repository_graph(request: &DecideRequest) -> bool {
    matches!(
        request.question.kind,
        QuestionKind::PlanningAdmission | QuestionKind::TaskPlan
    ) && (plan_of(request).is_some()
        || crate::decide::stat_task_plan::skill_binding_of(request).is_some())
}
