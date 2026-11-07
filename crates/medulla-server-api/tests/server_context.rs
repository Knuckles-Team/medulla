//! Process state is explicit: each `ServerContext` owns its request policy and
//! its replay ledger, so two contexts in one process share neither
//! (MEDULLA-SDLC-R023).
#![cfg(feature = "security")]

use medulla_server_api::authority_context::RequestContextPolicy;
use medulla_server_api::request_replay::PresentedNonce;
use medulla_server_api::server_context::ServerContext;

fn policy(tenant: &str) -> RequestContextPolicy {
    RequestContextPolicy {
        expected_audience: "medulla-test".to_string(),
        expected_tenant: tenant.to_string(),
        expected_policy_version: "policy-test".to_string(),
    }
}

fn presented(nonce: &str, now: u64) -> PresentedNonce<'_> {
    PresentedNonce {
        nonce,
        timestamp: now,
        now,
        window: 300,
    }
}

// spec: MEDULLA-SDLC-R023
#[test]
fn two_contexts_in_one_process_do_not_share_replay_state() {
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = ServerContext::with_durable_replay(Ok(policy("tenant-a")), dir_a.path());
    let b = ServerContext::with_durable_replay(Ok(policy("tenant-b")), dir_b.path());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // The durable ledger opens on first use, under its own context's directory.
    assert!(!dir_a.path().join("request-replay.redb").exists());
    let ledger_a = a.replay_ledger().unwrap();
    assert!(dir_a.path().join("request-replay.redb").exists());
    assert!(!dir_b.path().join("request-replay.redb").exists());

    assert!(ledger_a.check_and_record(presented("n-1", now)).unwrap());
    assert!(
        !ledger_a.check_and_record(presented("n-1", now)).unwrap(),
        "a replay in A"
    );
    assert!(
        b.replay_ledger()
            .unwrap()
            .check_and_record(presented("n-1", now))
            .unwrap(),
        "B has its own window"
    );

    // Each context answers with its own policy; a missing policy is a refusal.
    assert_eq!(a.request_policy().unwrap().expected_tenant, "tenant-a");
    assert_eq!(b.request_policy().unwrap().expected_tenant, "tenant-b");
    let refused = ServerContext::with_durable_replay(Err("no policy".to_string()), dir_b.path());
    assert_eq!(refused.request_policy().unwrap_err(), "no policy");

    // Sealing hands off A's window only; an unopened ledger hands off nothing.
    assert_eq!(a.seal_replay_ledger().unwrap(), 1);
    assert_eq!(refused.seal_replay_ledger().unwrap(), 0);
}

// spec: MEDULLA-SDLC-R023
#[test]
fn two_contexts_in_one_process_do_not_share_denial_samples() {
    use medulla_isolation::isolation::IsolationLayer;
    let (a, b) = (IsolationLayer::new(), IsolationLayer::new());
    a.offer_denial("agent-a", "graph:read", "SCOPE_DENIED");
    // A policy clone of one layer shares its sampler; a layer built apart does not.
    let (samples, dropped) = a.clone().drain_denials();
    assert_eq!((samples.len(), dropped), (1, 0));
    assert_eq!(samples[0].principal, "agent-a");
    assert_eq!(b.drain_denials().0.len(), 0, "B saw A's denial");
    assert_eq!(
        a.drain_denials().0.len(),
        0,
        "the drain emptied the shared sampler"
    );
}
