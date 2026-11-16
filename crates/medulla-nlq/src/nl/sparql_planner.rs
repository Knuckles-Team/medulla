//! Natural language to SPARQL (MEDULLA-FEDERATED-QUERY-R058.4.11).
//!
//! A [`SparqlNlPlanner`] answers one SPARQL text per step. The text is parsed by the
//! engine's SPARQL parser: only `SELECT`, `ASK`, `CONSTRUCT` and `DESCRIBE` are
//! accepted, and any update form refuses `NL_MUTATION_REFUSED` without executing. Every
//! predicate IRI the text names (triple patterns, property paths, `FILTER EXISTS`, the
//! construct template) must be a property of the composed ontology the
//! [`SchemaContext`] holds, matched by its local name, or `rdf:type`, the one predicate
//! that expresses a class and belongs to no ontology. An unknown IRI returns to the
//! planner as typed feedback inside the bounded loop, and a spent loop refuses
//! `NL_QUERY_STEPS_EXHAUSTED`. Execution is the served SPARQL path.

use medulla_rdf::sparql::{query_shape, ShapeError};

use super::context::{NlPlanStep, SchemaContext};
use super::planned_text::{feedback_text, run_text_loop, PlannedText, TextGround};
use super::{NlQueryBudget, NlQueryError};

/// `rdf:type`: how SPARQL names a class membership.
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// The NL to SPARQL planning port: one SPARQL text for `question`, given the typed
/// outcomes of the refused steps so far.
pub trait SparqlNlPlanner: Send + Sync {
    /// Produce one SPARQL text. An `Err` is surfaced as a typed [`NlQueryError`].
    fn plan_sparql(
        &self,
        context: &SchemaContext,
        question: &str,
        feedback: &[NlPlanStep],
    ) -> Result<String, NlQueryError>;
}

fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// Check `text` against the composed ontology properties of `context`.
pub fn ground_sparql(context: &SchemaContext, text: &str) -> Result<(), TextGround> {
    let shape = match query_shape(text) {
        Ok(shape) => shape,
        Err(ShapeError::Update) => return Err(TextGround::Mutation),
        Err(ShapeError::Parse(message) | ShapeError::Unsupported(message)) => {
            return Err(TextGround::Feedback(message))
        }
    };
    let properties = context.properties();
    match shape
        .predicates
        .iter()
        .find(|iri| iri.as_str() != RDF_TYPE && !properties.contains(local_name(iri)))
    {
        Some(iri) => {
            let known = properties
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            Err(TextGround::Feedback(format!(
                "SPARQL_PREDICATE_UNKNOWN: predicate <{iri}> is not a property of the \
                 ontology (properties: {known})"
            )))
        }
        None => Ok(()),
    }
}

/// Run the bounded correction loop for SPARQL: plan, enforce the budget, parse and
/// ground against `context`. The accepted text is the planner's, unchanged.
pub fn plan_sparql_grounded(
    planner: &dyn SparqlNlPlanner,
    question: &str,
    context: &SchemaContext,
    budget: NlQueryBudget,
    max_steps: usize,
) -> Result<PlannedText, NlQueryError> {
    run_text_loop(
        "sparql",
        budget,
        max_steps,
        |steps| planner.plan_sparql(context, question, steps),
        |text| ground_sparql(context, text),
    )
}

/// The system prompt of the SPARQL planner of [`super::UreqNlPlanner`].
pub const SPARQL_SYSTEM_PROMPT: &str = "You translate a question into ONE SPARQL 1.1 query \
(SELECT, ASK, CONSTRUCT or DESCRIBE). Use only the properties listed under Properties, \
as full IRIs or with a PREFIX you declare, and rdf:type for classes. Reply with the \
SPARQL text only: no explanation, no code fence, and never an update.";

/// The user message of a SPARQL planning step: the ontology properties, the question and
/// the typed outcome of every refused step.
pub fn sparql_user_message(
    context: &SchemaContext,
    question: &str,
    feedback: &[NlPlanStep],
) -> String {
    let properties = context
        .properties()
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!("Properties: {properties}\n\nQuestion: {question}\n");
    let refused = feedback_text(feedback);
    if !refused.is_empty() {
        out.push('\n');
        out.push_str(&refused);
        out.push_str("Reply with ONE corrected SPARQL query that uses only the properties above.");
    }
    out
}

#[cfg(feature = "nl-query")]
impl SparqlNlPlanner for super::UreqNlPlanner {
    fn plan_sparql(
        &self,
        context: &SchemaContext,
        question: &str,
        feedback: &[NlPlanStep],
    ) -> Result<String, NlQueryError> {
        let user = sparql_user_message(context, question, feedback);
        let content = self
            .complete(SPARQL_SYSTEM_PROMPT, &user)
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

    use super::super::context::OntologyProperty;
    use super::*;

    fn context() -> SchemaContext {
        let mut context = SchemaContext::from_view(&GraphView::default());
        context.link_ontology(&[
            OntologyProperty {
                name: "assignedTo".into(),
                label: None,
                domains: Vec::new(),
            },
            OntologyProperty {
                name: "priority".into(),
                label: None,
                domains: Vec::new(),
            },
        ]);
        context
    }

    struct Scripted(Mutex<Vec<String>>);

    impl SparqlNlPlanner for Scripted {
        fn plan_sparql(
            &self,
            _context: &SchemaContext,
            _question: &str,
            _feedback: &[NlPlanStep],
        ) -> Result<String, NlQueryError> {
            self.0
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| NlQueryError::Planner("script exhausted".into()))
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.11
    #[test]
    fn select_grounds_and_update_refuses() {
        let context = context();
        for text in [
            "SELECT ?s WHERE { ?s <http://example.test/ns#assignedTo> ?o }",
            "ASK { ?s <http://example.test/ns#priority>/<http://example.test/ns#assignedTo> ?o }",
            "SELECT ?s WHERE { ?s a <http://example.test/ns#Incident> . ?s <http://example.test/ns#priority> ?p }",
            "CONSTRUCT { ?s <http://example.test/ns#priority> ?p } WHERE { ?s <http://example.test/ns#priority> ?p }",
        ] {
            assert_eq!(ground_sparql(&context, text), Ok(()), "{text}");
        }
        for text in [
            "INSERT DATA { <http://x/s> <http://x/p> <http://x/o> }",
            "DELETE WHERE { ?s ?p ?o }",
            "CLEAR ALL",
            "DROP GRAPH <http://x/g>",
            "LOAD <http://x/doc>",
            "CREATE GRAPH <http://x/g>",
        ] {
            assert_eq!(
                ground_sparql(&context, text),
                Err(TextGround::Mutation),
                "{text}"
            );
        }
        for text in [
            "SELECT ?s WHERE { ?s <http://example.test/ns#invented> ?o }",
            "SELECT ?s WHERE { ?s <http://example.test/ns#priority> ?o FILTER EXISTS { ?s <http://example.test/ns#invented> ?x } }",
            "SELECT ?s WHERE { ?s <http://example.test/ns#priority>/<http://example.test/ns#invented> ?o }",
            "SELECT WHERE {",
        ] {
            assert!(
                matches!(ground_sparql(&context, text), Err(TextGround::Feedback(_))),
                "{text}"
            );
        }
        // An update ends the request at once, before any second step.
        let planner = Scripted(Mutex::new(vec![
            "ASK { ?s <http://example.test/ns#priority> ?o }".to_string(),
            "CLEAR ALL".to_string(),
        ]));
        let refused = plan_sparql_grounded(&planner, "wipe", &context, NlQueryBudget::default(), 3);
        assert!(matches!(refused, Err(NlQueryError::MutationRefused { .. })));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.11
    #[test]
    fn unknown_predicate_corrects_once_then_exhausts() {
        let context = context();
        let planner = Scripted(Mutex::new(vec![
            "SELECT ?s WHERE { ?s <http://example.test/ns#priority> ?o }".to_string(),
            "SELECT ?s WHERE { ?s <http://example.test/ns#invented> ?o }".to_string(),
        ]));
        let planned = plan_sparql_grounded(&planner, "q", &context, NlQueryBudget::default(), 2)
            .expect("the corrected step grounds");
        assert_eq!(planned.steps.len(), 2);
        assert!(planned.steps[0]
            .outcome
            .starts_with("SPARQL_PREDICATE_UNKNOWN"));
        assert_eq!(planned.steps[1].outcome, "accepted");

        let planner = Scripted(Mutex::new(vec![
            "SELECT ?s WHERE { ?s <http://x/b> ?o }".to_string(),
            "SELECT ?s WHERE { ?s <http://x/a> ?o }".to_string(),
        ]));
        match plan_sparql_grounded(&planner, "q", &context, NlQueryBudget::default(), 2) {
            Err(NlQueryError::StepsExhausted { steps }) => assert_eq!(steps.len(), 2),
            other => panic!("{other:?}"),
        }
    }
}
