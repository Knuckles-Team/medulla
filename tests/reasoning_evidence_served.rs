//! Evidence of the served reasoning methods (MEDULLA-FEDERATED-QUERY-R608.5).
//!
//! `RunRules` and `CausalVerify` each close one span per request, named
//! `medulla.reasoning.rules` and `medulla.reasoning.causal_verify`, carrying the counts the
//! response itself returns. Every request goes through the real dispatch entry.

mod common;
#[path = "common/span_capture.rs"]
mod span_capture;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, StructuralEquationWire};

const SECRET: &str = "reasoning-evidence-served-secret";
const GRAPH: &str = "__commons__";
const CHAIN: usize = 6;

fn chain_ttl() -> String {
    let mut ttl = String::from("@prefix ex: <http://ex/> .\n");
    for i in 0..CHAIN {
        ttl.push_str(&format!("ex:n{i} ex:parent ex:n{} .\n", i + 1));
    }
    ttl
}

fn run_rules(max_steps: Option<u64>) -> Method {
    Method::RunRules {
        ontology_ttl: chain_ttl(),
        rules: vec![
            "base: parent(?x,?y) -> ancestor(?x,?y)".into(),
            "step: ancestor(?x,?y) ^ parent(?y,?z) -> ancestor(?x,?z)".into(),
        ],
        query_predicate: Some("ancestor".into()),
        min_confidence: 0.0,
        derived_only: true,
        explain: false,
        max_steps,
    }
}

fn state() -> test_support::SharedState {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

/// Verifies MEDULLA-FEDERATED-QUERY-R608.5: the `RunRules` span carries the response's
/// own `steps`, `rules_fired` and `complete`, for a converged and a bounded run.
// spec: MEDULLA-FEDERATED-QUERY-R608.5
#[tokio::test]
async fn rules_span_carries_the_response_counters() {
    span_capture::install();
    let state = state();
    for (id, bound) in [(1_u64, None), (2, Some(2_u64))] {
        let response = test_support::call_ok(&state, SECRET, id, GRAPH, run_rules(bound)).await;
        let report: medulla_query_types::rdf_report::RuleReasonResponse = match response.result {
            Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) => {
                rmp_serde::from_slice(&bytes).expect("typed rule response")
            }
            Some(medulla_protocol_types::protocol::ResultPayload::Json(value)) => {
                serde_json::from_value(value).expect("typed rule response")
            }
            other => panic!("unexpected payload {other:?}"),
        };
        let matching = span_capture::spans_with_field(
            "medulla.reasoning.rules",
            "steps",
            &report.steps.to_string(),
        );
        let span = matching
            .iter()
            .find(|s| s.fields.get("complete") == Some(&report.complete.to_string()))
            .unwrap_or_else(|| panic!("no rules span for steps={}", report.steps));
        assert_eq!(
            span.fields.get("rules_fired"),
            Some(&report.rules_fired.to_string())
        );
    }
}

/// Verifies MEDULLA-FEDERATED-QUERY-R608.5: the `CausalVerify` span counts the model's
/// variables, the observations and the variables over the limit.
// spec: MEDULLA-FEDERATED-QUERY-R608.5
#[tokio::test]
async fn causal_verify_span_counts_variables_and_observations() {
    span_capture::install();
    let state = state();
    let variables = vec![
        StructuralEquationWire {
            id: "z".into(),
            parents: vec![],
            bias: 0.0,
            noise_var: 1.0,
        },
        StructuralEquationWire {
            id: "x".into(),
            parents: vec![("z".into(), 2.0)],
            bias: 1.0,
            noise_var: 0.01,
        },
    ];
    // 13 rows: the span's observation count is unique to this test.
    let observations: Vec<std::collections::BTreeMap<String, f64>> = (0..13)
        .map(|i| {
            let z = f64::from(i);
            let noise = if i % 2 == 0 { 0.1 } else { -0.1 };
            [
                ("z".to_string(), z),
                ("x".to_string(), 1.0 + 2.0 * z + noise),
            ]
            .into()
        })
        .collect();
    test_support::call_ok(
        &state,
        SECRET,
        1,
        GRAPH,
        Method::CausalVerify {
            variables,
            observations,
        },
    )
    .await;
    let spans =
        span_capture::spans_with_field("medulla.reasoning.causal_verify", "observations", "13");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].fields.get("variables"), Some(&"2".to_string()));
    // The root variable `z` has no parent to predict it, so it is over the limit; the
    // equation of `x` fits its rows.
    assert_eq!(spans[0].fields.get("over_limit"), Some(&"1".to_string()));
}
