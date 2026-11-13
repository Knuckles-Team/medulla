//! The question-family registry (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! One [`DecisionFamily`] trait carries each question kind's stage hooks.
//! [`for_kind`] maps every `QuestionKind` to one family through a single
//! exhaustive `match`, so a kind without a family fails to compile. Each
//! family lives in its own `family/<kind>.rs` file.

mod a2a_route;
mod branch_outcome;
mod capability_designation;
mod changeset_review;
mod classify;
mod command_config;
mod comparative_analysis_plan;
mod connector_event_triage;
mod connector_tool_choice;
mod connector_write_back;
pub(super) mod context_budget;
mod context_package;
mod context_replacement;
mod correction_admission;
mod edit_concentration;
mod enrichment_schedule;
mod evolution_round;
mod feedback_loop;
mod field_shielding;
mod graph_intake;
mod harness_config;
mod harness_contrast;
mod held_item_admission;
mod ingestion_lane;
mod instruction_disclosure;
mod item_recommendation;
mod load_set;
mod over_window_plan;
mod paper_admission;
mod path_preference;
mod placement;
mod planning_admission;
mod pre_tool_risk;
mod probe_verdict;
mod program_store;
mod rank;
mod resolve_entity;
mod retrieval_plan;
mod route;
mod run_adjustment;
mod schema_mapping;
mod skill_upgrade;
mod skill_workflow;
mod support;
mod task_plan;
mod team_coordination;
#[cfg(test)]
mod template;
mod template_choice;
mod temporal_basis;
mod thought_search;
mod tool_necessity;
mod tool_subset;
mod topology;
mod usage_guidance;

pub(in crate::decide) use held_item_admission::held_item_view;
pub(in crate::decide) use command_config::command_view;
pub(in crate::decide) use instruction_disclosure::disclosure_view;
use medulla_decision_stats::decision::candidate::CandidateView;
pub(in crate::decide) use paper_admission::with_similarity;
pub(in crate::decide) use support::advisory;

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{DecisionPolicy, EvidenceClass, PremiseRef, ResolutionKind};
use medulla_graph_core::graph::GraphView;

use super::stat_executor::{Executed, ExecutionContext, Pinned};

/// The stage hooks one question family carries. Every hook has a default
/// that does nothing: a family overrides only the stages it owns.
pub trait DecisionFamily {
    /// Candidates stage: the kind's gate over declared options.
    fn validate_candidates(
        &self,
        _options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Candidates stage: the kind's gate over the declared option set, run
    /// before the generic sorted-and-unique check so the kind's own duplicate
    /// refusal is the one the caller sees.
    fn precheck_declared(&self, _options: &[DeclaredOption]) -> Result<(), String> {
        Ok(())
    }

    /// Constraints stage: the kind's admission over the request.
    fn constraints(&self, _request: &DecideRequest) -> Result<(), String> {
        Ok(())
    }

    /// Solve stage: the kind's specialized answer; `None` falls through to
    /// the shared matrix ladder.
    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        _request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        None
    }

    /// Solve stage for a family whose answer reads the verified caller's own
    /// decision log: it receives that caller's log reader. The default is
    /// `conclude`; one family overrides one of the two, never both.
    fn conclude_with_log(
        &self,
        ctx: &ExecutionContext,
        _reader: &super::stat_log::LogReader,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        self.conclude(ctx, request, policy, graph_view)
    }

    /// Solve stage, after `conclude` or the shared ladder: the kind's
    /// refinement of the executed answer (the rank family orders and filters
    /// an advisory answer; the context-budget family partitions the sources
    /// by what the ladder's head read). `request` is `None` when a logged
    /// record is replayed, which has only its stored inputs, and `views` is
    /// then empty. The default keeps the answer unchanged.
    fn refine(
        &self,
        _request: Option<&DecideRequest>,
        _pinned: &Pinned,
        _views: &[CandidateView],
        _policy: &DecisionPolicy,
        _policy_digest: &str,
        executed: Executed,
    ) -> Result<Executed, String> {
        Ok(executed)
    }

    /// Record stage: the kind's extra premises, derived from the request, from
    /// what was executed and from the digest of the policy in force.
    fn premises(
        &self,
        _request: &DecideRequest,
        _executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        Vec::new()
    }

    /// Whether the kind routes a natural-language utterance to a template and
    /// binds it (`stat_nl::binding`); only `template_choice` does
    /// (MEDULLA-DECISION-ENGINE-R001.101.3).
    fn binds_utterance(&self) -> bool {
        false
    }

    /// Record stage: the resolution kind the kind seals its outcome with;
    /// `None` keeps the generic one.
    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        None
    }

    /// Record stage: the evidence class the kind seals its outcome with;
    /// `None` keeps the generic one.
    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        None
    }

    /// Conclude stage: the kind's refusal mapping.
    fn refusals(&self, detail: String) -> String {
        detail
    }
}

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
    static PROBEVERDICTFAMILY: probe_verdict::ProbeVerdictFamily =
        probe_verdict::ProbeVerdictFamily;
    static ITEMRECOMMENDATIONFAMILY: item_recommendation::ItemRecommendationFamily =
        item_recommendation::ItemRecommendationFamily;
    static LOADSETFAMILY: load_set::LoadSetFamily = load_set::LoadSetFamily;
    static USAGEGUIDANCEFAMILY: usage_guidance::UsageGuidanceFamily =
        usage_guidance::UsageGuidanceFamily;
    static BRANCHOUTCOMEFAMILY: branch_outcome::BranchOutcomeFamily =
        branch_outcome::BranchOutcomeFamily;
    static EDITCONCENTRATIONFAMILY: edit_concentration::EditConcentrationFamily =
        edit_concentration::EditConcentrationFamily;
    static THOUGHTSEARCHFAMILY: thought_search::ThoughtSearchFamily =
        thought_search::ThoughtSearchFamily;
    static COMPARATIVEANALYSISPLANFAMILY: comparative_analysis_plan::ComparativeAnalysisPlanFamily =
        comparative_analysis_plan::ComparativeAnalysisPlanFamily;
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
    static TOOLNECESSITYFAMILY: tool_necessity::ToolNecessityFamily =
        tool_necessity::ToolNecessityFamily;
    static FEEDBACKLOOPFAMILY: feedback_loop::FeedbackLoopFamily =
        feedback_loop::FeedbackLoopFamily;
    static PLANNINGADMISSIONFAMILY: planning_admission::PlanningAdmissionFamily =
        planning_admission::PlanningAdmissionFamily;
    static A2AROUTEFAMILY: a2a_route::A2aRouteFamily = a2a_route::A2aRouteFamily;
    static PAPERADMISSIONFAMILY: paper_admission::PaperAdmissionFamily =
        paper_admission::PaperAdmissionFamily;
    static HELDITEMADMISSIONFAMILY: held_item_admission::HeldItemAdmissionFamily =
        held_item_admission::HeldItemAdmissionFamily;
    static CHANGESETREVIEWFAMILY: changeset_review::ChangesetReviewFamily =
        changeset_review::ChangesetReviewFamily;
    static TEMPORALBASISFAMILY: temporal_basis::TemporalBasisFamily =
        temporal_basis::TemporalBasisFamily;
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
        QuestionKind::CommandConfig => &COMMANDCONFIGFAMILY,
        QuestionKind::RunAdjustment => &RUNADJUSTMENTFAMILY,
        QuestionKind::PathPreference => &PATHPREFERENCEFAMILY,
        QuestionKind::CorrectionAdmission => &CORRECTIONADMISSIONFAMILY,
        QuestionKind::FieldShielding => &FIELDSHIELDINGFAMILY,
        QuestionKind::GraphIntake => &GRAPHINTAKEFAMILY,
        QuestionKind::ToolNecessity => &TOOLNECESSITYFAMILY,
        QuestionKind::FeedbackLoop => &FEEDBACKLOOPFAMILY,
        QuestionKind::PlanningAdmission => &PLANNINGADMISSIONFAMILY,
        QuestionKind::A2aRoute => &A2AROUTEFAMILY,
        QuestionKind::PaperAdmission => &PAPERADMISSIONFAMILY,
        QuestionKind::HeldItemAdmission => &HELDITEMADMISSIONFAMILY,
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
            QuestionKind::CommandConfig,
            QuestionKind::RunAdjustment,
            QuestionKind::PathPreference,
            QuestionKind::CorrectionAdmission,
            QuestionKind::FieldShielding,
            QuestionKind::GraphIntake,
            QuestionKind::ToolNecessity,
            QuestionKind::FeedbackLoop,
            QuestionKind::PlanningAdmission,
            QuestionKind::A2aRoute,
            QuestionKind::PaperAdmission,
            QuestionKind::HeldItemAdmission,
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
            | QuestionKind::CommandConfig
            | QuestionKind::RunAdjustment
            | QuestionKind::PathPreference
            | QuestionKind::CorrectionAdmission
            | QuestionKind::FieldShielding
            | QuestionKind::GraphIntake
            | QuestionKind::ToolNecessity
            | QuestionKind::FeedbackLoop
            | QuestionKind::PlanningAdmission
            | QuestionKind::A2aRoute
            | QuestionKind::PaperAdmission
            | QuestionKind::HeldItemAdmission => true,
        };
        assert!(exhaustive);
        kinds
    }

    // spec: MEDULLA-DECISION-ENGINE-R001.101.1
    #[test]
    fn every_question_kind_has_a_family() {
        let kinds = all_kinds();
        assert_eq!(
            kinds.len(),
            53,
            "one family registration per QuestionKind variant"
        );
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
        let family = super::template::TemplateFamily;
        assert_eq!(
            family.refusals("no candidate left".to_string()),
            "template_no_candidate"
        );
        assert_eq!(family.refusals("other".to_string()), "other");
    }
}
