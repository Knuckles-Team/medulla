//! Served proof of the owner digest (MEDULLA-BUSINESS-OPS-R003.1).
//!
//! The digest definitions of `business_digest::digest_queries()` are put with
//! `Method::StoredQueryPut` and called with `Method::StoredQueryCall` through the real
//! signed dispatch. The row id of each answer is the supporting node.

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_source_types::business_digest::{
    digest_queries, DIGEST_OVERDUE, DIGEST_PENDING_APPROVALS,
};
use medulla_source_types::stored_query::StoredCall;

use crate::tests::{assert_ok, dispatch_on_heap, request, test_state};

const GRAPH: &str = "__commons__";

fn node(id: &str, properties: serde_json::Value) -> Method {
    Method::AddNode {
        node_id: id.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&properties).unwrap(),
    }
}

fn edge(source: &str, target: &str, label: &str) -> Method {
    Method::AddEdge {
        source_id: source.to_string(),
        target_id: target.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({ "relationship": label }))
            .unwrap(),
    }
}

/// The row ids of one stored-query answer, in answer order.
fn row_ids(response: &Response) -> Vec<String> {
    assert_ok(response);
    let Some(ResultPayload::Json(value)) = response.result.clone() else {
        panic!("a stored query call answers with a JSON result");
    };
    let call: StoredCall = serde_json::from_value(value).expect("a stored call result");
    call.rows
        .as_array()
        .expect("rows are an array")
        .iter()
        .map(|row| row["id"].as_str().expect("a row id").to_string())
        .collect()
}

// spec: MEDULLA-BUSINESS-OPS-R003.1
#[tokio::test]
async fn digest_pending_and_overdue_with_supporting_nodes() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state();
    let mut next_id = 7000_u64;
    let mut send = |method: Method| {
        next_id += 1;
        request(next_id, GRAPH, None, method)
    };

    // One person holds the approver role through a role assignment; a second role is
    // held by nobody the digest asks about.
    for (id, properties) in [
        ("person:ada", serde_json::json!({"type": "Person"})),
        ("role:approver", serde_json::json!({"type": "Role"})),
        ("role:auditor", serde_json::json!({"type": "Role"})),
        ("ra:ada", serde_json::json!({"type": "RoleAssignment"})),
    ] {
        assert_ok(&dispatch_on_heap(&state, send(node(id, properties))).await);
    }
    for (source, target, label) in [
        ("ra:ada", "person:ada", "assigneePerson"),
        ("ra:ada", "role:approver", "assignedRole"),
    ] {
        assert_ok(&dispatch_on_heap(&state, send(edge(source, target, label))).await);
    }
    // Two pending requests for the approver role, one already approved, one pending for a
    // role the person does not hold.
    for (id, status, amount, role) in [
        ("req:small", "pending", 100, "role:approver"),
        ("req:big", "pending", 500, "role:approver"),
        ("req:done", "approved", 900, "role:approver"),
        ("req:audit", "pending", 700, "role:auditor"),
    ] {
        let properties = serde_json::json!({
            "type": "ApprovalRequest",
            "requestStatus": status,
            "requestActionKind": "payment",
            "requestAmountMinor": amount,
            "valid_from": 1_700_000_000_i64,
        });
        assert_ok(&dispatch_on_heap(&state, send(node(id, properties))).await);
        assert_ok(
            &dispatch_on_heap(&state, send(edge(id, role, "requestRequiredRole"))).await,
        );
    }
    // Four obligation instances: two open or overdue before the as-of date, one
    // satisfied, one due after it.
    for (id, status, due) in [
        ("ob:jan", "open", "2026-01-05"),
        ("ob:feb", "overdue", "2026-02-01"),
        ("ob:done", "satisfied", "2026-01-10"),
        ("ob:later", "open", "2026-12-31"),
    ] {
        let properties = serde_json::json!({
            "type": "ObligationInstance",
            "obligationStatus": status,
            "instanceDueDate": due,
        });
        assert_ok(&dispatch_on_heap(&state, send(node(id, properties))).await);
    }

    for query in digest_queries() {
        assert_ok(&dispatch_on_heap(&state, send(Method::StoredQueryPut { query })).await);
    }

    // Pending approvals: the supporting request nodes, largest amount first.
    let pending = dispatch_on_heap(
        &state,
        send(Method::StoredQueryCall {
            name: DIGEST_PENDING_APPROVALS.to_string(),
            version: None,
            args: [("person".to_string(), serde_json::json!("person:ada"))].into(),
        }),
    )
    .await;
    assert_eq!(row_ids(&pending), vec!["req:big", "req:small"]);

    // Overdue: open or overdue obligations due before the as-of date, by due date.
    let overdue = dispatch_on_heap(
        &state,
        send(Method::StoredQueryCall {
            name: DIGEST_OVERDUE.to_string(),
            version: None,
            args: [("as_of_date".to_string(), serde_json::json!("2026-03-01"))].into(),
        }),
    )
    .await;
    assert_eq!(row_ids(&overdue), vec!["ob:jan", "ob:feb"]);

    // A call without a declared parameter refuses by name.
    let missing = dispatch_on_heap(
        &state,
        send(Method::StoredQueryCall {
            name: DIGEST_OVERDUE.to_string(),
            version: None,
            args: Default::default(),
        }),
    )
    .await;
    let refusal = missing.refusal_text().unwrap_or_default();
    assert!(
        refusal.contains("STORED_QUERY_PARAM_MISSING"),
        "got: {refusal}"
    );
}

/// The canonical `:ruleParameters` of the sales-tax recurrence rule.
#[cfg(feature = "numeric")]
const SALES_TAX_PARAMETERS: &str = "adjustment=next_business_day\nanchor=2026-01-20\nday=20\n\
                                    frequency=monthly\nzone=America/Chicago";

/// A `Method::Uql` statement without parameters.
#[cfg(feature = "numeric")]
fn uql(text: &str) -> Method {
    Method::Uql {
        text: text.to_string(),
        params: Default::default(),
        prepare: false,
        foreign_executions: Default::default(),
    }
}

/// The single row of a successful `Method::Uql` answer: its id and its text cells.
#[cfg(feature = "numeric")]
fn single_row(response: &Response) -> (String, std::collections::BTreeMap<String, String>) {
    use medulla_query_types::wire::UqlResult;

    assert_ok(response);
    let Some(ResultPayload::Raw(bytes)) = response.result.as_ref() else {
        panic!("a UQL statement answers with a raw result");
    };
    let UqlResult::Rows { mut rows, .. } = rmp_serde::from_slice(bytes).expect("a UQL result")
    else {
        panic!("the statement answers rows");
    };
    assert_eq!(rows.len(), 1, "one row per evaluation");
    let row = rows.remove(0);
    (row.id, row.provenance)
}

// spec: MEDULLA-BUSINESS-OPS-R002.7
#[cfg(feature = "numeric")]
#[tokio::test]
async fn evaluate_rule_stage_serves_recurrence_with_inputs() {
    use medulla_numeric::business::rule_spec;

    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state();
    let mut next_id = 7500_u64;
    let mut send = |method: Method| {
        next_id += 1;
        request(next_id, GRAPH, None, method)
    };

    // The rule record and its holiday calendar, written by one batch update.
    let operations = serde_json::json!([
        {"op": "upsert_node", "id": "rule:sales-tax", "properties": {
            "type": "RecurrenceRule",
            "ruleKind": "recurrence",
            "ruleVersion": 1,
            "ruleParameters": SALES_TAX_PARAMETERS,
        }},
        {"op": "upsert_node", "id": "cal:1", "properties": {
            "type": "HolidayCalendar",
            "holidayDate": ["2026-02-20"],
        }},
    ]);
    assert_ok(
        &dispatch_on_heap(
            &state,
            send(Method::BatchUpdate {
                operations_msgpack: rmp_serde::to_vec_named(&operations).unwrap(),
            }),
        )
        .await,
    );

    let stage = "MATCH (:HolidayCalendar) WHERE id = 'cal:1' |> EVALUATE RULE 'rule:sales-tax'";
    let answered =
        dispatch_on_heap(&state, send(uql(&format!("{stage} ASOF 1767225600000 COUNT 2")))).await;
    let (id, cells) = single_row(&answered);
    assert_eq!(id, "rule:sales-tax");
    assert_eq!(cells["kind"], "recurrence");

    // The digest is the one `rule_spec::load` computes over the same record.
    let parameters = rule_spec::parse_parameters(SALES_TAX_PARAMETERS).unwrap();
    let spec = rule_spec::load("recurrence", 1, parameters).unwrap();
    assert_eq!(cells["rule_digest"], spec.digest);

    // The input is the calendar node with the revision that was read.
    let revision = cells["inputs"]
        .strip_prefix("cal:1@")
        .expect("the one input is the calendar node");
    assert!(revision.bytes().all(|byte| byte.is_ascii_digit()), "{revision}");

    // The two due dates after the as-of day; the second falls on a holiday and moves to
    // the next business day.
    let result: serde_json::Value = serde_json::from_str(&cells["result"]).unwrap();
    assert_eq!(
        result["dates"],
        serde_json::json!([
            {"scheduled": "2026-01-20", "due": "2026-01-20"},
            {"scheduled": "2026-02-20", "due": "2026-02-23"},
        ])
    );
    assert_eq!(result["zone"], "America/Chicago");

    // No ASOF: the stage never reads a clock, so it refuses by code.
    let no_as_of = dispatch_on_heap(&state, send(uql(&format!("{stage} COUNT 2")))).await;
    let refusal = no_as_of.refusal_text().unwrap_or_default();
    assert!(refusal.contains("ASOF_REQUIRED"), "got: {refusal}");

    // An unknown rule id refuses by code.
    let unknown = dispatch_on_heap(
        &state,
        send(uql(
            "MATCH (:HolidayCalendar) WHERE id = 'cal:1' |> EVALUATE RULE 'rule:none' ASOF 1767225600000",
        )),
    )
    .await;
    let refusal = unknown.refusal_text().unwrap_or_default();
    assert!(refusal.contains("RULE_NOT_FOUND"), "got: {refusal}");
}
