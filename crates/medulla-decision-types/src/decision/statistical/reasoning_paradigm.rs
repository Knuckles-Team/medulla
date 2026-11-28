//! The reasoning-paradigm choice question (MEDULLA-DECISION-ENGINE-R823).
//!
//! A `Decide` over `QuestionKind::ReasoningParadigm` chooses among four
//! paradigms. Each option declares the capability tag the paradigm requires
//! (`classification[0]`) and its expected value and cost in Q32. A paradigm
//! whose tag the task does not offer is illegal; the scorer ranks only the
//! legal set, and an empty legal set abstains `PARADIGM_NONE_ADMISSIBLE`.

use medulla_types::refusal_code::RefusalCode;

use crate::decision::record::outcome::DecisionRefusal;

/// The declared-option fact key a paradigm's expected value is published under.
pub const EXPECTED_VALUE_KEY: &str = "paradigm.expected_value_q32";
/// The declared-option fact key a paradigm's cost is published under.
pub const COST_KEY: &str = "paradigm.cost_q32";
/// The request param naming the capability tags the task offers.
pub const PARAM_TASK_TAGS: &str = "task_tags";

/// The four paradigms, in ascending option-id order, each with the capability
/// tag it requires.
pub const PARADIGMS: [(&str, &str); 4] = [
    ("causal", "reasoning:causal"),
    ("owl", "reasoning:owl"),
    ("rules", "reasoning:rules"),
    ("statistical", "reasoning:statistical"),
];

/// The family's refusal table, registered in `code_registry::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[RefusalCode::new("PARADIGM_NONE_ADMISSIBLE")];

/// `PARADIGM_NONE_ADMISSIBLE`.
pub const NONE_ADMISSIBLE: RefusalCode = REFUSALS[0];

/// The task offers no capability tag any paradigm requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoParadigmAdmissible;

impl DecisionRefusal for NoParadigmAdmissible {
    fn code(&self) -> RefusalCode {
        NONE_ADMISSIBLE
    }

    fn detail(&self) -> String {
        "no reasoning paradigm is legal for the task's capability tags".to_string()
    }
}

/// The paradigm option ids the task's `tags` make legal, in ascending order.
pub fn legal_paradigms<'a>(tags: &[String]) -> Vec<&'a str> {
    PARADIGMS
        .iter()
        .filter(|(_, required)| tags.iter().any(|tag| tag == required))
        .map(|(option, _)| *option)
        .collect()
}
