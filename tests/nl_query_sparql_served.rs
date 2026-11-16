//! MEDULLA-FEDERATED-QUERY-R058.4.11: a served `NlQuery` with dialect `sparql` plans one
//! SPARQL query, grounds its predicates against the composed ontology and runs it on the
//! served SPARQL path; an update form is refused and never runs; an unknown predicate
//! returns to the planner and, when the steps are spent, refuses.
#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "nl-query",
    feature = "sparql",
    feature = "redb"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use medulla_nlq::nl::context::{NlPlanStep, SchemaContext};
use medulla_nlq::nl::NlQueryError;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_query_types::nl_query::NlDialect;

const SECRET: &str = "nl-query-sparql-served-secret";
const GRAPH: &str = "__commons__";

struct EchoPlanner;

impl medulla_nlq::nl::NlPlanner for EchoPlanner {
    fn id(&self) -> String {
        "scripted".to_string()
    }

    fn revision(&self) -> String {
        "1".to_string()
    }

    fn plan(&self, nl: &str, _schema_hint: &str) -> Result<String, String> {
        Ok(nl.to_string())
    }
}

/// Answers the question text as the SPARQL, so each case chooses its candidate.
struct EchoSparql;

impl medulla_nlq::nl::sparql_planner::SparqlNlPlanner for EchoSparql {
    fn plan_sparql(
        &self,
        _context: &SchemaContext,
        question: &str,
        _feedback: &[NlPlanStep],
    ) -> Result<String, NlQueryError> {
        Ok(question.to_string())
    }
}

fn state() -> test_support::SharedState {
    common::configure_authority();
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

fn nl_method(text: &str, dialect: NlDialect, max_steps: Option<u32>) -> Method {
    Method::NlQuery {
        text: text.to_string(),
        graph: GRAPH.to_string(),
        history: Vec::new(),
        dialect,
        max_steps,
    }
}

async fn nl(
    state: &test_support::SharedState,
    id: u64,
    text: &str,
    dialect: NlDialect,
    max_steps: Option<u32>,
) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(SECRET, id, nl_method(text, dialect, max_steps)),
    )
    .await
}

fn refusal(response: &Response) -> String {
    format!("{:?} {:?}", response.error, response.error_detail)
}

fn raw(response: &Response) -> Vec<u8> {
    match &response.result {
        Some(ResultPayload::Raw(bytes)) => bytes.clone(),
        other => panic!(
            "expected a Raw result, got {other:?} / {}",
            refusal(response)
        ),
    }
}

async fn direct_sparql(state: &test_support::SharedState, id: u64, query: &str) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::Sparql {
                query: query.to_string(),
                base_iri: String::new(),
                type_convention: String::new(),
                explain: false,
                requires_packs: Vec::new(),
            },
        ),
    )
    .await
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.11
#[tokio::test]
async fn sparql_dialect_plans_and_runs_a_select() {
    medulla_h_query::nl::set_nl_planner(Arc::new(EchoPlanner));
    medulla_h_query::nl::set_sparql_nl_planner(Arc::new(EchoSparql));
    let state = state();
    let added = test_support::dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            1,
            Method::AddNode {
                node_id: "d1".to_string(),
                properties_msgpack: test_support::json_bytes(
                    serde_json::json!({"type": "Doc", "year": 2000}),
                ),
            },
        ),
    )
    .await;
    assert!(added.error.is_none(), "{}", refusal(&added));

    // A SELECT that names no ontology predicate (`rdf:type` only) grounds and answers
    // exactly what a direct `Method::Sparql` of the same text does.
    let query = "SELECT ?s WHERE { ?s a ?class }";
    let direct = direct_sparql(&state, 10, query).await;
    assert!(direct.error.is_none(), "{}", refusal(&direct));
    let planned = nl(&state, 11, query, NlDialect::Sparql, None).await;
    assert!(planned.error.is_none(), "{}", refusal(&planned));
    assert_eq!(raw(&planned), raw(&direct));

    // Every update form is refused at once and never runs.
    for (id, update) in [
        (
            12u64,
            "INSERT DATA { <http://x/s> <http://x/p> <http://x/o> }",
        ),
        (13, "DELETE WHERE { ?s ?p ?o }"),
        (14, "CLEAR ALL"),
        (15, "DROP GRAPH <http://x/g>"),
    ] {
        let refused = nl(&state, id, update, NlDialect::Sparql, Some(4)).await;
        let text = refusal(&refused);
        assert!(text.contains("NL_MUTATION_REFUSED"), "{text}");
        assert!(refused.result.is_none(), "{text}");
    }
    let after = direct_sparql(&state, 16, query).await;
    assert_eq!(
        raw(&after),
        raw(&direct),
        "a refused update changes nothing"
    );

    // A predicate outside the composed ontology never runs: the budget spends, then refuses.
    let unknown = "SELECT ?s WHERE { ?s <http://example.test/ns#invented> ?o }";
    let refused = nl(&state, 17, unknown, NlDialect::Sparql, Some(2)).await;
    let text = refusal(&refused);
    assert!(text.contains("NL_QUERY_STEPS_EXHAUSTED"), "{text}");
    assert!(text.contains("step 2"), "{text}");
    assert!(refused.result.is_none(), "{text}");

    // Cross-tenant negative: another tenant's request is refused before any planning and
    // returns no rows.
    let mut foreign = common::context_for(common::TEST_AGENT);
    foreign.tenant = "tenant-foreign".to_string();
    let cross = test_support::dispatch(
        &state,
        common::signed_request_with_context(
            SECRET,
            18,
            GRAPH,
            foreign,
            nl_method(query, NlDialect::Sparql, None),
        ),
    )
    .await;
    assert!(cross.error.is_some(), "a foreign tenant is refused");
    assert!(cross.result.is_none(), "a foreign tenant reads no rows");
}
