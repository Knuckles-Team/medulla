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
            "GraphSchemaGet",
            Method::GraphSchemaGet {
                source_id: "admin:local".to_string(),
            },
        ),
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
    samples.extend(classification_claim_ops().into_iter().map(|(label, op)| {
        (label, Method::ClassificationClaim { op })
    }));
    samples.extend(glossary_ops().into_iter().map(|(label, op)| {
        (label, Method::Glossary { op: Box::new(op) })
    }));
    samples.extend(stewardship_ops().into_iter().map(|(label, op)| {
        (label, Method::Stewardship { op: Box::new(op) })
    }));
    samples.extend(dashboard_ops().into_iter().map(|(label, op)| {
        (label, Method::DashboardAsset { op: Box::new(op) })
    }));
    samples
}

/// One valid `DashboardAsset` op per op (MEDULLA-TYPED-PACKS-R109.3).
fn dashboard_ops() -> Vec<(&'static str, medulla_types::dashboard::DashboardAssetOp)> {
    use medulla_types::dashboard::{
        DashboardAssetOp, DashboardBody, DashboardGrant, DashboardPanel, DashboardRole,
    };
    let body = DashboardBody {
        panels: vec![DashboardPanel {
            id: "p1".to_string(),
            source: "orders".to_string(),
            query: "SELECT 1".to_string(),
            terms: vec![],
            concepts: vec![],
        }],
        layout: serde_json::Value::Null,
        parameters: serde_json::Value::Null,
    };
    vec![
        (
            "DashboardAsset.save",
            DashboardAssetOp::Save {
                id: "dash-1".to_string(),
                body,
                revision: 0,
            },
        ),
        (
            "DashboardAsset.share",
            DashboardAssetOp::Share {
                id: "dash-1".to_string(),
                grants: vec![DashboardGrant {
                    principal_or_group: "group:sales".to_string(),
                    role: DashboardRole::View,
                }],
            },
        ),
        (
            "DashboardAsset.grant",
            DashboardAssetOp::Grant {
                id: "dash-1".to_string(),
                grant: DashboardGrant {
                    principal_or_group: "group:ops".to_string(),
                    role: DashboardRole::Edit,
                },
            },
        ),
        (
            "DashboardAsset.revoke",
            DashboardAssetOp::Revoke {
                id: "dash-1".to_string(),
                principal_or_group: "group:ops".to_string(),
            },
        ),
        (
            "DashboardAsset.delete",
            DashboardAssetOp::Delete {
                id: "dash-1".to_string(),
                expected_revision: Some(1),
            },
        ),
        (
            "DashboardAsset.restore",
            DashboardAssetOp::Restore {
                id: "dash-1".to_string(),
            },
        ),
        (
            "DashboardAsset.get",
            DashboardAssetOp::Get {
                id: "dash-1".to_string(),
            },
        ),
    ]
}

/// One valid `Stewardship` op per op (MEDULLA-TYPED-PACKS-R802.3).
fn stewardship_ops() -> Vec<(
    &'static str,
    medulla_catalog_types::stewardship::StewardshipOp,
)> {
    use medulla_catalog_types::stewardship::{
        StewardRole, StewardshipAssignment, StewardshipOp, StewardshipScope, StewardshipTarget,
    };
    let assignment = StewardshipAssignment {
        scope: StewardshipScope::Asset("col:orders.total".to_string()),
        role: StewardRole::Steward,
        principal: "alice".to_string(),
    };
    let request_id = "change_request:t:col:orders.total:sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string();
    vec![
        (
            "Stewardship.assign",
            StewardshipOp::Assign {
                assignment: assignment.clone(),
            },
        ),
        ("Stewardship.revoke", StewardshipOp::Revoke { assignment }),
        (
            "Stewardship.open_change_request",
            StewardshipOp::OpenChangeRequest {
                target: "col:orders.total".to_string(),
                proposed_digest: format!("sha256:{}", "0".repeat(64)),
                scopes: StewardshipTarget {
                    asset: Some("col:orders.total".to_string()),
                    concept_scheme: None,
                    domain: None,
                },
            },
        ),
        (
            "Stewardship.review",
            StewardshipOp::Review {
                request_id: request_id.clone(),
                approve: true,
            },
        ),
        ("Stewardship.apply", StewardshipOp::Apply { request_id }),
        (
            "Stewardship.certify",
            StewardshipOp::Certify {
                asset: "col:orders.total".to_string(),
                level: "gold".to_string(),
            },
        ),
        (
            "Stewardship.mark_critical",
            StewardshipOp::MarkCritical {
                element: "col:orders.total".to_string(),
            },
        ),
        (
            "Stewardship.set_trust_flag",
            StewardshipOp::SetTrustFlag {
                asset: "col:orders.total".to_string(),
                flag: "warning".to_string(),
                reason: "the column is being backfilled".to_string(),
            },
        ),
    ]
}

/// One valid `Glossary` op per op (MEDULLA-TYPED-PACKS-R801.3).
fn glossary_ops() -> Vec<(
    &'static str,
    medulla_catalog_types::glossary::GlossaryOp,
)> {
    use medulla_catalog_types::glossary::{
        GlossaryOp, GlossaryTerm, GlossaryTermEntry, GlossaryTermStatus, GlossaryTransition,
    };
    let term = GlossaryTerm {
        scheme: "urn:scheme:finance".to_string(),
        pref_label: "Revenue".to_string(),
        alt_labels: vec!["Turnover".to_string()],
        definition: "Income from sales.".to_string(),
        broader: Vec::new(),
        related: Vec::new(),
        status: GlossaryTermStatus::Draft,
        replaced_by: None,
    };
    vec![
        (
            "Glossary.put_scheme",
            GlossaryOp::PutScheme {
                scheme: "urn:scheme:finance".to_string(),
                terms: vec![GlossaryTermEntry {
                    iri: "urn:term:revenue".to_string(),
                    term: term.clone(),
                }],
            },
        ),
        (
            "Glossary.put_term",
            GlossaryOp::PutTerm {
                iri: "urn:term:revenue".to_string(),
                term,
            },
        ),
        (
            "Glossary.transition",
            GlossaryOp::Transition {
                iri: "urn:term:revenue".to_string(),
                event: GlossaryTransition::Submit,
                change_request: None,
            },
        ),
        (
            "Glossary.bind",
            GlossaryOp::Bind {
                term: "urn:term:revenue".to_string(),
                asset: "col:orders.total".to_string(),
            },
        ),
        (
            "Glossary.propose_description",
            GlossaryOp::ProposeDescription {
                proposal: medulla_catalog_types::glossary::DescriptionProposal {
                    target: "col:orders.total".to_string(),
                    text: "Order total in the order currency.".to_string(),
                    inputs: medulla_catalog_types::glossary::DescriptionInputs {
                        samples_digest: Some(format!("sha256:{}", "0".repeat(64))),
                        ..Default::default()
                    },
                    state: medulla_catalog_types::glossary::DescriptionState::Proposed,
                },
            },
        ),
        (
            "Glossary.review_description",
            GlossaryOp::ReviewDescription {
                proposal: "description_proposal:acme:col:orders.total".to_string(),
                approve: true,
            },
        ),
    ]
}

/// One valid `ClassificationClaim` op per op (MEDULLA-TYPED-PACKS-R652.3).
fn classification_claim_ops() -> Vec<(
    &'static str,
    medulla_identity_types::classification::ClassificationClaimOp,
)> {
    use medulla_identity_types::classification::{
        ClaimKind, ClaimProvenance, ClaimState, ClassificationClaim, ClassificationClaimOp,
    };
    use medulla_identity_types::tenancy::Marking;
    let claim = ClassificationClaim {
        asset: "col:orders.card".to_string(),
        categories: vec![Marking::new("payment_card").expect("a valid marking")],
        provenance: ClaimProvenance {
            kind: ClaimKind::Manual,
            detector_id: None,
            sample_size: None,
            hit_ratio: None,
            scanned_at_ms: None,
        },
        state: ClaimState::Proposed,
        evidence: vec!["review:1".to_string()],
    };
    vec![
        (
            "ClassificationClaim.propose",
            ClassificationClaimOp::Propose { claim },
        ),
        (
            "ClassificationClaim.promote",
            ClassificationClaimOp::Promote {
                claim_id: "classification_claim:col:orders.card:payment_card".to_string(),
                evidence: vec!["review:2".to_string()],
            },
        ),
        (
            "ClassificationClaim.reject",
            ClassificationClaimOp::Reject {
                claim_id: "classification_claim:col:orders.card:payment_card".to_string(),
            },
        ),
    ]
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
