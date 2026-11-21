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
