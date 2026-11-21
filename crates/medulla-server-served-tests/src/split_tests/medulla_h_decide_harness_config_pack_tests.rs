//! MEDULLA-TYPED-PACKS-R045.9: a two-tool connector pack, imported through the
//! served `ConnectorPack` import, yields a budget-respecting tool subset from
//! the served `Decide` `HarnessConfig` question.
//!
//! The pack import and the decision handler live in sibling handler crates, so
//! the proof is hosted here where both are linked. The declared tool sizes are
//! read back from the imported components, not typed in again.

use medulla_agent_types::agent_component::AgentComponentFacts;
use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};
use medulla_decision_types::decision::statistical::harness_config::HarnessPart;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, DecisionBatch, StatisticalDecisionRecord,
};
use medulla_h_decide::decide::stat_test_support::{decode, Harness};
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::{Method, Request};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::BoundedVec;

use super::medulla_h_decide_capability_pack_feed_tests::{
    build_pack_with, Served, CONNECTOR, TENANT,
};
use super::medulla_h_decide_harness_config_tests::{answer, seed_config, Cfg};

/// The System-role agent the pack harness isolation registers.
const ADMIN: &str = "pack-feed-served-admin";
const CAPABILITY: &str = "eg:cap/x";

fn tool_component(name: &str) -> String {
    format!("mcp:{CONNECTOR}/tool/{name}")
}

/// The question of `seed_config`, with its candidates replaced by the imported
/// tools `names`, each declared with the size its stored facts carry.
fn imported_candidates(h: &Harness, mut asked: DecideRequest, names: &[&str]) -> DecideRequest {
    let options: Vec<DeclaredOption> = names
        .iter()
        .map(|name| {
            let id = tool_component(name);
            let entry = h
                .store
                .current_component(TENANT, &id)
                .unwrap()
                .unwrap_or_else(|| panic!("the import left no component {id}"));
            let AgentComponentFacts::Tool { schema_tokens, .. } = entry.facts else {
                panic!("{id} is not a tool component");
            };
            DeclaredOption {
                option_id: id,
                classification: BoundedVec::new(vec![CAPABILITY.to_string()]).unwrap(),
                numbers: BoundedVec::new(
                    schema_tokens
                        .map(|tokens| DeclaredNumber {
                            key: "schema_tokens".to_string(),
                            q32: i64::from(tokens),
                        })
                        .into_iter()
                        .collect(),
                )
                .unwrap(),
                texts: BoundedVec::default(),
            }
        })
        .collect();
    asked.candidates = CandidateSource::Declared {
        options: BoundedVec::new(options).unwrap(),
    };
    asked
}

async fn decide(h: &Harness, asked: DecideRequest) -> StatisticalDecisionRecord {
    let method = Method::Decide {
        request: Box::new(asked),
    };
    let method = rmp_serde::from_slice(&rmp_serde::to_vec_named(&method).unwrap()).unwrap();
    let response = Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 45,
                graph: "__commons__".into(),
                auth_token: String::new(),
                agent_id: Some(ADMIN.into()),
                method,
            },
            VerifiedRequestContext::verified_for_test_with_scopes(
                ADMIN,
                TENANT,
                &["kg:read", "agent:decision-write"],
            ),
        ),
    )
    .await;
    let batch: DecisionBatch = decode(response).expect("a served harness_config decision");
    batch.records.as_slice()[0].clone()
}

// spec: MEDULLA-TYPED-PACKS-R045.9
#[tokio::test]
async fn two_tool_pack_subset_respects_budget() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let served = Served::new();
    served.declare_generation().await;
    served.bind().await;
    // Tool a is 400 tokens, tool b is 900, tool c declares no size.
    served
        .import(build_pack_with(&[
            ("a", Some(400)),
            ("b", Some(900)),
            ("c", None),
        ]))
        .await;
    let Served { _dir, state } = served;
    let store = state.write().await.ensure_agent_library().unwrap();
    let h = Harness {
        _dir,
        state,
        store,
        nonce: std::cell::Cell::new(1),
    };
    let cfg = Cfg {
        required: vec![CAPABILITY],
        tools: Vec::new(),
        budget: 500,
        ..Cfg::default()
    };
    let asked = seed_config(&h, &cfg).await;

    // The two sized tools: only the 400-token tool fits the 500-token budget.
    let two = imported_candidates(&h, asked.clone(), &["a", "b"]);
    let record = decide(&h, two).await;
    match answer(&record).tool_subset {
        HarnessPart::Decided { value } => {
            assert_eq!(value.as_slice(), [tool_component("a")]);
        }
        other => panic!("two sized tools must decide the subset, got {other:?}"),
    }

    // A third tool without a size abstains the subset instead of guessing.
    let three = imported_candidates(&h, asked, &["a", "b", "c"]);
    let record = decide(&h, three).await;
    match answer(&record).tool_subset {
        HarnessPart::Abstained {
            missing_fact,
            conservative_default,
        } => {
            assert_eq!(missing_fact.as_slice(), ["schemaTokens".to_string()]);
            assert!(conservative_default.is_empty());
        }
        other => panic!("a tool of unknown size must abstain the subset, got {other:?}"),
    }
}
