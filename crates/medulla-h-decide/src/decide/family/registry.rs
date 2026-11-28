//! The question-family registry: the one exhaustive `match` from a `QuestionKind` to its family
//! (MEDULLA-DECISION-ENGINE-R001.101). It names every family, so it sits above all of them.

use super::edit_concentration;
use super::team_coordination;
use medulla_decision_types::decision::statistical::QuestionKind;
use medulla_h_decide_base::decide::family::classify;
use medulla_h_decide_base::decide::family::comparative_analysis;
use medulla_h_decide_base::decide::family::connector_event_triage;
use medulla_h_decide_base::decide::family::connector_tool_choice;
use medulla_h_decide_base::decide::family::connector_write_back;
use medulla_h_decide_base::decide::family::context_budget;
use medulla_h_decide_base::decide::family::enrichment_schedule;
use medulla_h_decide_base::decide::family::feature_match;
use medulla_h_decide_base::decide::family::harness_contrast;
use medulla_h_decide_base::decide::family::ingestion_lane;
use medulla_h_decide_base::decide::family::placement;
use medulla_h_decide_base::decide::family::placement_plan;
use medulla_h_decide_base::decide::family::planning_admission;
use medulla_h_decide_base::decide::family::pre_tool_risk;
use medulla_h_decide_base::decide::family::research_voi;
use medulla_h_decide_base::decide::family::retrieval_plan;
use medulla_h_decide_base::decide::family::schema_mapping;
use medulla_h_decide_base::decide::family::skill_upgrade;
use medulla_h_decide_base::decide::family::template_choice;
use medulla_h_decide_base::decide::family::thought_search;
use medulla_h_decide_base::decide::family::tool_subset;
use medulla_h_decide_base::decide::family::DecisionFamily;
use medulla_h_decide_learning::decide::family::capability_designation;
use medulla_h_decide_learning::decide::family::command_config;
use medulla_h_decide_learning::decide::family::context_package;
use medulla_h_decide_learning::decide::family::defect_triage;
use medulla_h_decide_learning::decide::family::evolution_round;
use medulla_h_decide_learning::decide::family::harness_config;
use medulla_h_decide_learning::decide::family::instruction_disclosure;
use medulla_h_decide_learning::decide::family::load_set;
use medulla_h_decide_learning::decide::family::merge_arming;
use medulla_h_decide_learning::decide::family::paper_admission;
use medulla_h_decide_learning::decide::family::probe_selection;
use medulla_h_decide_learning::decide::family::process_decision;
use medulla_h_decide_learning::decide::family::process_plan;
use medulla_h_decide_learning::decide::family::program_store;
use medulla_h_decide_learning::decide::family::promotion;
use medulla_h_decide_learning::decide::family::route;
use medulla_h_decide_learning::decide::family::run_adjustment;
use medulla_h_decide_learning::decide::family::skill_evolution;
use medulla_h_decide_learning::decide::family::tool_necessity;
use medulla_h_decide_learning::decide::family::tool_relevance;
use medulla_h_decide_runlog::decide::family::a2a_route;
use medulla_h_decide_runlog::decide::family::adversarial_admission;
use medulla_h_decide_runlog::decide::family::analyst_synthesis;
use medulla_h_decide_runlog::decide::family::answer_ranking;
use medulla_h_decide_runlog::decide::family::autonomy;
use medulla_h_decide_runlog::decide::family::branch_outcome;
use medulla_h_decide_runlog::decide::family::capacity_proposal;
use medulla_h_decide_runlog::decide::family::changeset_review;
use medulla_h_decide_runlog::decide::family::context_replacement;
use medulla_h_decide_runlog::decide::family::correction_admission;
use medulla_h_decide_runlog::decide::family::council;
use medulla_h_decide_runlog::decide::family::credential_reuse;
use medulla_h_decide_runlog::decide::family::curriculum_promotion;
use medulla_h_decide_runlog::decide::family::delegation_admission;
use medulla_h_decide_runlog::decide::family::fanout_aggregate;
use medulla_h_decide_runlog::decide::family::fanout_width;
use medulla_h_decide_runlog::decide::family::feedback_loop;
use medulla_h_decide_runlog::decide::family::field_shielding;
use medulla_h_decide_runlog::decide::family::generated_code_admission;
use medulla_h_decide_runlog::decide::family::graph_intake;
use medulla_h_decide_runlog::decide::family::held_item_admission;
use medulla_h_decide_runlog::decide::family::ingest_shortfall;
use medulla_h_decide_runlog::decide::family::insight_selection;
use medulla_h_decide_runlog::decide::family::item_recommendation;
use medulla_h_decide_runlog::decide::family::over_window_plan;
use medulla_h_decide_runlog::decide::family::path_preference;
use medulla_h_decide_runlog::decide::family::playbook_triage;
use medulla_h_decide_runlog::decide::family::population_drift;
use medulla_h_decide_runlog::decide::family::probe_verdict;
use medulla_h_decide_runlog::decide::family::process_remediation;
use medulla_h_decide_runlog::decide::family::rank;
use medulla_h_decide_runlog::decide::family::reasoning_paradigm;
use medulla_h_decide_runlog::decide::family::recruit;
use medulla_h_decide_runlog::decide::family::recursion_mode;
use medulla_h_decide_runlog::decide::family::resolve_conflict;
use medulla_h_decide_runlog::decide::family::resolve_entity;
use medulla_h_decide_runlog::decide::family::revert_or_fix_forward;
use medulla_h_decide_runlog::decide::family::reward_decomposition;
use medulla_h_decide_runlog::decide::family::runtime_quota;
use medulla_h_decide_runlog::decide::family::scale_intent;
use medulla_h_decide_runlog::decide::family::serving_technique;
use medulla_h_decide_runlog::decide::family::skill_workflow;
use medulla_h_decide_runlog::decide::family::step_verification;
use medulla_h_decide_runlog::decide::family::sub_call_admission;
use medulla_h_decide_runlog::decide::family::task_plan;
use medulla_h_decide_runlog::decide::family::temporal_basis;
use medulla_h_decide_runlog::decide::family::tool_action_admission;
use medulla_h_decide_runlog::decide::family::topology;
use medulla_h_decide_runlog::decide::family::trajectory_join;
use medulla_h_decide_runlog::decide::family::trajectory_ladder;
use medulla_h_decide_runlog::decide::family::usage_guidance;

/// The family a `QuestionKind` decides with. The match is exhaustive over
/// `QuestionKind`: a new variant without a family fails to compile
/// (MEDULLA-DECISION-ENGINE-R001.101).
pub fn for_kind(kind: QuestionKind) -> &'static dyn DecisionFamily {
    static ROUTEFAMILY: route::RouteFamily = route::RouteFamily;
    static RANKFAMILY: rank::RankFamily = rank::RankFamily;
    static CLASSIFYFAMILY: classify::ClassifyFamily = classify::ClassifyFamily;
    static RESOLVEENTITYFAMILY: resolve_entity::ResolveEntityFamily =
        resolve_entity::ResolveEntityFamily;
    static SCHEMAMAPPINGFAMILY: schema_mapping::SchemaMappingFamily =
        schema_mapping::SchemaMappingFamily;
    static TEMPLATECHOICEFAMILY: template_choice::TemplateChoiceFamily =
        template_choice::TemplateChoiceFamily;
    static RETRIEVALPLANFAMILY: retrieval_plan::RetrievalPlanFamily =
        retrieval_plan::RetrievalPlanFamily;
    static INGESTIONLANEFAMILY: ingestion_lane::IngestionLaneFamily =
        ingestion_lane::IngestionLaneFamily;
    static REASONINGPARADIGMFAMILY: reasoning_paradigm::ReasoningParadigmFamily =
        reasoning_paradigm::ReasoningParadigmFamily;
    static MODELCAPACITYFAMILY:
        medulla_h_decide_base::decide::stat_model_capacity::ModelCapacityFamily =
        medulla_h_decide_base::decide::stat_model_capacity::ModelCapacityFamily;
    static ENRICHMENTSCHEDULEFAMILY: enrichment_schedule::EnrichmentScheduleFamily =
        enrichment_schedule::EnrichmentScheduleFamily;
    static PRETOOLRISKFAMILY: pre_tool_risk::PreToolRiskFamily = pre_tool_risk::PreToolRiskFamily;
    static CONNECTOREVENTTRIAGEFAMILY: connector_event_triage::ConnectorEventTriageFamily =
        connector_event_triage::ConnectorEventTriageFamily;
    static CONNECTORTOOLCHOICEFAMILY: connector_tool_choice::ConnectorToolChoiceFamily =
        connector_tool_choice::ConnectorToolChoiceFamily;
    static CONNECTORWRITEBACKFAMILY: connector_write_back::ConnectorWriteBackFamily =
        connector_write_back::ConnectorWriteBackFamily;
    static TOOLSUBSETFAMILY: tool_subset::ToolSubsetFamily = tool_subset::ToolSubsetFamily;
    static HARNESSCONFIGFAMILY: harness_config::HarnessConfigFamily =
        harness_config::HarnessConfigFamily;
    static TASKPLANFAMILY: task_plan::TaskPlanFamily = task_plan::TaskPlanFamily;
    static HARNESSCONTRASTFAMILY: harness_contrast::HarnessContrastFamily =
        harness_contrast::HarnessContrastFamily;
    static PLACEMENTFAMILY: placement::PlacementFamily = placement::PlacementFamily;
    static PLACEMENTPLANFAMILY: placement_plan::PlacementPlanFamily =
        placement_plan::PlacementPlanFamily;
    static CONTEXTBUDGETFAMILY: context_budget::ContextBudgetFamily =
        context_budget::ContextBudgetFamily;
    static OVERWINDOWPLANFAMILY: over_window_plan::OverWindowPlanFamily =
        over_window_plan::OverWindowPlanFamily;
    static INSTRUCTIONDISCLOSUREFAMILY: instruction_disclosure::InstructionDisclosureFamily =
        instruction_disclosure::InstructionDisclosureFamily;
    static ROUNDVERDICTFAMILY: evolution_round::RoundVerdictFamily =
        evolution_round::RoundVerdictFamily;
    static STAGEALLOCATIONFAMILY: evolution_round::StageAllocationFamily =
        evolution_round::StageAllocationFamily;
    static PROBESELECTIONFAMILY: probe_selection::ProbeSelectionFamily =
        probe_selection::ProbeSelectionFamily;
    static PROBEVERDICTFAMILY: probe_verdict::ProbeVerdictFamily =
        probe_verdict::ProbeVerdictFamily;
    static ITEMRECOMMENDATIONFAMILY: item_recommendation::ItemRecommendationFamily =
        item_recommendation::ItemRecommendationFamily;
    static LOADSETFAMILY: load_set::LoadSetFamily = load_set::LoadSetFamily;
    static USAGEGUIDANCEFAMILY: usage_guidance::UsageGuidanceFamily =
        usage_guidance::UsageGuidanceFamily;
    static BRANCHOUTCOMEFAMILY: branch_outcome::BranchOutcomeFamily =
        branch_outcome::BranchOutcomeFamily;
    static DEFECTTRIAGEFAMILY: defect_triage::DefectTriageFamily =
        defect_triage::DefectTriageFamily;
    static MERGEARMINGFAMILY: merge_arming::MergeArmingFamily = merge_arming::MergeArmingFamily;
    static REVERTORFIXFORWARDFAMILY: revert_or_fix_forward::RevertOrFixForwardFamily =
        revert_or_fix_forward::RevertOrFixForwardFamily;
    static EDITCONCENTRATIONFAMILY: edit_concentration::EditConcentrationFamily =
        edit_concentration::EditConcentrationFamily;
    static THOUGHTSEARCHFAMILY: thought_search::ThoughtSearchFamily =
        thought_search::ThoughtSearchFamily;
    static COMPARATIVEANALYSISPLANFAMILY: comparative_analysis::ComparativeAnalysisPlanFamily =
        comparative_analysis::ComparativeAnalysisPlanFamily;
    static CONTEXTPACKAGEFAMILY: context_package::ContextPackageFamily =
        context_package::ContextPackageFamily;
    static TOPOLOGYFAMILY: topology::TopologyFamily = topology::TopologyFamily;
    static SKILLWORKFLOWFAMILY: skill_workflow::SkillWorkflowFamily =
        skill_workflow::SkillWorkflowFamily;
    static SKILLUPGRADEFAMILY: skill_upgrade::SkillUpgradeFamily =
        skill_upgrade::SkillUpgradeFamily;
    static TEAMCOORDINATIONFAMILY: team_coordination::TeamCoordinationFamily =
        team_coordination::TeamCoordinationFamily;
    static CAPABILITYDESIGNATIONFAMILY: capability_designation::CapabilityDesignationFamily =
        capability_designation::CapabilityDesignationFamily;
    static PROGRAMLOOKUPFAMILY: program_store::ProgramLookupFamily =
        program_store::ProgramLookupFamily;
    static RECURSIONMODEFAMILY: recursion_mode::RecursionModeFamily =
        recursion_mode::RecursionModeFamily;
    static SUBCALLADMISSIONFAMILY: sub_call_admission::SubCallAdmissionFamily =
        sub_call_admission::SubCallAdmissionFamily;
    static CONTEXTREPLACEMENTFAMILY: context_replacement::ContextReplacementFamily =
        context_replacement::ContextReplacementFamily;
    static COMMANDCONFIGFAMILY: command_config::CommandConfigFamily =
        command_config::CommandConfigFamily;
    static RUNADJUSTMENTFAMILY: run_adjustment::RunAdjustmentFamily =
        run_adjustment::RunAdjustmentFamily;
    static PATHPREFERENCEFAMILY: path_preference::PathPreferenceFamily =
        path_preference::PathPreferenceFamily;
    static CORRECTIONADMISSIONFAMILY: correction_admission::CorrectionAdmissionFamily =
        correction_admission::CorrectionAdmissionFamily;
    static FIELDSHIELDINGFAMILY: field_shielding::FieldShieldingFamily =
        field_shielding::FieldShieldingFamily;
    static GRAPHINTAKEFAMILY: graph_intake::GraphIntakeFamily = graph_intake::GraphIntakeFamily;
    static PROMOTIONFAMILY: promotion::PromotionFamily = promotion::PromotionFamily;
    static PROCESSDECISIONFAMILY: process_decision::ProcessDecisionFamily =
        process_decision::ProcessDecisionFamily;
    static TOOLNECESSITYFAMILY: tool_necessity::ToolNecessityFamily =
        tool_necessity::ToolNecessityFamily;
    static FEEDBACKLOOPFAMILY: feedback_loop::FeedbackLoopFamily =
        feedback_loop::FeedbackLoopFamily;
    static RUNTIMEQUOTAFAMILY: runtime_quota::RuntimeQuotaFamily =
        runtime_quota::RuntimeQuotaFamily;
    static PLANNINGADMISSIONFAMILY: planning_admission::PlanningAdmissionFamily =
        planning_admission::PlanningAdmissionFamily;
    static TOOLACTIONADMISSIONFAMILY: tool_action_admission::ToolActionAdmissionFamily =
        tool_action_admission::ToolActionAdmissionFamily;
    static A2AROUTEFAMILY: a2a_route::A2aRouteFamily = a2a_route::A2aRouteFamily;
    static PAPERADMISSIONFAMILY: paper_admission::PaperAdmissionFamily =
        paper_admission::PaperAdmissionFamily;
    static HELDITEMADMISSIONFAMILY: held_item_admission::HeldItemAdmissionFamily =
        held_item_admission::HeldItemAdmissionFamily;
    static CHANGESETREVIEWFAMILY: changeset_review::ChangesetReviewFamily =
        changeset_review::ChangesetReviewFamily;
    static TEMPORALBASISFAMILY: temporal_basis::TemporalBasisFamily =
        temporal_basis::TemporalBasisFamily;
    static CREDENTIALREUSEFAMILY: credential_reuse::CredentialReuseFamily =
        credential_reuse::CredentialReuseFamily;
    static GENERATEDCODEADMISSIONFAMILY: generated_code_admission::GeneratedCodeAdmissionFamily =
        generated_code_admission::GeneratedCodeAdmissionFamily;
    static CRITERIONREVIEWFAMILY: evolution_round::CriterionReviewFamily =
        evolution_round::CriterionReviewFamily;
    static TOOLRELEVANCEFAMILY: tool_relevance::ToolRelevanceFamily =
        tool_relevance::ToolRelevanceFamily;
    static INSIGHTSELECTIONFAMILY: insight_selection::InsightSelectionFamily =
        insight_selection::InsightSelectionFamily;
    static FANOUTAGGREGATEFAMILY: fanout_aggregate::FanoutAggregateFamily =
        fanout_aggregate::FanoutAggregateFamily;
    static DELEGATIONADMISSIONFAMILY: delegation_admission::DelegationAdmissionFamily =
        delegation_admission::DelegationAdmissionFamily;
    static ANSWERRANKINGFAMILY: answer_ranking::AnswerRankingFamily =
        answer_ranking::AnswerRankingFamily;
    static SERVINGTECHNIQUEFAMILY: serving_technique::ServingTechniqueFamily =
        serving_technique::ServingTechniqueFamily;
    static PROCESSPLANFAMILY: process_plan::ProcessPlanFamily = process_plan::ProcessPlanFamily;
    static PROCESSREMEDIATIONFAMILY: process_remediation::ProcessRemediationFamily =
        process_remediation::ProcessRemediationFamily;
    static INGESTSHORTFALLFAMILY: ingest_shortfall::IngestShortfallFamily =
        ingest_shortfall::IngestShortfallFamily;
    static CAPACITYPROPOSALFAMILY: capacity_proposal::CapacityProposalFamily =
        capacity_proposal::CapacityProposalFamily;
    static SCALEINTENTFAMILY: scale_intent::ScaleIntentFamily = scale_intent::ScaleIntentFamily;
    static REWARDDECOMPOSITIONFAMILY: reward_decomposition::RewardDecompositionFamily =
        reward_decomposition::RewardDecompositionFamily;
    static CURRICULUMPROMOTIONFAMILY: curriculum_promotion::CurriculumPromotionFamily =
        curriculum_promotion::CurriculumPromotionFamily;
    static POPULATIONDRIFTFAMILY: population_drift::PopulationDriftFamily =
        population_drift::PopulationDriftFamily;
    static RESEARCHVOIFAMILY: research_voi::ResearchVoiFamily = research_voi::ResearchVoiFamily;
    static FEATUREMATCHFAMILY: feature_match::FeatureMatchFamily =
        feature_match::FeatureMatchFamily;
    static AUTONOMYFAMILY: autonomy::AutonomyFamily = autonomy::AutonomyFamily;
    static FANOUTWIDTHFAMILY: fanout_width::FanoutWidthFamily = fanout_width::FanoutWidthFamily;
    static TRAJECTORYJOINFAMILY: trajectory_join::TrajectoryJoinFamily =
        trajectory_join::TrajectoryJoinFamily;
    static STEPVERIFICATIONFAMILY: step_verification::StepVerificationFamily =
        step_verification::StepVerificationFamily;
    static ADVERSARIALADMISSIONFAMILY: adversarial_admission::AdversarialAdmissionFamily =
        adversarial_admission::AdversarialAdmissionFamily;
    static PLAYBOOKTRIAGEFAMILY: playbook_triage::PlaybookTriageFamily =
        playbook_triage::PlaybookTriageFamily;
    static SKILLEVOLUTIONFAMILY: skill_evolution::SkillEvolutionFamily =
        skill_evolution::SkillEvolutionFamily;
    static TRAJECTORYLADDERFAMILY: trajectory_ladder::TrajectoryLadderFamily =
        trajectory_ladder::TrajectoryLadderFamily;
    static ANALYSTSYNTHESISFAMILY: analyst_synthesis::AnalystSynthesisFamily =
        analyst_synthesis::AnalystSynthesisFamily;
    static RECRUITFAMILY: recruit::RecruitFamily = recruit::RecruitFamily;
    static RESOLVECONFLICTFAMILY: resolve_conflict::ResolveConflictFamily =
        resolve_conflict::ResolveConflictFamily;
    static COUNCILFAMILY: council::CouncilFamily = council::CouncilFamily;
    match kind {
        QuestionKind::Route => &ROUTEFAMILY,
        QuestionKind::Rank => &RANKFAMILY,
        QuestionKind::Classify => &CLASSIFYFAMILY,
        QuestionKind::ResolveEntity => &RESOLVEENTITYFAMILY,
        QuestionKind::SchemaMapping => &SCHEMAMAPPINGFAMILY,
        QuestionKind::TemplateChoice => &TEMPLATECHOICEFAMILY,
        QuestionKind::RetrievalPlan => &RETRIEVALPLANFAMILY,
        QuestionKind::IngestionLane => &INGESTIONLANEFAMILY,
        QuestionKind::EnrichmentSchedule => &ENRICHMENTSCHEDULEFAMILY,
        QuestionKind::PreToolRisk => &PRETOOLRISKFAMILY,
        QuestionKind::ConnectorEventTriage => &CONNECTOREVENTTRIAGEFAMILY,
        QuestionKind::ConnectorToolChoice => &CONNECTORTOOLCHOICEFAMILY,
        QuestionKind::ConnectorWriteBack => &CONNECTORWRITEBACKFAMILY,
        QuestionKind::ToolSubset => &TOOLSUBSETFAMILY,
        QuestionKind::HarnessConfig => &HARNESSCONFIGFAMILY,
        QuestionKind::TaskPlan => &TASKPLANFAMILY,
        QuestionKind::HarnessContrast => &HARNESSCONTRASTFAMILY,
        QuestionKind::Placement => &PLACEMENTFAMILY,
        QuestionKind::PlacementPlan => &PLACEMENTPLANFAMILY,
        QuestionKind::ContextBudget => &CONTEXTBUDGETFAMILY,
        QuestionKind::OverWindowPlan => &OVERWINDOWPLANFAMILY,
        QuestionKind::InstructionDisclosure => &INSTRUCTIONDISCLOSUREFAMILY,
        QuestionKind::ChangesetReview => &CHANGESETREVIEWFAMILY,
        QuestionKind::TemporalBasis => &TEMPORALBASISFAMILY,
        QuestionKind::RoundVerdict => &ROUNDVERDICTFAMILY,
        QuestionKind::StageAllocation => &STAGEALLOCATIONFAMILY,
        QuestionKind::ProbeVerdict => &PROBEVERDICTFAMILY,
        QuestionKind::ItemRecommendation => &ITEMRECOMMENDATIONFAMILY,
        QuestionKind::LoadSet => &LOADSETFAMILY,
        QuestionKind::UsageGuidance => &USAGEGUIDANCEFAMILY,
        QuestionKind::BranchOutcome => &BRANCHOUTCOMEFAMILY,
        QuestionKind::EditConcentration => &EDITCONCENTRATIONFAMILY,
        QuestionKind::ThoughtSearch => &THOUGHTSEARCHFAMILY,
        QuestionKind::ComparativeAnalysisPlan => &COMPARATIVEANALYSISPLANFAMILY,
        QuestionKind::ContextPackage => &CONTEXTPACKAGEFAMILY,
        QuestionKind::Topology => &TOPOLOGYFAMILY,
        QuestionKind::ProgramLookup => &PROGRAMLOOKUPFAMILY,
        QuestionKind::SkillWorkflowCompile => &SKILLWORKFLOWFAMILY,
        QuestionKind::SkillUpgrade => &SKILLUPGRADEFAMILY,
        QuestionKind::TeamCoordination => &TEAMCOORDINATIONFAMILY,
        QuestionKind::CapabilityDesignation => &CAPABILITYDESIGNATIONFAMILY,
        QuestionKind::ContextReplacement => &CONTEXTREPLACEMENTFAMILY,
        QuestionKind::RecursionMode => &RECURSIONMODEFAMILY,
        QuestionKind::SubCallAdmission => &SUBCALLADMISSIONFAMILY,
        QuestionKind::CommandConfig => &COMMANDCONFIGFAMILY,
        QuestionKind::RunAdjustment => &RUNADJUSTMENTFAMILY,
        QuestionKind::PathPreference => &PATHPREFERENCEFAMILY,
        QuestionKind::CorrectionAdmission => &CORRECTIONADMISSIONFAMILY,
        QuestionKind::FieldShielding => &FIELDSHIELDINGFAMILY,
        QuestionKind::GraphIntake => &GRAPHINTAKEFAMILY,
        QuestionKind::ToolNecessity => &TOOLNECESSITYFAMILY,
        QuestionKind::DecisionTableEval | QuestionKind::BranchSelect => &PROCESSDECISIONFAMILY,
        QuestionKind::FeedbackLoop => &FEEDBACKLOOPFAMILY,
        QuestionKind::PlanningAdmission => &PLANNINGADMISSIONFAMILY,
        QuestionKind::ToolActionAdmission => &TOOLACTIONADMISSIONFAMILY,
        QuestionKind::A2aRoute => &A2AROUTEFAMILY,
        QuestionKind::PaperAdmission => &PAPERADMISSIONFAMILY,
        QuestionKind::HeldItemAdmission => &HELDITEMADMISSIONFAMILY,
        QuestionKind::CriterionReview => &CRITERIONREVIEWFAMILY,
        QuestionKind::GeneratedCodeAdmission => &GENERATEDCODEADMISSIONFAMILY,
        QuestionKind::CredentialReuse => &CREDENTIALREUSEFAMILY,
        QuestionKind::RuntimeQuota => &RUNTIMEQUOTAFAMILY,
        QuestionKind::InsightSelection => &INSIGHTSELECTIONFAMILY,
        QuestionKind::ToolRelevance => &TOOLRELEVANCEFAMILY,
        QuestionKind::FanoutAggregate => &FANOUTAGGREGATEFAMILY,
        QuestionKind::DelegationAdmission => &DELEGATIONADMISSIONFAMILY,
        QuestionKind::AnswerRanking => &ANSWERRANKINGFAMILY,
        QuestionKind::ServingTechniqueActivation => &SERVINGTECHNIQUEFAMILY,
        QuestionKind::ReasoningParadigm => &REASONINGPARADIGMFAMILY,
        QuestionKind::ModelCapacity => &MODELCAPACITYFAMILY,
        QuestionKind::ProcessRemediation => &PROCESSREMEDIATIONFAMILY,
        QuestionKind::ProcessPlan => &PROCESSPLANFAMILY,
        QuestionKind::IngestShortfall => &INGESTSHORTFALLFAMILY,
        QuestionKind::CapacityProposal => &CAPACITYPROPOSALFAMILY,
        QuestionKind::ScaleIntent => &SCALEINTENTFAMILY,
        QuestionKind::ProbeSelection => &PROBESELECTIONFAMILY,
        QuestionKind::RewardDecomposition => &REWARDDECOMPOSITIONFAMILY,
        QuestionKind::CurriculumPromotion => &CURRICULUMPROMOTIONFAMILY,
        QuestionKind::ResearchNext => &RESEARCHVOIFAMILY,
        QuestionKind::FeatureMatch => &FEATUREMATCHFAMILY,
        QuestionKind::PopulationDrift => &POPULATIONDRIFTFAMILY,
        QuestionKind::FanoutWidth => &FANOUTWIDTHFAMILY,
        QuestionKind::AutonomyVerdict => &AUTONOMYFAMILY,
        QuestionKind::TrajectoryJoin => &TRAJECTORYJOINFAMILY,
        QuestionKind::StepVerification => &STEPVERIFICATIONFAMILY,
        QuestionKind::AdversarialAdmission => &ADVERSARIALADMISSIONFAMILY,
        QuestionKind::SkillEvolution => &SKILLEVOLUTIONFAMILY,
        QuestionKind::PlaybookTriage => &PLAYBOOKTRIAGEFAMILY,
        QuestionKind::TrajectoryLadder => &TRAJECTORYLADDERFAMILY,
        QuestionKind::AnalystSynthesis => &ANALYSTSYNTHESISFAMILY,
        QuestionKind::Recruit => &RECRUITFAMILY,
        QuestionKind::ResolveConflict => &RESOLVECONFLICTFAMILY,
        QuestionKind::CouncilCoordination => &COUNCILFAMILY,
        QuestionKind::MergeArming => &MERGEARMINGFAMILY,
        QuestionKind::RevertOrFixForward => &REVERTORFIXFORWARDFAMILY,
        QuestionKind::DefectTriage => &DEFECTTRIAGEFAMILY,
        QuestionKind::Promotion => &PROMOTIONFAMILY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One entry per `QuestionKind` variant; the match is exhaustive, so a
    /// new variant without a family fails to compile.
    fn all_kinds() -> Vec<QuestionKind> {
        let kinds = vec![
            QuestionKind::Route,
            QuestionKind::Rank,
            QuestionKind::Classify,
            QuestionKind::ResolveEntity,
            QuestionKind::SchemaMapping,
            QuestionKind::TemplateChoice,
            QuestionKind::RetrievalPlan,
            QuestionKind::IngestionLane,
            QuestionKind::EnrichmentSchedule,
            QuestionKind::PreToolRisk,
            QuestionKind::ConnectorEventTriage,
            QuestionKind::ConnectorToolChoice,
            QuestionKind::ConnectorWriteBack,
            QuestionKind::ToolSubset,
            QuestionKind::HarnessConfig,
            QuestionKind::TaskPlan,
            QuestionKind::HarnessContrast,
            QuestionKind::Placement,
            QuestionKind::PlacementPlan,
            QuestionKind::ContextBudget,
            QuestionKind::OverWindowPlan,
            QuestionKind::InstructionDisclosure,
            QuestionKind::ChangesetReview,
            QuestionKind::TemporalBasis,
            QuestionKind::RoundVerdict,
            QuestionKind::StageAllocation,
            QuestionKind::ProbeVerdict,
            QuestionKind::ItemRecommendation,
            QuestionKind::LoadSet,
            QuestionKind::UsageGuidance,
            QuestionKind::BranchOutcome,
            QuestionKind::EditConcentration,
            QuestionKind::ThoughtSearch,
            QuestionKind::ComparativeAnalysisPlan,
            QuestionKind::ContextPackage,
            QuestionKind::Topology,
            QuestionKind::ProgramLookup,
            QuestionKind::SkillWorkflowCompile,
            QuestionKind::SkillUpgrade,
            QuestionKind::TeamCoordination,
            QuestionKind::CapabilityDesignation,
            QuestionKind::ContextReplacement,
            QuestionKind::RecursionMode,
            QuestionKind::SubCallAdmission,
            QuestionKind::CommandConfig,
            QuestionKind::RunAdjustment,
            QuestionKind::PathPreference,
            QuestionKind::CorrectionAdmission,
            QuestionKind::FieldShielding,
            QuestionKind::GraphIntake,
            QuestionKind::ToolNecessity,
            QuestionKind::DecisionTableEval,
            QuestionKind::BranchSelect,
            QuestionKind::FeedbackLoop,
            QuestionKind::PlanningAdmission,
            QuestionKind::ToolActionAdmission,
            QuestionKind::A2aRoute,
            QuestionKind::PaperAdmission,
            QuestionKind::HeldItemAdmission,
            QuestionKind::CriterionReview,
            QuestionKind::GeneratedCodeAdmission,
            QuestionKind::CredentialReuse,
            QuestionKind::RuntimeQuota,
            QuestionKind::InsightSelection,
            QuestionKind::ToolRelevance,
            QuestionKind::FanoutAggregate,
            QuestionKind::DelegationAdmission,
            QuestionKind::AnswerRanking,
            QuestionKind::ServingTechniqueActivation,
            QuestionKind::ReasoningParadigm,
            QuestionKind::ModelCapacity,
            QuestionKind::ProcessRemediation,
            QuestionKind::ProcessPlan,
            QuestionKind::IngestShortfall,
            QuestionKind::CapacityProposal,
            QuestionKind::ScaleIntent,
            QuestionKind::ProbeSelection,
            QuestionKind::RewardDecomposition,
            QuestionKind::CurriculumPromotion,
            QuestionKind::ResearchNext,
            QuestionKind::FeatureMatch,
            QuestionKind::PopulationDrift,
            QuestionKind::FanoutWidth,
            QuestionKind::AutonomyVerdict,
            QuestionKind::TrajectoryJoin,
            QuestionKind::StepVerification,
            QuestionKind::AdversarialAdmission,
            QuestionKind::SkillEvolution,
            QuestionKind::PlaybookTriage,
            QuestionKind::TrajectoryLadder,
            QuestionKind::AnalystSynthesis,
            QuestionKind::Recruit,
            QuestionKind::ResolveConflict,
            QuestionKind::CouncilCoordination,
            QuestionKind::MergeArming,
            QuestionKind::RevertOrFixForward,
            QuestionKind::DefectTriage,
            QuestionKind::Promotion,
        ];
        let exhaustive = match kinds[0] {
            QuestionKind::Route
            | QuestionKind::Rank
            | QuestionKind::Classify
            | QuestionKind::ResolveEntity
            | QuestionKind::SchemaMapping
            | QuestionKind::TemplateChoice
            | QuestionKind::RetrievalPlan
            | QuestionKind::IngestionLane
            | QuestionKind::EnrichmentSchedule
            | QuestionKind::PreToolRisk
            | QuestionKind::ConnectorEventTriage
            | QuestionKind::ConnectorToolChoice
            | QuestionKind::ConnectorWriteBack
            | QuestionKind::ToolSubset
            | QuestionKind::HarnessConfig
            | QuestionKind::TaskPlan
            | QuestionKind::HarnessContrast
            | QuestionKind::Placement
            | QuestionKind::PlacementPlan
            | QuestionKind::ContextBudget
            | QuestionKind::OverWindowPlan
            | QuestionKind::InstructionDisclosure
            | QuestionKind::ChangesetReview
            | QuestionKind::TemporalBasis
            | QuestionKind::RoundVerdict
            | QuestionKind::StageAllocation
            | QuestionKind::ProbeVerdict
            | QuestionKind::ItemRecommendation
            | QuestionKind::LoadSet
            | QuestionKind::UsageGuidance
            | QuestionKind::BranchOutcome
            | QuestionKind::EditConcentration
            | QuestionKind::ThoughtSearch
            | QuestionKind::ComparativeAnalysisPlan
            | QuestionKind::ContextPackage
            | QuestionKind::Topology
            | QuestionKind::ProgramLookup
            | QuestionKind::SkillWorkflowCompile
            | QuestionKind::SkillUpgrade
            | QuestionKind::TeamCoordination
            | QuestionKind::CapabilityDesignation
            | QuestionKind::ContextReplacement
            | QuestionKind::RecursionMode
            | QuestionKind::SubCallAdmission
            | QuestionKind::CommandConfig
            | QuestionKind::RunAdjustment
            | QuestionKind::PathPreference
            | QuestionKind::CorrectionAdmission
            | QuestionKind::FieldShielding
            | QuestionKind::GraphIntake
            | QuestionKind::ToolNecessity
            | QuestionKind::DecisionTableEval
            | QuestionKind::BranchSelect
            | QuestionKind::FeedbackLoop
            | QuestionKind::PlanningAdmission
            | QuestionKind::ToolActionAdmission
            | QuestionKind::A2aRoute
            | QuestionKind::PaperAdmission
            | QuestionKind::HeldItemAdmission
            | QuestionKind::CriterionReview
            | QuestionKind::GeneratedCodeAdmission
            | QuestionKind::CredentialReuse
            | QuestionKind::RuntimeQuota
            | QuestionKind::InsightSelection
            | QuestionKind::ToolRelevance
            | QuestionKind::FanoutAggregate
            | QuestionKind::DelegationAdmission
            | QuestionKind::AnswerRanking
            | QuestionKind::ServingTechniqueActivation
            | QuestionKind::ReasoningParadigm
            | QuestionKind::ModelCapacity
            | QuestionKind::ProcessRemediation
            | QuestionKind::ProcessPlan
            | QuestionKind::IngestShortfall
            | QuestionKind::CapacityProposal
            | QuestionKind::ScaleIntent
            | QuestionKind::ProbeSelection
            | QuestionKind::TrajectoryLadder
            | QuestionKind::AnalystSynthesis
            | QuestionKind::Recruit
            | QuestionKind::ResolveConflict
            | QuestionKind::CouncilCoordination
            | QuestionKind::MergeArming
            | QuestionKind::RevertOrFixForward
            | QuestionKind::DefectTriage
            | QuestionKind::Promotion
            | QuestionKind::RewardDecomposition
            | QuestionKind::CurriculumPromotion
            | QuestionKind::ResearchNext
            | QuestionKind::FeatureMatch
            | QuestionKind::PopulationDrift
            | QuestionKind::FanoutWidth
            | QuestionKind::AutonomyVerdict
            | QuestionKind::TrajectoryJoin
            | QuestionKind::StepVerification
            | QuestionKind::AdversarialAdmission
            | QuestionKind::SkillEvolution
            | QuestionKind::PlaybookTriage => true,
        };
        assert!(exhaustive);
        kinds
    }

    // spec: MEDULLA-DECISION-ENGINE-R001.101.1
    #[test]
    fn every_question_kind_has_a_family() {
        let kinds = all_kinds();
        // The exhaustive match in `all_kinds` proves every variant is listed; no
        // literal count (a line every family branch would edit). Each kind once:
        for (i, kind) in kinds.iter().enumerate() {
            assert!(
                !kinds[i + 1..].contains(kind),
                "one family registration per QuestionKind variant: {kind:?} listed twice"
            );
        }
        for kind in &kinds {
            // The registry is total by construction: `for_kind` cannot name
            // a kind the exhaustive match does not map.
            let family = for_kind(*kind);
            assert_eq!(family.refusals("detail".to_string()), "detail");
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R001.101.4
    /// The family template (`TEMPLATE.md`) embeds `template.rs` verbatim, and
    /// that file compiles against the live trait and applies its refusal
    /// table (MEDULLA-DECISION-ENGINE-R001.101.4).
    #[test]
    fn template_compiles() {
        let source = include_str!("template.rs").trim_end();
        let doc = include_str!("TEMPLATE.md");
        assert!(
            doc.contains(&format!("```rust\n{source}\n```")),
            "TEMPLATE.md must embed template.rs verbatim"
        );
        let family = super::super::template::TemplateFamily;
        assert_eq!(
            family.refusals("no candidate left".to_string()),
            "template_no_candidate"
        );
        assert_eq!(family.refusals("other".to_string()), "other");
    }
}
