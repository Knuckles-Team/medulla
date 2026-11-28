//! The Decide layer's served surface (RF-ADR-010).
//!
//! Four methods, four files: assembly, its commit, the statistical executor
//! and the two admin jobs. Each entry point's signature is frozen by the
//! contract wave; the package that lands a handler replaces only its body.

/// The server state every Decide entry point is handed.
pub type SharedState = std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>;

#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod commit;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod planned_write;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod shape;

// The statistical package (EH-056 ... EH-074): candidate sources, the
// decision executor, the NL template binding and the two admin jobs.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod candidates;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod connector_points;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod drift_registry;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod family;
#[cfg(feature = "decide")]
pub mod features;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod prompt_candidates;
#[cfg(feature = "decide")]
#[cfg(feature = "decide")]
pub use family::task_context::read_task_context;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_adapter_lo;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_capability_outcome;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_classes;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_designation;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_evaluator;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_executor;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_generation;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_harness_adoption;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_harness_contrast;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_lifecycle_record;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_load_set;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_log;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_log_reader;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_model_capacity;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_pointer;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_promotion;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_proposals;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_recommend;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_replay_lo;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_skill_ablation;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_skill_tier;
// MEDULLA-DECISION-ENGINE-R001.100.2: the served safety bound stage.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod safety_stage;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_resolve;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_usage_guidance;
// MEDULLA-DECISION-ENGINE-R129.1: rule promotion attach point.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod program_embedding;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod run_record;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_retention;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_slate;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod tool_index;
// EH-523: the per-component split of a slate's outcome on the outcome aggregate.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_slate_attribution;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_support;
// MEDULLA-DECISION-ENGINE-R045: the smallest covering tool subset within a
// context-token budget.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_placement;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_placement_plan;
#[cfg(feature = "decide")]
mod stat_thought_search;
// MEDULLA-DECISION-ENGINE-R140.104: the trajectory decisions of a task plan.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_tool_subset;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_vectors;
// EH-396: the query adapter on the served read paths.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod served_adapter;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod telemetry;

// Swarm topology through the assembly decision (SWARM-TOPOLOGY-DECIDE-DESIGN):
// the schema entailments and capacity headroom a topology question reads, and
// the publish-time topology shape check.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod assemble_shield;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod capacity_premise;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod topology_read;
#[cfg(feature = "decide")]
mod topology_schema;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod triage_read;

#[cfg(feature = "statechart")]
pub use stat_proposals::handle_revision_proposal_list;
