//! `Pull` and `Ack` of the memory-event consumer (MEDULLA-DECISION-ENGINE-R073.7).
//!
//! A memory event is one row of a graph's outbox (R073.6). A consumer reads them through the same
//! durable ledger every projection uses: it subscribes under a consumer name, claims leases and
//! acknowledges them; the acknowledgement is the cursor advance. The ledger binds one consumer to
//! one topic and one scope, so the stored consumer name is derived from the verified tenant, the
//! caller's consumer name and the topic. Two consequences follow. A tenant cannot name another
//! tenant's cursor, and a row that another tenant wrote into the same graph outbox is leased only
//! to be acknowledged in this tenant's own cursor and never returned. A topic outside the three of
//! R073.6 is `ACCESS_DENIED`.

use std::sync::Arc;

use medulla_decision_types::decision::knob_families::participant::default_u64;
use medulla_decision_types::memory_events::MEMORY_EVENT_TOPICS;
use medulla_mutation_types::mutation_outbox::{OutboxAcked, OutboxPulled, OutboxTarget};
use medulla_mutation_types::mutation_scope::MutationOutboxLease;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_transaction::OutboxClaimBudget;

/// The format identity of a pull answer.
const PULL_SCHEMA_VERSION: u16 = 1;
/// How long one leased row stays leased.
const LEASE_MS: u64 = 60_000;
/// Rows one claim call may skip because they belong to another tenant, per topic, before the pull
/// returns what it has.
const MAX_FOREIGN_SKIPS_PER_PULL: usize = 1_000;

/// The consumer name the ledger keeps for `consumer` on `topic` under `tenant`.
pub(super) fn ledger_consumer(tenant: &str, consumer: &str, topic: &str) -> String {
    format!("memory-events:{tenant}:{consumer}:{topic}")
}

/// The persisted graph name of a pull or ack target; only a graph carries memory events.
fn graph_of(target: &OutboxTarget) -> Result<String, String> {
    match target {
        OutboxTarget::Graph { graph } => Ok(medulla_kernel::persist::sanitize(graph)),
        OutboxTarget::NativeStore { .. } => {
            Err("OUTBOX_SCOPE_MISMATCH: memory events are rows of a graph outbox".to_string())
        }
    }
}

/// Lease up to `limit` rows of each requested topic that belong to the caller's tenant.
pub(super) async fn pull(
    persistence: &Arc<dyn PersistenceBackend>,
    verified: &VerifiedRequestContext,
    target: &OutboxTarget,
    consumer: &str,
    topics: &[String],
    limit: u32,
    now_ms: u64,
) -> Result<OutboxPulled, String> {
    let graph = graph_of(target)?;
    if let Some(topic) = topics
        .iter()
        .find(|topic| !MEMORY_EVENT_TOPICS.contains(&topic.as_str()))
    {
        return Err(format!(
            "ACCESS_DENIED: {topic:?} is not a memory event topic"
        ));
    }
    let cap = u32::try_from(default_u64("participant.events_pull_max")).unwrap_or(u32::MAX);
    let limit = limit.min(cap);
    let mut leases = Vec::new();
    for topic in topics {
        let ledger = ledger_consumer(verified.tenant(), consumer, topic);
        persistence
            .subscribe_mutation_outbox(&graph, &ledger, topic)
            .await?;
        let mut skipped = 0_usize;
        let mut taken = 0_u32;
        while taken < limit && skipped < MAX_FOREIGN_SKIPS_PER_PULL {
            let mut budget = OutboxClaimBudget::new(limit - taken, LEASE_MS, now_ms)?;
            let outcome = persistence
                .claim_mutation_outbox(&graph, &ledger, &mut budget)
                .await?;
            if outcome.claims.is_empty() {
                break;
            }
            for lease in outcome.claims {
                if lease
                    .record
                    .intent
                    .headers
                    .get("tenant")
                    .map(String::as_str)
                    == Some(verified.tenant())
                {
                    taken += 1;
                    leases.push(lease);
                } else {
                    // Another tenant's row: acknowledged in this tenant's own cursor so the
                    // ordered stream moves on, and never returned.
                    skipped += 1;
                    persistence
                        .ack_mutation_outbox(&graph, &lease, now_ms)
                        .await?;
                }
            }
            if !outcome.more_available {
                break;
            }
        }
    }
    Ok(OutboxPulled {
        schema_version: PULL_SCHEMA_VERSION,
        leases,
    })
}

/// Acknowledge one leased row of the caller's own cursor.
pub(super) async fn ack(
    persistence: &Arc<dyn PersistenceBackend>,
    verified: &VerifiedRequestContext,
    target: &OutboxTarget,
    consumer: &str,
    lease: &MutationOutboxLease,
    now_ms: u64,
) -> Result<OutboxAcked, String> {
    let graph = graph_of(target)?;
    let topic = lease.record.intent.topic.as_str();
    // A lease issued to another tenant or another consumer name is not the caller's: it answers
    // the same refusal as a lease that does not exist.
    if !MEMORY_EVENT_TOPICS.contains(&topic)
        || lease.consumer != ledger_consumer(verified.tenant(), consumer, topic)
    {
        return Err("ACCESS_DENIED: the lease is not held by this consumer".to_string());
    }
    let cursor = persistence
        .ack_mutation_outbox(&graph, lease, now_ms)
        .await?;
    Ok(OutboxAcked {
        schema_version: PULL_SCHEMA_VERSION,
        consumer: consumer.to_string(),
        batch_id: cursor.batch_id,
        ordinal: cursor.outbox_ordinal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R073.7
    #[test]
    fn the_ledger_consumer_is_per_tenant_consumer_and_topic() {
        let a = ledger_consumer("t1", "hooks", "memory.claim.changed");
        assert_ne!(a, ledger_consumer("t2", "hooks", "memory.claim.changed"));
        assert_ne!(a, ledger_consumer("t1", "other", "memory.claim.changed"));
        assert_ne!(a, ledger_consumer("t1", "hooks", "memory.card.updated"));
        assert_eq!(a, ledger_consumer("t1", "hooks", "memory.claim.changed"));
    }

    /// One committed batch whose outbox holds three `memory.claim.changed` rows, owned by tenants
    /// `t1`, `t2` and `t1` in that order, so a consumer of `t1` sees the foreign row between its own.
    fn claim_event_batch(
        graph: &str,
        batch_id: &str,
    ) -> medulla_protocol_types::mutation_batch::MutationBatch {
        use medulla_mutation_types::mutation_scope::{
            DurabilityDomain, IncarnationId, LogicalName, MutationOutboxIntent,
            MutationScopeIdentity, MutationSurface, ScopeTenantId, MUTATION_BATCH_VERSION,
        };
        use medulla_protocol_types::mutation_batch::{
            MutationBatch, MutationOperation, VersionExpectation, MUTATION_ACTOR_HEADER,
        };
        use medulla_protocol_types::protocol::Method;
        let identity = MutationScopeIdentity::graph(
            ScopeTenantId::new("caller-tenant").unwrap(),
            LogicalName::new(graph).unwrap(),
            IncarnationId::new("incarnation:test:memory-events").unwrap(),
        );
        let actor = format!("principal:sha256:{}", "c".repeat(64));
        let intent = |tenant: &str, claim: &str| MutationOutboxIntent {
            topic: "memory.claim.changed".to_string(),
            key: "ada".to_string(),
            payload: rmp_serde::to_vec_named(&serde_json::json!({
                "claim_id": claim,
                "from": "proposed",
                "to": "supported",
                "subject": "ada",
                "observer": "system",
            }))
            .unwrap(),
            headers: std::collections::BTreeMap::from([
                (MUTATION_ACTOR_HEADER.to_string(), actor.clone()),
                ("tenant".to_string(), tenant.to_string()),
                ("event_kind".to_string(), "claim_changed".to_string()),
            ]),
        };
        let mut batch = MutationBatch {
            schema_version: MUTATION_BATCH_VERSION,
            batch_id: batch_id.to_string(),
            envelope: medulla_store::redb_store::store_graph_names::fixture_operation_envelope(
                &identity, &actor, 17, batch_id,
            ),
            identity,
            placement_epoch: 1,
            version_expectation: VersionExpectation::Graph(0),
            fencing_token: Some(1),
            authoritative_state: None,
            operations: vec![MutationOperation {
                ordinal: 0,
                surface: MutationSurface::Transaction,
                domain: DurabilityDomain::GraphRows,
                method: Method::AddNode {
                    node_id: "memory-event-node".to_string(),
                    properties_msgpack: rmp_serde::to_vec_named(
                        &serde_json::json!({"type": "MemoryEvent"}),
                    )
                    .unwrap(),
                },
            }],
            outbox: vec![
                intent("t1", "c-first"),
                intent("t2", "c-foreign"),
                intent("t1", "c-last"),
            ],
            created_at_ms: 10,
        };
        let schema_digest = batch
            .envelope
            .operation()
            .expect("fixture carries an operation envelope")
            .method_schema_digest;
        batch.reseal_envelope(schema_digest).unwrap();
        batch
    }

    // spec: MEDULLA-DECISION-ENGINE-R073.7
    #[tokio::test(flavor = "multi_thread")]
    async fn pull_leases_then_ack_advances_cursor() {
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = tempfile::tempdir().unwrap();
        let backend = medulla_persistence::redb_backend::RedbBackend::open(
            dir.path().to_string_lossy().into_owned(),
            64,
        )
        .expect("open redb backend");
        let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);
        let graph = "memory-events-graph";
        persistence
            .commit_mutation_batch(graph, &claim_event_batch(graph, "events-batch"), None, 11)
            .await
            .expect("the fixture batch commits");

        let verified = VerifiedRequestContext::verified_for_test_in_tenant("consumer", "t1");
        let target = OutboxTarget::Graph {
            graph: graph.to_string(),
        };
        let topics = vec!["memory.claim.changed".to_string()];

        // The pull leases the tenant's two rows in commit order; the other tenant's row sits
        // between them, is acknowledged in this cursor and is never returned.
        let pulled = pull(&persistence, &verified, &target, "hooks", &topics, 8, 100)
            .await
            .unwrap();
        let claims: Vec<(String, u32)> = pulled
            .leases
            .iter()
            .map(|lease| (lease.record.batch_id.clone(), lease.record.ordinal))
            .collect();
        assert_eq!(
            claims,
            vec![
                ("events-batch".to_string(), 0),
                ("events-batch".to_string(), 2)
            ]
        );
        assert!(pulled
            .leases
            .iter()
            .all(|lease| lease.record.intent.headers["tenant"] == "t1"));

        // A lease of another consumer name is refused; the right one advances the cursor.
        let foreign = ack(
            &persistence,
            &verified,
            &target,
            "someone-else",
            &pulled.leases[0],
            101,
        )
        .await;
        assert!(foreign.unwrap_err().starts_with("ACCESS_DENIED"));
        let first = ack(
            &persistence,
            &verified,
            &target,
            "hooks",
            &pulled.leases[0],
            101,
        )
        .await
        .unwrap();
        assert_eq!(
            (first.batch_id.as_str(), first.ordinal),
            ("events-batch", 0)
        );
        let last = ack(
            &persistence,
            &verified,
            &target,
            "hooks",
            &pulled.leases[1],
            102,
        )
        .await
        .unwrap();
        assert_eq!((last.batch_id.as_str(), last.ordinal), ("events-batch", 2));

        // Everything is acknowledged: a later pull returns nothing, and a topic outside the three
        // is refused.
        let again = pull(&persistence, &verified, &target, "hooks", &topics, 8, 200)
            .await
            .unwrap();
        assert!(again.leases.is_empty());
        let outside = pull(
            &persistence,
            &verified,
            &target,
            "hooks",
            &["other.topic".to_string()],
            8,
            200,
        )
        .await;
        assert!(outside.unwrap_err().starts_with("ACCESS_DENIED"));
    }
}
