pub use medulla_h_decide_stat_tests::decide::stat_tests::*;

mod commit_tests;
mod er_proposals_tests;
// MEDULLA-DECISION-ENGINE-R157.3, R157.4: the served delegation quality gate.
mod delegation_quality_tests;
mod distilled_rank_tests;
// MEDULLA-DECISION-ENGINE-R160.3: package runs recorded through DecisionLog learn.
mod package_run_tests;
// MEDULLA-DECISION-ENGINE-R100.1.1: feedback events recorded through DecisionLog learn.
mod feedback_tests;
// MEDULLA-DECISION-ENGINE-R132.3: retrieved entries admitted by trust tier.
mod entry_trust_tests;
// MEDULLA-DECISION-ENGINE-R823, R825: reasoning paradigm and model capacity questions.
mod model_capacity_tests;
// MEDULLA-DECISION-ENGINE-R163.3, R163.4: graph intake served through Decide.
mod graph_intake_tests;
// MEDULLA-DECISION-ENGINE-R701.4, R701.5: failure refusals and gap evidence.
mod failure_patterns_tests;
// MEDULLA-DECISION-ENGINE-R702.3 to R702.5: human corrections planned by CorrectionRecord.
mod correction_tests;
// MEDULLA-DECISION-ENGINE-R708.3, R708.4, R708.5: knowledge flags.
mod knowledge_flag_tests;
// MEDULLA-DECISION-ENGINE-R709.3 to R709.7: the knowledge curation cycle.
mod knowledge_curation_tests;
// MEDULLA-DECISION-ENGINE-R710.3, R710.4, R710.5: friction reports.
mod friction_report_tests;
// MEDULLA-DECISION-ENGINE-R029.2.2: the served context budget partition.
mod context_budget_tests;
// MEDULLA-DECISION-ENGINE-R137.8: the served package selection journey.
mod context_package_tests;
// MEDULLA-DECISION-ENGINE-R136.4: served round verdict and stage allocation.
mod evolution_round_tests;
// MEDULLA-DECISION-ENGINE-R138.100.2: served generated-code admission.
mod generated_code_admission_tests;
// MEDULLA-DECISION-ENGINE-R832: served credential reuse.
mod credential_reuse_tests;
// MEDULLA-DECISION-ENGINE-R137.3: the served over-window plan.
mod over_window_plan_tests;
// MEDULLA-DECISION-ENGINE-R137.14: the served held-out replacement gate.
mod context_replacement_tests;
// MEDULLA-DECISION-ENGINE-R137.100: relevance-triggered instruction disclosure.
mod instruction_disclosure_tests;
// MEDULLA-DECISION-ENGINE-R106.1: council coordination, served.
mod council_tests;
// MEDULLA-DECISION-ENGINE-R135.12 to R135.14: insight submission and selection, served.
mod insight_tests;
// MEDULLA-DECISION-ENGINE-R134.100: the served fan-out aggregation.
mod fanout_tests;
// MEDULLA-DECISION-ENGINE-R803.8: adoption, contrast and lesson stored in one write, served.
mod harness_adoption_tests;
// MEDULLA-DECISION-ENGINE-R805.15: an operator reviews an evaluated revision, served.
mod review_tests;
// MEDULLA-SDLC-R013, R019.4: scenario runs and would-dispatch decisions in the one log.
mod connector_toolchoice_tests;
mod connector_triage_tests;
mod connector_writeback_tests;
#[cfg(feature = "query")]
mod evaluator_tests;
mod knob_tests;
mod lifecycle_record_tests;
mod nl_tests;
// MEDULLA-DECISION-ENGINE-R031.100: paper relevance admission.
#[cfg(feature = "query")]
mod paper_admission_tests;
// MEDULLA-DECISION-ENGINE-R038.3: the served held-item admission gate.
mod held_item_admission_tests;
mod ingest_shortfall_tests;
mod item_recommendation_tests;
// MEDULLA-DECISION-ENGINE-R151.1: feature-importance model on the fit request.
mod importance_penalty_tests;
// MEDULLA-DECISION-ENGINE-R020.1.1: context digest and matched history.
mod comparative_analysis_tests;
mod concentration_tests;
mod defect_triage_tests;
mod exploration_pool_tests;
mod memory_change_tests;
mod merge_arming_tests;
// MEDULLA-DECISION-ENGINE-R818: probe selection, served.
mod feedback_loop_tests;
// MEDULLA-DECISION-ENGINE-R174.8: drift monitors select the shards a refresh covers.
mod drift_refresh_tests;
// MEDULLA-DECISION-ENGINE-R174.9: fresh data through the search source.
mod freshness_refresh_tests;
#[cfg(feature = "statechart")]
mod loops_tests;
// MEDULLA-DECISION-ENGINE-R100.101.3: served grounded evaluation.
mod grounded_eval_tests;
// MEDULLA-DECISION-ENGINE-R161.3: correction admission, served.
mod correction_admission_tests;
// MEDULLA-DECISION-ENGINE-R162.3: field shielding, served.
mod field_shielding_tests;
// MEDULLA-DECISION-ENGINE-R148.3: command config, served.
mod command_config_tests;
mod curriculum_promotion_tests;
#[cfg(feature = "statechart")]
mod feature_match_tests;
mod limit_guard_tests;
#[cfg(feature = "statechart")]
mod org_runtime_tests;
