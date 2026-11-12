//! Stored-query materialization over a changing input (MEDULLA-FEDERATED-QUERY-R708.2).
//!
//! The input is a small table whose generation advances on every write, standing in for the
//! graph snapshot revision a served read reports. `serve_materialized` is the one function the
//! served `StoredQueryCall` handler decides reuse and recompute with.

use std::collections::BTreeMap;

use medulla_identity_types::identity::ShareScope;
use medulla_source_types::stored_query::{
    serve_materialized, RefreshPolicy, Served, StoredParamType, StoredQuery, StoredQueryLanguage,
    StoredResult,
};
use serde_json::{json, Value};

/// A table of (id, status) rows with a generation counter.
struct Input {
    rows: Vec<(String, String)>,
    generation: u64,
}

impl Input {
    fn write(&mut self, id: &str, status: &str) {
        self.rows.push((id.to_string(), status.to_string()));
        self.generation += 1;
    }

    /// The stored query: rows whose status equals the bound `status`.
    fn run(&self, status: &str) -> (Vec<String>, Value, u64) {
        let rows: Vec<Value> = self
            .rows
            .iter()
            .filter(|(_, s)| s == status)
            .map(|(id, _)| json!({"id": id}))
            .collect();
        (vec![], Value::Array(rows), self.generation)
    }
}

fn query(refresh: RefreshPolicy) -> StoredQuery {
    StoredQuery {
        name: "by_status".into(),
        version: 1,
        language: StoredQueryLanguage::Uql,
        text: "MATCH (i:Incident) WHERE i.status = $status RETURN i".into(),
        params: BTreeMap::from([("status".to_string(), StoredParamType::String)]),
        materialize: true,
        refresh,
        author: "usr:alice".into(),
        share: ShareScope::Private,
    }
}

fn read(
    input: &Input,
    stored: &mut Option<StoredResult>,
    policy: RefreshPolicy,
    now: u64,
) -> (u64, Served) {
    let (result, served) =
        serve_materialized(policy, stored.take(), now, || Ok(input.run("open"))).unwrap();
    let count = result.row_count;
    *stored = Some(result);
    (count, served)
}

// spec: MEDULLA-FEDERATED-QUERY-R708.2
#[test]
fn on_change_refreshes() {
    let policy = query(RefreshPolicy::OnChange).refresh;
    let mut input = Input {
        rows: vec![("a".into(), "open".into())],
        generation: 1,
    };
    let mut stored = None;
    assert_eq!(
        read(&input, &mut stored, policy, 10),
        (1, Served::Recomputed)
    );
    // Nothing changed: the stored record is reused and keeps its refresh time.
    assert_eq!(read(&input, &mut stored, policy, 20), (1, Served::Reused));
    assert_eq!(stored.as_ref().unwrap().refreshed_at_ms, 10);
    // A write to an unrelated status advances the generation: the rows are recomputed and
    // still equal, and the record names the new generation.
    input.write("b", "closed");
    assert_eq!(
        read(&input, &mut stored, policy, 30),
        (1, Served::Recomputed)
    );
    // A write that changes the answer is read back at once.
    input.write("c", "open");
    assert_eq!(
        read(&input, &mut stored, policy, 40),
        (2, Served::Recomputed)
    );
    let stored = stored.unwrap();
    assert_eq!(stored.input_generation, input.generation);
    assert_eq!(stored.rows, json!([{"id": "a"}, {"id": "c"}]));
}

// spec: MEDULLA-FEDERATED-QUERY-R708.2
#[test]
fn period_refreshes_on_the_first_read_after_it_elapses() {
    let policy = RefreshPolicy::Period { seconds: 60 };
    let mut input = Input {
        rows: vec![("a".into(), "open".into())],
        generation: 1,
    };
    let mut stored = None;
    assert_eq!(
        read(&input, &mut stored, policy, 0),
        (1, Served::Recomputed)
    );
    input.write("b", "open");
    // Inside the period the input change is not visible.
    assert_eq!(
        read(&input, &mut stored, policy, 59_999),
        (1, Served::Reused)
    );
    assert_eq!(
        read(&input, &mut stored, policy, 60_000),
        (2, Served::Recomputed)
    );
}
