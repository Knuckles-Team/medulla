//! L2 — tag-based column masking + row filters: the ONE plan rewrite — all four
//! mask methods, one exempt principal and one row filter
//! (MEDULLA-FEDERATED-QUERY-R706.2) — plus the served-language projection
//! adapters (MEDULLA-FEDERATED-QUERY-R706.3).

use medulla_exec::algebra::{Op, Plan, Pred};
use medulla_exec::policy_mask::{
    apply_policies, mask_expr, projection_plan, sparql_read_rewrite, sql_read_rewrite,
    AppliedPolicy, MaskMethod, MaskPolicy, PolicyKind, PolicySet, PrincipalRef, RowFilterPolicy,
};

/// A canonical plan: source, a tenant filter, then the projection under test.
fn plan_with_projection(channels: &[&str]) -> Plan {
    Plan::new(vec![
        Op::Scan {
            label: "Person".into(),
        },
        Op::Filter {
            preds: vec![Pred::Eq {
                prop: "tenant".into(),
                value: "acme".into(),
            }],
        },
        Op::Project {
            channels: channels.iter().map(|c| c.to_string()).collect(),
        },
    ])
}

/// Four mask methods on four tags, one extra tag exempt for `role:dpo`, and one
/// row filter.
fn policy_set() -> PolicySet {
    PolicySet {
        masks: vec![
            MaskPolicy {
                tag: "pii.email".into(),
                principals_exempt: Vec::new(),
                method: MaskMethod::Null,
            },
            MaskPolicy {
                tag: "pii.ssn".into(),
                principals_exempt: Vec::new(),
                method: MaskMethod::Hash,
            },
            MaskPolicy {
                tag: "pii.dob".into(),
                principals_exempt: Vec::new(),
                method: MaskMethod::Partial,
            },
            MaskPolicy {
                tag: "pii.home".into(),
                principals_exempt: Vec::new(),
                method: MaskMethod::Redact,
            },
            MaskPolicy {
                tag: "pii.notes".into(),
                principals_exempt: vec!["role:dpo".into()],
                method: MaskMethod::Redact,
            },
        ],
        row_filters: vec![RowFilterPolicy {
            tag: "region.eu".into(),
            predicate: "region = 'eu'".into(),
        }],
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R706.1
#[test]
fn policies_round_trip() {
    let set = policy_set();
    let json = serde_json::to_string(&set).unwrap();
    assert_eq!(serde_json::from_str::<PolicySet>(&json).unwrap(), set);
}

// spec: MEDULLA-FEDERATED-QUERY-R706.2
#[test]
fn masks_and_filters_rewrite_the_plan() {
    let mut plan = plan_with_projection(&[
        "pii.email",
        "pii.ssn",
        "pii.dob",
        "pii.home",
        "pii.notes",
        "plain",
    ]);
    let set = policy_set();
    let principal = PrincipalRef::from("role:dpo");
    let applied = apply_policies(&mut plan, &set, &principal);

    // The evidence records: one per applied mask + one per row filter — and no
    // masked value rides on a record.
    let masks: Vec<&AppliedPolicy> = applied
        .iter()
        .filter(|a| a.kind == PolicyKind::Mask)
        .collect();
    assert_eq!(
        masks.len(),
        4,
        "the exempt principal's column is not masked"
    );
    assert!(applied
        .iter()
        .any(|a| a.kind == PolicyKind::RowFilter && a.tag == "region.eu"));

    // All four methods land as mask expressions; the exempt column and the
    // untagged column stand raw.
    let channels = match &plan.ops[2] {
        Op::Project { channels } => channels.clone(),
        _ => panic!("projection lost"),
    };
    assert_eq!(
        channels,
        vec![
            "NULL",
            "SHA256(pii.ssn)",
            "MASK_PARTIAL(pii.dob)",
            "'[REDACTED]'",
            "pii.notes",
            "plain",
        ]
    );

    // The row filter AND-joins the existing (tenant) predicate.
    let preds = match &plan.ops[1] {
        Op::Filter { preds } => preds.clone(),
        _ => panic!("filter lost"),
    };
    assert_eq!(preds.len(), 2);
    assert!(matches!(&preds[1], Pred::Eq { prop, value } if prop == "region" && value == "eu"));
}

// spec: MEDULLA-FEDERATED-QUERY-R706.2
#[test]
fn mask_expr_covers_all_four_methods() {
    assert_eq!(mask_expr(MaskMethod::Null, "john@example.com"), "NULL");
    assert_eq!(
        mask_expr(MaskMethod::Redact, "john@example.com"),
        "[REDACTED]"
    );
    assert_eq!(
        mask_expr(MaskMethod::Partial, "password-123"),
        "********-123"
    );
    assert_eq!(mask_expr(MaskMethod::Partial, "abcd"), "****");
    // lowercase SHA-256 hex
    assert_eq!(
        mask_expr(MaskMethod::Hash, "john@example.com"),
        "855f96e983f1f8e8be944692b6f719fd54329826cb62e98015efee8e2e071dd4"
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R706.3
#[test]
fn one_rewrite_serves_every_language() {
    let set = policy_set();
    let principal = PrincipalRef::from("analyst");

    // The projection adapter is the language → shared-Plan seam.
    let adapter = projection_plan(vec!["pii.email".into(), "plain".into()]);
    assert!(matches!(&adapter.ops[0], Op::Project { channels } if channels.len() == 2));

    // The SQL read: the pushed query carries the masked projection (aliases
    // survive) and the row-filter predicate AND-joined into the WHERE.
    let (sql, applied) = sql_read_rewrite(
        r#"SELECT "pii.email", "pii.ssn" AS ssn, name FROM people WHERE year > 2000"#,
        &set,
        &principal,
    )
    .expect("the plain SELECT list is rewritable");
    assert_eq!(
        sql,
        "SELECT NULL, SHA256(pii.ssn) AS ssn, name FROM people WHERE year > 2000 \
         AND (region = 'eu')"
    );
    assert!(applied.iter().any(|a| a.kind == PolicyKind::Mask));
    assert!(applied.iter().any(|a| a.kind == PolicyKind::RowFilter));

    // A no-policy set rewrites nothing — byte-for-byte.
    let (sql, applied) = sql_read_rewrite("SELECT a FROM t", &PolicySet::default(), &principal)
        .expect("a default set never refuses");
    assert_eq!(sql, "SELECT a FROM t");
    assert!(applied.is_empty());

    // The SPARQL read: a row filter splices in as a FILTER…
    let row_filter_set = PolicySet {
        masks: Vec::new(),
        row_filters: vec![RowFilterPolicy {
            tag: "region.eu".into(),
            predicate: "region = 'eu'".into(),
        }],
    };
    let (sparql, applied) =
        sparql_read_rewrite("SELECT ?x ?y WHERE ?s ?p ?o", &row_filter_set, &principal)
            .expect("a row filter is SPARQL-expressible");
    assert_eq!(
        sparql,
        r#"SELECT ?x ?y WHERE FILTER (region = "eu") ?s ?p ?o"#
    );
    assert_eq!(applied.len(), 1);

    // …but a column mask is refused — SPARQL 1.1 cannot express it inline
    // (fail closed, until the value-level boundary lands).
    let err = sparql_read_rewrite("SELECT ?pii ?name WHERE ?s ?p ?o", &set, &principal)
        .expect_err("a masked SPARQL projection is refused");
    assert!(err.contains("POLICY_SPARQL_MASK_UNSUPPORTED"));
}
