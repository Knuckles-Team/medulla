pub use medulla_h_decide_stat_tests::decide::stat_tests::*;

mod reasoning_paradigm_tests;
mod retention_tests;
mod route_tests;
mod tool_subset_tests;
// MEDULLA-DECISION-ENGINE-R711.3, R711.4, R711.5: the skill evaluation job.
mod skill_eval_tests;
// MEDULLA-DECISION-ENGINE-R712.3, R712.4, R712.5: ablation, canary and compile staleness.
mod skill_ablation_tests;
// MEDULLA-DECISION-ENGINE-R713.3 to R713.7: skill telemetry.
mod skill_telemetry_tests;
mod skill_tier_tests;
// MEDULLA-DECISION-ENGINE-R833: served scale intents.
mod scale_intent_tests;
// MEDULLA-DECISION-ENGINE-R038.2: the served tool-action admission gate.
mod tool_action_admission_tests;
// MEDULLA-DECISION-ENGINE-R133.3: the served runtime quota.
mod runtime_quota_tests;
// MEDULLA-DECISION-ENGINE-R135.17.1: the replay row of a stored program.
mod program_match_tests;
// MEDULLA-DECISION-ENGINE-R134.12 and R134.12.1: the rung choice and the paired verdicts, served.
mod trajectory_ladder_tests;
// MEDULLA-DECISION-ENGINE-R134.13: cited analyst synthesis, served.
mod synthesis_tests;
// MEDULLA-DECISION-ENGINE-R828: the branch-merge conflict winner, served.
mod policy_activation_tests;
mod resolve_conflict_tests;
mod tool_necessity_tests;
mod tool_relevance_tests;
// MEDULLA-DECISION-ENGINE-R156.3 to R156.5: skill documents compile to programs, served.
mod skill_workflow_tests;
// MEDULLA-DECISION-ENGINE-R137.13: the served recursion-mode decision.
mod recursion_mode_tests;
// MEDULLA-DECISION-ENGINE-R137.15: the served sub-call admission check.
mod scope_bound_tests;
mod sub_call_admission_tests;
// MEDULLA-DECISION-ENGINE-R817: the revision proposal list, tenant scoped and paged.
mod revision_proposal_tests;
#[cfg(feature = "redb")]
mod topology_tests;
// MEDULLA-PLACEMENT-R700.3: the served placement plan question.
#[cfg(all(feature = "decide", feature = "redb"))]
mod placement_plan_tests;
#[cfg(feature = "query")]
mod retrieval_plan_tests;
// MEDULLA-DECISION-ENGINE-R821: per-source trust and retrieval strategy adoption.
#[cfg(feature = "query")]
mod retrieval_adoption_tests;
#[cfg(feature = "query")]
mod run_adjustment_tests;
mod run_credit_tests;
mod session_note_tests;
// EH-525: the learned reputation relation and SOURCE RELIABILITY.
mod pretool_risk_tests;
mod process_decision_tests;
mod process_plan_tests;
mod process_remediation_tests;
mod prompt_candidate_tests;
#[cfg(feature = "query")]
mod reputation_tests;
mod serving_technique_tests;
#[cfg(test)]
mod transfer_tests;
mod writeback_queue_tests;
// MEDULLA-DECISION-ENGINE-R174.7.3.1: serve the scoped adapter.
#[cfg(feature = "query")]
mod scoped_adapter_tests;
// EH-528: replay evaluation (its validation kernels are finance's).
mod planning_admission_tests;
mod probe_selection_tests;
mod probe_verdict_tests;
#[cfg(feature = "finance")]
mod replay_tests;
mod revert_family_tests;
mod schema_mapping_tests;
mod skill_upgrade_tests;
mod team_coordination_tests;
mod trajectory_tests;
// MEDULLA-DECISION-ENGINE-R072.100: one span per pipeline stage.
mod run_observation_tests;
mod telemetry_tests;
// MEDULLA-DECISION-ENGINE-R750: bounded multi-step reasoning search, served.
mod thought_search_tests;
// MEDULLA-DECISION-ENGINE-R146.3: temporal basis, served.
mod temporal_basis_tests;
// MEDULLA-DECISION-ENGINE-R100.100.2: served pinned ranking evaluation.
mod population_tests;
mod research_voi_tests;
mod retrieval_eval_tests;
mod reward_decomposition_tests;
mod safety_bound_tests;
mod step_verification_tests;
// MEDULLA-DECISION-ENGINE-R703.3: skill gaps, neologisms and merges, served.
mod skill_evolution_tests;
// MEDULLA-DECISION-ENGINE-R704.3: playbook triage, served.
mod playbook_triage_tests;
// MEDULLA-DECISION-ENGINE-R177.3 to R177.5: usage guidance served through Decide.
#[cfg(feature = "redb")]
mod usage_guidance_tests;
