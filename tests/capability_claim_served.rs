//! Served heavy-capability claims (MEDULLA-PLACEMENT-R004.3).
//!
//! A host declares its heavy capabilities as a `placement:Host` node of the commons graph. A
//! remote worker registers with its `host_iri`; its claim holds the capabilities that host
//! declares and nothing the worker lists itself.

#![cfg(all(
    feature = "server",
    feature = "security",
    feature = "redb",
    feature = "jobs"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_work_types::jobs::{JobKind, JobOp, SubmitJobSpec};
use medulla_work_types::worker_registry::WorkerRegistration;

const SECRET: &str = "medulla-placement-r004-3-capability-claim-secret";
const COMMONS: &str = "__commons__";
const GRAPH: &str = "capability-claim-graph";

fn host_node(capabilities: &[&str]) -> Vec<u8> {
    rmp_serde::to_vec_named(&serde_json::json!({
        "type": medulla_types::ontology_iri!("placement#Host"),
        medulla_types::ontology_iri!("placement#capabilities"): capabilities,
    }))
    .expect("host node properties")
}

fn registration(worker_id: &str, host_iri: &str) -> WorkerRegistration {
    WorkerRegistration {
        worker_id: worker_id.into(),
        host_iri: host_iri.into(),
        cores: 8,
        memory_bytes: 8 * 1024 * 1024 * 1024,
        lanes_supported: ["general".to_string()].into(),
    }
}

// spec: MEDULLA-PLACEMENT-R004.3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claim_uses_declared_host_capability() {
    common::configure_authority();
    let dir = test_support::fresh_dir("placement-r004-3-claim");
    let (_backend, state) = test_support::redb_state_at(
        SECRET,
        common::current_isolation(),
        dir.to_str().expect("utf-8 temp dir"),
    );
    let tenant = common::test_tenant();
    let context = VerifiedRequestContext::verified_for_test_in_tenant("agent-a", tenant);
    let authority = CarrierAuthority::from_verified(&context).expect("verified carrier");

    {
        let mut guard = state.write().await;
        for graph in [COMMONS, GRAPH] {
            if guard.registry.get(graph).is_none() {
                guard
                    .registry
                    .create_graph(graph, GraphType::Team, None)
                    .expect("create graph");
            }
        }
        let commons = guard.registry.get(COMMONS).expect("commons graph").core.clone();
        commons.add_node("host:alpha".to_string(), host_node(&["backtest"]));
        commons.add_node("host:beta".to_string(), host_node(&[]));
    }

    for (id, worker, host) in [(1, "worker-alpha", "host:alpha"), (2, "worker-beta", "host:beta")] {
        let registered = medulla_h_jobs::worker_registry::try_handle(
            &state,
            id,
            &context,
            Method::WorkerRegister {
                registration: registration(worker, host),
            },
        )
        .await
        .expect("worker registry method");
        assert!(registered.error.is_none(), "{:?}", registered.error);
    }

    let submitted = medulla_h_jobs::jobs::handle(
        &state,
        3,
        &authority,
        None,
        false,
        &context,
        JobOp::Submit(SubmitJobSpec {
            graph: GRAPH.into(),
            tenant: String::new(),
            actor: String::new(),
            purpose: "capability claim".into(),
            priority: 0,
            deadline_unix_ms: None,
            quota_cpu_ms: None,
            memory_bytes: None,
            io_bytes: None,
            output_bytes: None,
            worker_pool: String::new(),
            worker_region: String::new(),
            required_capabilities: vec!["backtest".to_string()],
            max_attempts: 1,
            backoff_ms: 0,
            kind: JobKind::MineAssociate {
                transactions: vec![vec!["a".to_string(), "b".to_string()]],
                min_support: 0.1,
                min_confidence: 0.5,
                algorithm: "fpgrowth".to_string(),
            },
        }),
    )
    .await;
    assert!(submitted.error.is_none(), "{:?}", submitted.error);

    let claim = |id: u64, worker: &str, capabilities: Vec<String>| {
        let state = state.clone();
        let context = context.clone();
        let authority = authority.clone();
        let worker = worker.to_string();
        async move {
            medulla_h_jobs::jobs::handle(
                &state,
                id,
                &authority,
                None,
                true,
                &context,
                JobOp::WorkerClaim {
                    worker_instance: worker,
                    capabilities,
                    lease_ms: 30_000,
                },
            )
            .await
        }
    };
    let claimed = |response: &medulla_protocol_types::protocol::Response| match &response.result {
        Some(ResultPayload::Raw(bytes)) => !matches!(
            rmp_serde::from_slice::<Option<serde_json::Value>>(bytes),
            Ok(None)
        ),
        _ => false,
    };

    // The worker on the host that declares nothing is not offered the job.
    let beta = claim(4, "worker-beta", Vec::new()).await;
    assert!(beta.error.is_none(), "{:?}", beta.error);
    assert!(!claimed(&beta), "a host without the declaration claims nothing");

    // A claim that lists a heavy capability itself is refused.
    let lying = claim(5, "worker-beta", vec!["backtest".to_string()]).await;
    assert!(
        lying
            .refusal_text()
            .is_some_and(|text| text.starts_with("INVALID_ARGUMENT")),
        "{:?}",
        lying.error
    );

    // The worker on the declaring host claims it.
    let alpha = claim(6, "worker-alpha", Vec::new()).await;
    assert!(alpha.error.is_none(), "{:?}", alpha.error);
    assert!(claimed(&alpha), "the declared host claims the job");
}
