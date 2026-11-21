//! Process plans (MEDULLA-CQ-ORCH-R704): the translation of one stored canonical workflow
//! into a task plan. Nothing here names a source format; the plan reads the canonical form
//! and the binding links of its stored step nodes only (R704.9).

pub mod bind;
pub mod plan;
pub mod rank;

#[cfg(test)]
mod tests;
