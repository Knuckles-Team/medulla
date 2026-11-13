//! Knob tables of the memory, participant and profile families
//! (MEDULLA-DECISION-ENGINE-R179.8). Each module declares its own
//! `KNOBS` table; [`REGISTRY`] lists them for the knob registry.

pub mod context;
pub mod criterion_review;
pub mod fanout;
pub mod memory;
pub mod participant;
pub mod placement;
pub mod profile;
pub mod prompt_candidate;
pub mod resolve_entity;
pub mod red_team;
pub mod round_verdict;
pub mod route;
pub mod schema_mapping;
pub mod task_plan;
pub mod topology;
pub mod trajectory;
pub mod verifier;

use super::knobs::KnobSpec;

/// The family tables this crate registers.
pub const REGISTRY: &[&[KnobSpec]] = &[
    context::KNOBS,
    criterion_review::KNOBS,
    fanout::KNOBS,
    memory::KNOBS,
    participant::KNOBS,
    placement::KNOBS,
    profile::KNOBS,
    prompt_candidate::KNOBS,
    task_plan::KNOBS,
    resolve_entity::KNOBS,
    red_team::KNOBS,
    round_verdict::KNOBS,
    route::KNOBS,
    schema_mapping::KNOBS,
    topology::KNOBS,
    trajectory::KNOBS,
    verifier::KNOBS,
    super::ingestion_admission::KNOBS,
    super::statistical::run_credit::KNOBS,
    super::thought_search::KNOBS,
    super::capability_designation::KNOBS,
    super::statistical::distilled::KNOBS,
    super::delegation_quality::KNOBS,
    super::field_shielding::KNOBS,
    super::paper_admission::KNOBS,
    super::held_item::KNOBS,
    super::serving_technique::KNOBS,
    super::process_remediation::KNOBS,
    super::command_config::KNOBS,
    super::instruction_disclosure::KNOBS,
    super::memory_benchmark::KNOBS,
    super::run_adjustment::KNOBS,
    super::path_success::KNOBS,
    super::temporal_query::KNOBS,
    super::comparative_analysis::KNOBS,
    super::run_governance::KNOBS,
    super::program_store::KNOBS,
    super::program_store::insight::KNOBS,
    super::program_store::preplan::KNOBS,
    super::tool_relevance::KNOBS,
    super::delegation::KNOBS,
    super::comparative_analysis::KNOBS,
    super::memory_rewrite::KNOBS,
    super::memory_benchmark::KNOBS,
    super::load_set_knobs::KNOBS,
];
