//! The decision plane refusal-code registry: every family table, every module
//! table, and the lookup of a code by its token (MEDULLA-DECISION-ENGINE-R010.100.1).
//!
//! The code constants themselves live in [`super::codes`]; this module is the
//! one place that names every module that declares a table.

use super::codes::*;
use super::statistical::QuestionKind;
use medulla_types::refusal_code::RefusalCode;

/// The `REFUSALS` table of every registered family, keyed by its question
/// kind. A family module joins this list in the change that declares its table.
pub const FAMILY_TABLES: &[(QuestionKind, &[RefusalCode])] = &[
    (QuestionKind::Rank, RANK),
    (QuestionKind::ConnectorWriteBack, CONNECTOR_WRITE_BACK),
    (QuestionKind::ResolveEntity, RESOLVE_ENTITY),
    (QuestionKind::FeatureMatch, super::feature_match::REFUSALS),
    (
        QuestionKind::ContextPackage,
        super::context_package::REFUSALS,
    ),
    (QuestionKind::ProbeVerdict, PROBE_VERDICT),
    (QuestionKind::ProbeSelection, PROBE_SELECTION),
    (QuestionKind::Route, ROUTE),
    (QuestionKind::RoundVerdict, ROUND_VERDICT),
    (QuestionKind::StageAllocation, STAGE_ALLOCATION),
    (QuestionKind::CriterionReview, CRITERION_REVIEW),
    (
        QuestionKind::GeneratedCodeAdmission,
        GENERATED_CODE_ADMISSION,
    ),
    (QuestionKind::CredentialReuse, CREDENTIAL_REUSE),
    (
        QuestionKind::ScaleIntent,
        super::statistical::scale::REFUSALS,
    ),
    (QuestionKind::RecursionMode, RECURSION_SIGNALS),
    (QuestionKind::SubCallAdmission, super::sub_call::REFUSALS),
    (
        QuestionKind::RetrievalPlan,
        super::statistical::run_credit::REFUSALS,
    ),
    (
        QuestionKind::RetrievalPlan,
        super::statistical::retrieval_plan::REFUSALS,
    ),
    (
        QuestionKind::RetrievalPlan,
        super::statistical::feedback::REFUSALS,
    ),
    (QuestionKind::ThoughtSearch, super::thought_search::REFUSALS),
    (QuestionKind::CommandConfig, super::command_config::REFUSALS),
    (QuestionKind::GraphIntake, super::graph_intake::REFUSALS),
    (QuestionKind::ToolNecessity, super::tool_necessity::REFUSALS),
    (
        QuestionKind::DecisionTableEval,
        super::process_decision::REFUSALS,
    ),
    (
        QuestionKind::BranchSelect,
        super::process_decision::REFUSALS,
    ),
    (
        QuestionKind::ItemRecommendation,
        super::item_recommendation::REFUSALS,
    ),
    (QuestionKind::RunAdjustment, super::run_adjustment::REFUSALS),
    (QuestionKind::PathPreference, super::path_success::REFUSALS),
    (
        QuestionKind::Placement,
        super::statistical::placement::REFUSALS,
    ),
    (
        QuestionKind::CorrectionAdmission,
        super::trace_fix_binding::REFUSALS,
    ),
    (
        QuestionKind::ContextReplacement,
        super::context_replacement::REFUSALS,
    ),
    (
        QuestionKind::TaskPlan,
        super::task_plan::trajectory_budget::REFUSALS,
    ),
    (
        QuestionKind::TaskPlan,
        super::task_plan::trajectory_projection::REFUSALS,
    ),
    (
        QuestionKind::TaskPlan,
        super::task_plan::branch_breadth::REFUSALS,
    ),
    (QuestionKind::TaskPlan, super::task_plan::pairwise::REFUSALS),
    (QuestionKind::ProgramLookup, super::program_store::REFUSALS),
    (
        QuestionKind::SkillWorkflowCompile,
        super::skill_workflow::REFUSALS,
    ),
    (
        QuestionKind::FeedbackLoop,
        super::checker_feedback::REFUSALS,
    ),
    (
        QuestionKind::SkillUpgrade,
        super::skill_upgrade_request::REFUSALS,
    ),
    (
        QuestionKind::PlanningAdmission,
        super::planning_admission::REFUSALS,
    ),
    (QuestionKind::EditConcentration, CONCENTRATION_REFUSALS),
    (QuestionKind::ContextBudget, super::context_source::REFUSALS),
    (QuestionKind::A2aRoute, super::a2a_route::REFUSALS),
    (QuestionKind::TeamCoordination, TEAM_REFUSALS),
    (
        QuestionKind::PaperAdmission,
        super::paper_admission::REFUSALS,
    ),
    (QuestionKind::HeldItemAdmission, super::held_item::REFUSALS),
    (
        QuestionKind::ServingTechniqueActivation,
        super::serving_technique::REFUSALS,
    ),
    (
        QuestionKind::ReasoningParadigm,
        super::statistical::reasoning_paradigm::REFUSALS,
    ),
    (
        QuestionKind::CapacityProposal,
        super::statistical::capacity::REFUSALS,
    ),
    (
        QuestionKind::ProcessRemediation,
        super::process_remediation::REFUSALS,
    ),
    (
        QuestionKind::ProcessPlan,
        super::process_plan::PROCESS_PLAN_REFUSALS,
    ),
    (
        QuestionKind::ToolActionAdmission,
        super::tool_action::REFUSALS,
    ),
    (
        QuestionKind::InstructionDisclosure,
        super::instruction_disclosure::REFUSALS,
    ),
    (
        QuestionKind::ComparativeAnalysisPlan,
        super::comparative_analysis::REFUSALS,
    ),
    (
        QuestionKind::ChangesetReview,
        super::changeset_review::REFUSALS,
    ),
    (QuestionKind::TemporalBasis, super::temporal_query::REFUSALS),
    (
        QuestionKind::FieldShielding,
        super::field_shielding::REFUSALS,
    ),
    (QuestionKind::RuntimeQuota, super::run_governance::REFUSALS),
    (QuestionKind::TaskPlan, super::task_plan::TASK_PLAN_REFUSALS),
    (QuestionKind::ToolRelevance, super::tool_relevance::REFUSALS),
    (QuestionKind::ResearchNext, super::research_voi::REFUSALS),
    (
        QuestionKind::InsightSelection,
        super::program_store::insight::REFUSALS,
    ),
    (QuestionKind::BranchOutcome, super::branch_fusion::REFUSALS),
    (QuestionKind::AutonomyVerdict, super::autonomy::REFUSALS),
    (
        QuestionKind::SkillEvolution,
        super::skill_evolution::REFUSALS,
    ),
    (
        QuestionKind::FanoutAggregate,
        super::task_plan::fanout::REFUSALS,
    ),
    (
        QuestionKind::DelegationAdmission,
        super::delegation::REFUSALS,
    ),
    (
        QuestionKind::CapabilityDesignation,
        super::capability_designation::REFUSALS,
    ),
    (QuestionKind::LoadSet, super::load_set::REFUSALS),
    (QuestionKind::UsageGuidance, super::usage_guidance::REFUSALS),
    (
        QuestionKind::AdversarialAdmission,
        super::trajectory_verdict::REFUSALS,
    ),
    (QuestionKind::PlaybookTriage, super::playbook::REFUSALS),
    (
        QuestionKind::TrajectoryJoin,
        super::trajectory_verdict::JOIN_REFUSALS,
    ),
    (
        QuestionKind::StepVerification,
        super::trajectory_verdict::VERIFY_REFUSALS,
    ),
    (QuestionKind::RewardDecomposition, super::reward::REFUSALS),
    (
        QuestionKind::CurriculumPromotion,
        super::curriculum::REFUSALS,
    ),
    (
        QuestionKind::PopulationDrift,
        super::population::DRIFT_REFUSALS,
    ),
    (
        QuestionKind::FanoutWidth,
        super::population::FANOUT_REFUSALS,
    ),
    (
        QuestionKind::TrajectoryLadder,
        super::trajectory_ladder::REFUSALS,
    ),
    (
        QuestionKind::AnalystSynthesis,
        super::analyst_synthesis::REFUSALS,
    ),
    (
        QuestionKind::ResolveConflict,
        super::resolve_conflict::REFUSALS,
    ),
    (QuestionKind::CouncilCoordination, super::council::REFUSALS),
    (QuestionKind::MergeArming, super::merge_arming::REFUSALS),
    (
        QuestionKind::RevertOrFixForward,
        super::revert_or_fix_forward::REFUSALS,
    ),
];

/// The `REFUSALS` table of every module that declares one in a crate below the
/// decision plane, keyed by module name. A module joins this list in the change
/// that declares its table; the uniqueness test collects it with every family
/// table (MEDULLA-DECISION-ENGINE-R010.100.1).
pub fn module_tables() -> Vec<(&'static str, Vec<RefusalCode>)> {
    use crate::{belief_revision, claims, context_facts, source_toc};
    use medulla_catalog_source_types::asset_usage;
    use medulla_source_types::meeting_transcript;
    use medulla_types::{
        archimate_exchange, bookmark_query, media_tag, paper_ingest, participant,
        transition_refused,
    };
    let codes = |pairs: &[(RefusalCode, &str)]| pairs.iter().map(|(code, _)| *code).collect();
    vec![
        ("memory_change", super::memory_benchmark::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R144.1, R144.2: the indexless retrieval refusals.
        ("indexless", super::indexless_retrieval::REFUSALS.to_vec()),
        ("safety", super::safety::REFUSALS.to_vec()),
        ("limits", super::limit_types::REFUSALS.to_vec()),
        ("entry_trust", super::entry_types::REFUSALS.to_vec()),
        ("failure", crate::failure_patterns::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R702.4: the served correction refusals.
        ("correction", crate::correction::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R708.4: the knowledge flag refusals.
        ("knowledge_flag", crate::knowledge_flag::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R710.4: the friction report refusals.
        ("friction", crate::friction::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R711.4: the skill evaluation refusals.
        ("skill_eval", crate::skill_eval::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R714.5, R715.1: the skill tier and always-on refusals.
        ("promotion_tier", super::promotion_tier::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R713.4: the skill telemetry refusals.
        ("skill_telemetry", crate::skill_telemetry::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R709.2, R709.4: the knowledge curation refusals.
        (
            "knowledge_curation",
            crate::knowledge_curation::REFUSALS.to_vec(),
        ),
        // MEDULLA-DECISION-ENGINE-R810.4: the program optimization job refusals.
        ("program", super::program::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R142.100: the local-model certification refusals.
        (
            "local_certification",
            super::local_certification::REFUSALS.to_vec(),
        ),
        (
            "delegation_quality",
            super::delegation_quality::REFUSALS.to_vec(),
        ),
        (
            "distilled",
            super::statistical::distilled::REFUSALS.to_vec(),
        ),
        (
            "retrieval_drift",
            super::statistical::retrieval_drift::REFUSALS.to_vec(),
        ),
        ("asset_usage", asset_usage::REFUSALS.to_vec()),
        ("archimate_exchange", archimate_exchange::REFUSALS.to_vec()),
        // MEDULLA-CQ-ORCH-R702.12: the served process document refusals.
        (
            "process_exchange",
            medulla_types::process_exchange::REFUSALS.to_vec(),
        ),
        // MEDULLA-ORCH-VOCAB-R009.101.1: the harness session index refusals.
        (
            "harness_session",
            medulla_types::harness_session::REFUSALS.to_vec(),
        ),
        // MEDULLA-ORCH-VOCAB-R012.8: the policy activation refusals.
        (
            "policy_document",
            medulla_types::policy_document::REFUSALS.to_vec(),
        ),
        // MEDULLA-ORCH-VOCAB-R009.100: the served run-trace export refusals.
        (
            "run_trace_export",
            medulla_types::run_trace_export::REFUSALS.to_vec(),
        ),
        // MEDULLA-CQ-ORCH-R700.4: the served process compile and the import codes it keeps.
        (
            "process_compile",
            medulla_types::process_import::COMPILE_REFUSALS
                .iter()
                .chain(medulla_types::process_import::REFUSALS)
                .map(|code| RefusalCode::new(code))
                .collect(),
        ),
        ("belief_revision", codes(belief_revision::REFUSALS)),
        ("claims", codes(claims::REFUSALS)),
        ("context_facts", context_facts::REFUSALS.to_vec()),
        ("meeting_transcript", codes(meeting_transcript::REFUSALS)),
        ("participant", participant::REFUSALS.to_vec()),
        // MEDULLA-MEDIA-VOCAB-R001.100.2: the media tag ingest refusals.
        ("media_tag", media_tag::REFUSALS.to_vec()),
        // MEDULLA-MEDIA-VOCAB-R001.102.4: the bookmark search and relations refusals.
        ("bookmark", bookmark_query::REFUSALS.to_vec()),
        // MEDULLA-MEDIA-VOCAB-R002.5.5: the paper ingest refusals.
        ("paper", paper_ingest::REFUSALS.to_vec()),
        // MEDULLA-MEDIA-VOCAB-R650.4: the media attachment and derivation refusals.
        ("memory", medulla_work_types::memory::REFUSALS.to_vec()),
        // MEDULLA-DECISION-ENGINE-R173.7: the harness edit lifecycle.
        (
            "harness_edit",
            medulla_work_types::harness_edit::REFUSALS.to_vec(),
        ),
        // MEDULLA-DECISION-ENGINE-R160.4: skill package publication.
        (
            "package_publication",
            super::package_validation::REFUSALS.to_vec(),
        ),
        ("source_toc", source_toc::REFUSALS.to_vec()),
        ("transition_refused", transition_refused::REFUSALS.to_vec()),
        (
            "lifecycle",
            medulla_source_types::lifecycle::REFUSALS.to_vec(),
        ),
        // MEDULLA-SDLC-R002.7: the deploy record refusals.
        (
            "deploy_record",
            medulla_source_types::deploy_record::REFUSALS.to_vec(),
        ),
        // MEDULLA-DECISION-ENGINE-R901: the agent goal refusals.
        (
            "agent_goal",
            medulla_source_types::agent_goal::REFUSALS.to_vec(),
        ),
        // MEDULLA-UNIFIED-DATA-PLANE-R790: the connector run refusals.
        (
            "connector_run",
            medulla_source_types::connector_run::REFUSALS.to_vec(),
        ),
        // MEDULLA-UNIFIED-DATA-PLANE-R791: the source connection and mapping refusals.
        (
            "source_connection",
            medulla_source_types::source_connection::REFUSALS.to_vec(),
        ),
        // MEDULLA-BUSINESS-OPS-R004: the approval gate question's refusal.
        (
            "approval_gate",
            medulla_source_types::approval_gate::REFUSALS.to_vec(),
        ),
        // MEDULLA-ORCH-VOCAB-R810.2: the agent template instantiate refusals.
        (
            "agent_template",
            medulla_agent_types::agent_template::REFUSALS.to_vec(),
        ),
        (
            "topology_enforcement",
            medulla_agent_types::agent_topology::enforcement::REFUSALS.to_vec(),
        ),
        (
            "search_task",
            medulla_work_types::search_task::REFUSALS.to_vec(),
        ),
        ("topology", medulla_query_types::topology::REFUSALS.to_vec()),
        // The standalone topology question's read refusal (R111.1).
        (
            "topology_question",
            vec![RefusalCode::new(
                super::statistical::topology::TOPOLOGY_SCHEMA_UNAVAILABLE,
            )],
        ),
    ]
}

/// Every registered code: the shared codes first, then each family and module
/// table. A code may appear in more than one table.
pub fn registered() -> impl Iterator<Item = RefusalCode> {
    SHARED
        .iter()
        .chain(FOLDED.iter())
        .chain(PROMOTION.iter())
        .chain(FAMILY_TABLES.iter().flat_map(|(_, table)| table.iter()))
        .chain(super::usage::REFUSALS.iter())
        .chain(super::transfer::REFUSALS.iter())
        .chain(super::evidence_stream::REFUSALS.iter())
        .chain(super::execution_part::REFUSALS.iter())
        .chain(super::importance_penalty::REFUSALS.iter())
        .chain(medulla_agent_types::agent_component::red_team::REFUSALS.iter())
        .chain(super::memory_rewrite::REFUSALS.iter())
        .chain(super::statistical::retrieval_generation::REFUSALS.iter())
        .copied()
        .chain(module_tables().into_iter().flat_map(|(_, table)| table))
}

/// Resolve a wire string to its registered code, shared codes first. The
/// assembly and commit surface's closed `DecisionErrorCode` set is registered
/// too: an assembly refusal folds under `RefusalOrigin::ASSEMBLE` with its own
/// code (MEDULLA-DECISION-ENGINE-R010.100.2).
pub fn resolve(code: &str) -> Option<RefusalCode> {
    registered()
        .find(|registered| registered.as_str() == code)
        .or_else(|| decision_error_code(code))
}

/// The registered code of one `DecisionErrorCode` token.
fn decision_error_code(code: &str) -> Option<RefusalCode> {
    super::DecisionErrorCode::ALL
        .iter()
        .find(|registered| registered.as_str() == code)
        .map(|registered| RefusalCode::new(registered.as_str()))
}

#[cfg(test)]
mod thought_search_registration_tests {
    use super::resolve;
    use crate::decision::statistical::StatisticalErrorCode;
    use crate::decision::thought_search::REFUSALS;

    // spec: MEDULLA-DECISION-ENGINE-R750.752
    #[test]
    fn declared_refusals_are_registered_statistical_codes() {
        for code in REFUSALS {
            assert_eq!(resolve(code.as_str()), Some(*code));
            assert!(StatisticalErrorCode::ALL
                .iter()
                .any(|known| known.as_str() == code.as_str()));
        }
    }
}

#[cfg(test)]
mod task_plan_registration_tests {
    // spec: MEDULLA-DECISION-ENGINE-R134.6
    #[test]
    fn every_planner_code_is_registered_upper_snake() {
        use super::resolve;
        for code in crate::decision::task_plan::TASK_PLAN_REFUSALS {
            assert_eq!(resolve(code.as_str()), Some(*code));
            assert!(code
                .as_str()
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b == b'_'));
        }
        for raised in [
            "EMPTY_PLAN",
            "PLAN_CYCLE",
            "DISCONNECTED_OUTPUT",
            "BINDING_NOT_DEPENDENCY",
            "PLAN_PARAMETER_CONFLICT",
        ] {
            assert!(resolve(raised).is_some(), "{raised} is registered");
        }
    }
}
