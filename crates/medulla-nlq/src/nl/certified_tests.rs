use super::*;
use serde_json::json;

fn term(value: serde_json::Value) -> Option<CertifiedTerm> {
    let serde_json::Value::Object(props) = value else {
        panic!("object")
    };
    CertifiedTerm::from_properties(&props)
}

fn revenue() -> CertifiedTerm {
    term(
        json!({"type": "GlossaryTerm", "name": "net revenue", "definition": "paid minus refunded",
        "status": "certified", "version": "3", "lineage": ["amount_paid", "amount_refunded"]}),
    )
    .expect("certified term")
}

// spec: MEDULLA-FEDERATED-QUERY-R058.6
#[test]
fn only_certified_complete_terms_are_read() {
    assert_eq!(revenue().version, "3");
    assert!(term(
        json!({"type": "GlossaryTerm", "name": "x", "status": "draft",
        "version": "1", "lineage": []})
    )
    .is_none());
    assert!(
        term(json!({"type": "Doc", "name": "x", "status": "certified",
        "version": "1", "lineage": []}))
        .is_none()
    );
    assert!(term(
        json!({"type": "GlossaryTerm", "name": "x", "status": "certified",
        "lineage": []})
    )
    .is_none());
}

// spec: MEDULLA-FEDERATED-QUERY-R058.6
#[test]
fn candidate_without_the_whole_lineage_is_a_mismatch() {
    let terms = [revenue()];
    let bad = check_certified_definitions(
        &terms,
        "what is net revenue per order",
        "MATCH (o:Order) |> FILTER amount_paid > 0",
    );
    assert_eq!(bad.used.len(), 1);
    assert_eq!(bad.mismatches.len(), 1);
    assert_eq!(bad.mismatches[0].missing, vec!["amount_refunded"]);
    let good = check_certified_definitions(
        &terms,
        "what is net revenue per order",
        "MATCH (o:Order) |> FILTER amount_paid > amount_refunded",
    );
    assert!(good.mismatches.is_empty());
    assert_eq!(good.used[0].version, "3");
    let unrelated = check_certified_definitions(&terms, "list orders", "MATCH (o:Order)");
    assert!(unrelated.used.is_empty() && unrelated.mismatches.is_empty());
}

fn record(question: &str, rejected: &str, approved: &str) -> NlCorrection {
    NlCorrection {
        question: question.into(),
        rejected_uql: rejected.into(),
        approved_uql: approved.into(),
        certifier: "certifier-1".into(),
        certified_at_ms: 1_000,
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R058.7
#[test]
fn only_the_same_question_matches_its_corrections() {
    let one = record(
        "Net revenue of orders?",
        "MATCH (:Order)",
        "MATCH (:Order) |> WHERE p IS NOT NULL",
    );
    let corrections = vec![one];
    assert_eq!(
        matching_corrections(&corrections, "orders of net revenue").len(),
        1,
        "word order, case and punctuation do not matter"
    );
    assert!(matching_corrections(&corrections, "gross revenue of orders").is_empty());
    assert!(corrections_hint(&corrections, "gross revenue of orders").is_empty());
    let hint = corrections_hint(&corrections, "net revenue of orders");
    assert!(
        hint.contains("Approved correction (certified by certifier-1)"),
        "{hint}"
    );
}
