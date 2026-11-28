//! Refusal codes of the composed execution answer's per-part abstentions
//! (MEDULLA-CQ-ORCH-R008.3).
//!
//! Each of the seven parts of `AgentExecutionAnswer` answers or abstains on its own. An
//! abstained part carries one of these codes inside `AbstainReason::Family`.

use medulla_types::refusal_code::RefusalCode;

/// The part's concept has no connected authority, so its source could not be read.
pub const ORCH_PART_NO_SOURCE: RefusalCode = RefusalCode::new("ORCH_PART_NO_SOURCE");
/// The part's query returned no row.
pub const ORCH_PART_EMPTY: RefusalCode = RefusalCode::new("ORCH_PART_EMPTY");
/// The assembly the part depends on abstained; its reason is copied into the detail.
pub const ORCH_PART_ASSEMBLY_ABSTAINED: RefusalCode =
    RefusalCode::new("ORCH_PART_ASSEMBLY_ABSTAINED");

/// The execution answer family's own refusal codes.
pub const REFUSALS: &[RefusalCode] = &[
    ORCH_PART_NO_SOURCE,
    ORCH_PART_EMPTY,
    ORCH_PART_ASSEMBLY_ABSTAINED,
];
