//! One valid sample per method, op and enum variant the 2.27.x contract wave
//! adds.
//!
//! These exist so the round-trip, strictness and dispatch-refusal tests all
//! work from the SAME values. A second, hand-written copy per test is how a
//! sample drifts from the shape it is supposed to pin, and how a test starts
//! proving something about a value nothing else ever sends.

use crate::protocol::Method;
use medulla_agent_types::agent_component::AgentComponentOp;

/// One valid `Method` per new variant and per op, labelled by the surface the
/// contract-wave stub refuses it under.
///
/// The label is exactly what `contract_wave::not_yet_served` names, so a test
/// can assert the refusal text without restating it.
pub fn contract_wave_samples() -> Vec<(&'static str, Method)> {
    let mut samples = vec![
        (
            "AgentAssemble",
            Method::AgentAssemble {
                request: Box::new(
                    medulla_decision_types::decision_fixtures::contract_decision::assembly_request(
                    ),
                ),
            },
        ),
        (
            "DecisionCommit",
            Method::DecisionCommit {
                request: Box::new(
                    medulla_decision_types::decision_fixtures::contract_decision::commit_request(),
                ),
            },
        ),
        (
            "Decide",
            Method::Decide {
                request: Box::new(
                    medulla_decision_types::decision_fixtures::contract_statistical::decide_request(
                    ),
                ),
            },
        ),
        (
            "Solve",
            Method::Solve {
                request: Box::new(
                    medulla_decision_types::decision_fixtures::contract_solver::request(),
                ),
            },
        ),
        ("GraphSchemaList", Method::GraphSchemaList),
        (
            "EdgeIndex.status",
            Method::EdgeIndex {
                op: Box::new(medulla_index_types::managed_index::EdgeIndexOp::Status),
            },
        ),
        (
            "EdgeSearch",
            Method::EdgeSearch {
                request: Box::new(medulla_index_types::managed_index::EdgeSearchRequest {
                    index: "rel_text".to_string(),
                    purpose: "retrieval".to_string(),
                    query: medulla_index_types::managed_index::EdgeSearchQuery::Text {
                        text: "graph".to_string(),
                    },
                    k: 5,
                    edge_type: None,
                    property_equals: Default::default(),
                }),
            },
        ),
        (
            "AgentComponent.content",
            Method::AgentComponent {
                op: AgentComponentOp::Content {
                    request:
                        medulla_catalog_types::catalog_fixtures::contract_control::component_content_request(
                        ),
                },
            },
        ),
    ];
    samples.extend(
        medulla_decision_types::decision_fixtures::contract_statistical::fit_ops()
            .into_iter()
            .map(|(label, op)| (label, Method::DecisionFit { op: Box::new(op) })),
    );
    samples.extend(
        medulla_decision_types::decision_fixtures::contract_statistical::eval_ops()
            .into_iter()
            .map(|(label, op)| (label, Method::DecisionEval { op: Box::new(op) })),
    );
    samples.extend(
        medulla_decision_types::decision_fixtures::contract_statistical::log_ops()
            .into_iter()
            .map(|(label, op)| (label, Method::DecisionLog { op: Box::new(op) })),
    );
    samples.extend(
        medulla_catalog_types::catalog_fixtures::contract_pack::ops()
            .into_iter()
            .map(|(label, op)| (label, Method::ConnectorPack { op: Box::new(op) })),
    );
    samples.extend(
        medulla_catalog_types::catalog_fixtures::contract_control::graph_schema_ops()
            .into_iter()
            .map(|(label, op)| (label, Method::GraphSchema { op: Box::new(op) })),
    );
    samples.extend(
        medulla_catalog_types::catalog_fixtures::contract_control::outbox_ops()
            .into_iter()
            .map(|(label, op)| (label, Method::MutationOutbox { op: Box::new(op) })),
    );
    samples
}

/// One sample per new METHOD (not per op), for tests that only need to reach
/// each dispatch arm once.
pub fn one_sample_per_method() -> Vec<(&'static str, Method)> {
    let mut seen: Vec<&'static str> = Vec::new();
    contract_wave_samples()
        .into_iter()
        .filter(|(label, _)| {
            let method = label.split('.').next().unwrap_or(label);
            if seen.contains(&method) {
                return false;
            }
            seen.push(method);
            true
        })
        .collect()
}

/// The sample labelled `label`, for a stub test that pins exactly its own
/// surface.
pub fn sample_for(label: &str) -> Method {
    contract_wave_samples()
        .into_iter()
        .find(|(candidate, _)| *candidate == label)
        .map(|(_, method)| method)
        .unwrap_or_else(|| panic!("no contract-wave sample is labelled {label}"))
}
