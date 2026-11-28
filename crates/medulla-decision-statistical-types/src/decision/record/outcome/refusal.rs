//! The typed-refusal trait of the decision plane, apart from the outcome types so
//! a family module can implement it without depending on the registry lookup.

use medulla_types::refusal_code::RefusalCode;

/// A typed refusal enum of the decision plane. Each variant maps to exactly one
/// registered code (`crate::decision::codes`), so no refusal reaches the wire as
/// a catch-all (MEDULLA-DECISION-ENGINE-R010.100.2).
pub trait DecisionRefusal {
    /// The registered code of this refusal.
    fn code(&self) -> RefusalCode;
    /// The human detail that accompanies the code.
    fn detail(&self) -> String;
}
