//! MEDULLA-FEDERATED-QUERY-R058.4.10: a served `NlQuery` with dialect `sql` (or `auto`
//! choosing `sql`) plans one SQL SELECT, grounds it against the caller's catalog and runs
//! it on the served SQL read path; a write is refused and never runs; a name outside the
//! catalog returns to the planner and, when the steps are spent, refuses.
#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "nl-query",
    feature = "query",
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

const SECRET: &str = "nl-query-sql-served-secret";
const GRAPH: &str = "__commons__";

/// Plans UQL as the question text; only the dialect choice and refusals matter here.
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

/// Answers the question text as the SQL, so each case chooses its candidate.
struct EchoSql;

impl medulla_nlq::nl::sql_planner::SqlNlPlanner for EchoSql {
    fn plan_sql(
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

async fn sql(state: &test_support::SharedState, id: u64, query: &str) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::Sql {
                query: query.to_string(),
                params_msgpack: Vec::new(),
            },
        ),
    )
    .await
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

async fn seeded_state() -> test_support::SharedState {
    medulla_h_query::nl::set_nl_planner(Arc::new(EchoPlanner));
    medulla_h_query::nl::set_sql_nl_planner(Arc::new(EchoSql));
    let state = state();
    for (id, statement) in [
        (1, "CREATE TABLE orders (id INT, total INT)"),
        (2, "INSERT INTO orders (id, total) VALUES (1, 10), (2, 20)"),
    ] {
        let created = sql(&state, id, statement).await;
        assert!(
            created.error.is_none(),
            "{statement}: {}",
            refusal(&created)
        );
    }
    state
}

// spec: MEDULLA-FEDERATED-QUERY-R058.4.10
#[tokio::test]
async fn sql_dialect_plans_and_runs_a_select() {
    let state = seeded_state().await;
    let query = "SELECT id, total FROM orders ORDER BY id";

    // The planned SELECT answers exactly what a direct `Method::Sql` of the same text does.
    let direct = sql(&state, 10, query).await;
    assert!(direct.error.is_none(), "{}", refusal(&direct));
    let planned = nl(&state, 11, query, NlDialect::Sql, None).await;
    assert!(planned.error.is_none(), "{}", refusal(&planned));
    assert_eq!(raw(&planned), raw(&direct));

    // `auto` chooses `sql` when the request links to a catalog table and to no label.
    let auto = nl(&state, 12, query, NlDialect::Auto, None).await;
    assert!(auto.error.is_none(), "{}", refusal(&auto));
    assert_eq!(raw(&auto), raw(&direct));

    // A write is refused at once and never runs: the rows are unchanged.
    for (id, write) in [(13u64, "DELETE FROM orders"), (14, "DROP TABLE orders")] {
        let refused = nl(&state, id, write, NlDialect::Sql, Some(4)).await;
        let text = refusal(&refused);
        assert!(text.contains("NL_MUTATION_REFUSED"), "{text}");
        assert!(refused.result.is_none(), "{text}");
    }
    let after = sql(&state, 15, query).await;
    assert_eq!(raw(&after), raw(&direct), "a refused write changes nothing");

    // A table or column outside the catalog never runs: the budget spends, then refuses.
    for (id, unknown) in [
        (16u64, "SELECT id FROM invoices"),
        (17, "SELECT price FROM orders"),
    ] {
        let refused = nl(&state, id, unknown, NlDialect::Sql, Some(2)).await;
        let text = refusal(&refused);
        assert!(text.contains("NL_QUERY_STEPS_EXHAUSTED"), "{text}");
        assert!(text.contains("step 2"), "{text}");
        assert!(refused.result.is_none(), "{text}");
    }

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
            nl_method(query, NlDialect::Sql, None),
        ),
    )
    .await;
    assert!(cross.error.is_some(), "a foreign tenant is refused");
    assert!(cross.result.is_none(), "a foreign tenant reads no rows");
}
