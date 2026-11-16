//! Natural language to SQL (MEDULLA-FEDERATED-QUERY-R058.4.10).
//!
//! A [`SqlNlPlanner`] answers one SQL text per step. The text is parsed with the SQL
//! front end `Method::Sql` classifies with: anything other than a single `SELECT` is
//! refused `NL_MUTATION_REFUSED` and never executes. Every table and column it names
//! must be in the caller's catalog as the [`SchemaContext`] holds it; an unknown name
//! returns to the planner as typed feedback inside the bounded loop, and a spent loop
//! refuses `NL_QUERY_STEPS_EXHAUSTED`. Execution is the served SQL read path.

use std::collections::BTreeSet;

use medulla_sql_lang::sql::select_names::{select_names, SelectNamesError};

use super::context::{NlPlanStep, SchemaContext};
use super::planned_text::{feedback_text, run_text_loop, PlannedText, TextGround};
use super::{NlQueryBudget, NlQueryError};

/// The NL to SQL planning port: one SQL text for `question`, given the typed outcomes of
/// the refused steps so far.
pub trait SqlNlPlanner: Send + Sync {
    /// Produce one SQL text. An `Err` (network, model, empty output) is surfaced as a
    /// typed [`NlQueryError`], never a panic or a silent empty result.
    fn plan_sql(
        &self,
        context: &SchemaContext,
        question: &str,
        feedback: &[NlPlanStep],
    ) -> Result<String, NlQueryError>;
}

/// Check `text` against the catalog tables of `context`: one `SELECT`, every table and
/// column named in the catalog the caller may read.
pub fn ground_sql(context: &SchemaContext, text: &str) -> Result<(), TextGround> {
    let names = match select_names(text) {
        Ok(names) => names,
        Err(SelectNamesError::NotSelect(_)) => return Err(TextGround::Mutation),
        Err(SelectNamesError::Parse(message)) => return Err(TextGround::Feedback(message)),
    };
    let known: Vec<(String, BTreeSet<String>)> = context
        .tables()
        .iter()
        .map(|table| {
            (
                table.name.to_lowercase(),
                table.columns.iter().map(|c| c.to_lowercase()).collect(),
            )
        })
        .collect();
    let table_list = known
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut visible: BTreeSet<&String> = BTreeSet::new();
    for table in &names.tables {
        match known.iter().find(|(name, _)| name == table) {
            Some((_, columns)) => visible.extend(columns),
            None => {
                return Err(TextGround::Feedback(format!(
                "SQL_TABLE_UNKNOWN: table `{table}` is not in the catalog (tables: {table_list})"
            )))
            }
        }
    }
    if names.columns_renamed {
        return Ok(());
    }
    match names
        .columns
        .iter()
        .find(|column| !visible.contains(column))
    {
        Some(column) => {
            let columns = visible
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(TextGround::Feedback(format!(
                "SQL_COLUMN_UNKNOWN: column `{column}` is not on the tables the query reads \
                 (columns: {columns})"
            )))
        }
        None => Ok(()),
    }
}

/// Run the bounded correction loop for SQL: plan, enforce the budget, parse and ground
/// against `context`. The accepted text is the planner's, unchanged.
pub fn plan_sql_grounded(
    planner: &dyn SqlNlPlanner,
    question: &str,
    context: &SchemaContext,
    budget: NlQueryBudget,
    max_steps: usize,
) -> Result<PlannedText, NlQueryError> {
    run_text_loop(
        "sql",
        budget,
        max_steps,
        |steps| planner.plan_sql(context, question, steps),
        |text| ground_sql(context, text),
    )
}

/// The system prompt of the SQL planner of [`super::UreqNlPlanner`].
pub const SQL_SYSTEM_PROMPT: &str = "You translate a question into ONE PostgreSQL SELECT \
statement. Use only the tables and columns listed under Catalog. Reply with the SQL text only: \
no explanation, no code fence, no second statement, and never a statement that writes.";

/// The user message of a SQL planning step: the catalog, the question and the typed
/// outcome of every refused step.
pub fn sql_user_message(
    context: &SchemaContext,
    question: &str,
    feedback: &[NlPlanStep],
) -> String {
    let mut out = String::from("Catalog:\n");
    for table in context.tables() {
        out.push_str(&format!("  {}({})\n", table.name, table.columns.join(", ")));
    }
    out.push_str(&format!("\nQuestion: {question}\n"));
    let refused = feedback_text(feedback);
    if !refused.is_empty() {
        out.push('\n');
        out.push_str(&refused);
        out.push_str("Reply with ONE corrected SQL SELECT that uses only the catalog above.");
    }
    out
}

#[cfg(feature = "nl-query")]
impl SqlNlPlanner for super::UreqNlPlanner {
    fn plan_sql(
        &self,
        context: &SchemaContext,
        question: &str,
        feedback: &[NlPlanStep],
    ) -> Result<String, NlQueryError> {
        let user = sql_user_message(context, question, feedback);
        let content = self
            .complete(SQL_SYSTEM_PROMPT, &user)
            .map_err(NlQueryError::Planner)?;
        let text = super::strip_query(&content);
        if text.is_empty() {
            return Err(NlQueryError::Planner(
                "nl-query: LLM produced an empty query".to_string(),
            ));
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use medulla_graph_core::graph::GraphView;

    use super::super::context::NlTable;
    use super::*;

    fn context() -> SchemaContext {
        let mut context = SchemaContext::from_view(&GraphView::default());
        context.set_tables(vec![
            NlTable {
                name: "orders".into(),
                columns: vec!["id".into(), "total".into(), "customer_id".into()],
            },
            NlTable {
                name: "customers".into(),
                columns: vec!["id".into(), "name".into()],
            },
        ]);
        context
    }

    struct Scripted {
        answers: Mutex<Vec<String>>,
        feedback_seen: Mutex<Vec<usize>>,
    }

    impl Scripted {
        fn new(answers: &[&str]) -> Self {
            Self {
                answers: Mutex::new(answers.iter().rev().map(|s| s.to_string()).collect()),
                feedback_seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl SqlNlPlanner for Scripted {
        fn plan_sql(
            &self,
            _context: &SchemaContext,
            _question: &str,
            feedback: &[NlPlanStep],
        ) -> Result<String, NlQueryError> {
            self.feedback_seen.lock().unwrap().push(feedback.len());
            self.answers
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| NlQueryError::Planner("script exhausted".into()))
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.10
    #[test]
    fn select_grounds_and_mutation_refuses() {
        let context = context();
        assert_eq!(
            ground_sql(
                &context,
                "SELECT c.name, SUM(o.total) AS spend FROM orders o \
                 JOIN customers c ON c.id = o.customer_id GROUP BY c.name ORDER BY spend"
            ),
            Ok(())
        );
        for text in [
            "DELETE FROM orders",
            "DROP TABLE orders",
            "INSERT INTO orders (id) VALUES (1)",
            "COPY orders TO STDOUT",
            "SELECT 1; DELETE FROM orders",
            "SELECT id INTO archive FROM orders",
        ] {
            assert_eq!(
                ground_sql(&context, text),
                Err(TextGround::Mutation),
                "{text}"
            );
        }
        // Unknown names come back as typed feedback, not as a refusal.
        for (text, code) in [
            ("SELECT id FROM invoices", "SQL_TABLE_UNKNOWN"),
            ("SELECT price FROM orders", "SQL_COLUMN_UNKNOWN"),
            ("SELECT name FROM orders", "SQL_COLUMN_UNKNOWN"),
        ] {
            match ground_sql(&context, text) {
                Err(TextGround::Feedback(message)) => {
                    assert!(message.starts_with(code), "{message}")
                }
                other => panic!("{text}: {other:?}"),
            }
        }
        // A mutation ends the request at once: the planner is asked once.
        let planner = Scripted::new(&["DELETE FROM orders", "SELECT id FROM orders"]);
        let refused = plan_sql_grounded(
            &planner,
            "remove orders",
            &context,
            NlQueryBudget::default(),
            3,
        );
        assert!(matches!(refused, Err(NlQueryError::MutationRefused { .. })));
        assert_eq!(planner.feedback_seen.lock().unwrap().len(), 1);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.10
    #[test]
    fn unknown_name_corrects_once_then_exhausts() {
        let context = context();
        let planner = Scripted::new(&["SELECT price FROM orders", "SELECT total FROM orders"]);
        let planned = plan_sql_grounded(&planner, "totals", &context, NlQueryBudget::default(), 2)
            .expect("the corrected step grounds");
        assert_eq!(planned.text, "SELECT total FROM orders");
        assert_eq!(planned.steps.len(), 2);
        assert_eq!(planned.steps[0].dialect, "sql");
        assert!(planned.steps[0].outcome.starts_with("SQL_COLUMN_UNKNOWN"));
        assert_eq!(planned.steps[1].outcome, "accepted");
        assert_eq!(*planner.feedback_seen.lock().unwrap(), vec![0, 1]);

        let planner = Scripted::new(&["SELECT a FROM nowhere", "SELECT b FROM nowhere"]);
        match plan_sql_grounded(&planner, "x", &context, NlQueryBudget::default(), 2) {
            Err(NlQueryError::StepsExhausted { steps }) => assert_eq!(steps.len(), 2),
            other => panic!("{other:?}"),
        }
    }
}
