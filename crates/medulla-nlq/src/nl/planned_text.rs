//! The bounded correction loop of the text dialects (MEDULLA-FEDERATED-QUERY-R058.4.10,
//! R058.4.11): SQL and SPARQL planners answer with one query text per step. Each text is
//! budget-checked, parsed and grounded by the dialect's own check; a refused step goes
//! back to the planner as typed feedback, and the loop stops at `max_steps`.

use super::context::{NlPlanStep, MAX_STEPS};
use super::{enforce_budget, NlQueryBudget, NlQueryError};

/// What a dialect's grounding check says about one candidate text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextGround {
    /// The text is not a read statement; natural language never writes. Refused at once.
    Mutation,
    /// The text parses or grounds badly; the typed reason goes back to the planner.
    Feedback(String),
}

/// An accepted text and every step that led to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedText {
    /// The accepted query text, exactly as the planner wrote it.
    pub text: String,
    /// Every step, in order, ending with the accepted one.
    pub steps: Vec<NlPlanStep>,
}

/// Run the loop for `dialect` (`sql` or `sparql`). `plan` asks the planner for one text
/// given the refused steps so far; `ground` checks it.
pub(super) fn run_text_loop(
    dialect: &str,
    budget: NlQueryBudget,
    max_steps: usize,
    mut plan: impl FnMut(&[NlPlanStep]) -> Result<String, NlQueryError>,
    ground: impl Fn(&str) -> Result<(), TextGround>,
) -> Result<PlannedText, NlQueryError> {
    let mut steps: Vec<NlPlanStep> = Vec::new();
    for _ in 0..max_steps.clamp(1, MAX_STEPS) {
        let text = plan(&steps)?;
        enforce_budget(&text, budget)?;
        match ground(&text) {
            Ok(()) => {
                steps.push(NlPlanStep {
                    dialect: dialect.to_string(),
                    query_text: text.clone(),
                    outcome: "accepted".to_string(),
                });
                return Ok(PlannedText { text, steps });
            }
            Err(TextGround::Mutation) => return Err(NlQueryError::MutationRefused { query: text }),
            Err(TextGround::Feedback(outcome)) => steps.push(NlPlanStep {
                dialect: dialect.to_string(),
                query_text: text,
                outcome,
            }),
        }
    }
    Err(NlQueryError::StepsExhausted { steps })
}

/// The typed outcome of every refused step, as the planner's next user message.
pub(super) fn feedback_text(steps: &[NlPlanStep]) -> String {
    steps
        .iter()
        .map(|step| {
            format!(
                "A previous answer was refused.\nAnswer: {}\nReason: {}\n",
                step.query_text, step.outcome
            )
        })
        .collect()
}
