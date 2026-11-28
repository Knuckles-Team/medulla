#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod assemble;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod family;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_decision_reads;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_generation_cycle;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_generation_hi;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_generation_space;
// MEDULLA-ORCH-VOCAB-R012.7.3: the policy review record, the activation guard and the loader.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod policy_activation;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_task_plan;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_topology;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_writeback;
// EH-394/395: retrieval outcomes on the decision log.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_retrieval;
// EH-525: the learned reputation relation of the decision views.
#[cfg(all(feature = "decide", feature = "query"))]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_reputation;
// EH-066 — the decision record views (SQL catalog relations + the UQL `DECISIONS` source).
#[cfg(feature = "decide")]
mod definitions_read;
#[cfg(all(feature = "decide", feature = "query"))]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_view_lo;
#[cfg(feature = "decide")]
mod steps_read;
