//! Scope and outbox construction for the canonical compiled batch.

use std::collections::BTreeMap;

use medulla_mutation_types::mutation_scope::IncarnationId;
use medulla_mutation_types::mutation_scope::LogicalName;
use medulla_mutation_types::mutation_scope::MutationOutboxIntent;
use medulla_mutation_types::mutation_scope::MutationScopeIdentity;
use medulla_mutation_types::mutation_scope::ScopeTenantId;
use medulla_protocol_types::mutation_batch::MutationOperation;
use medulla_protocol_types::mutation_batch::VersionExpectation;

use super::{CompileBatch, CompiledOutbox, COMPILED_BATCH_INCARNATION};

/// Derive the durable scope only after the caller and reserved-name checks.
pub(super) fn compiled_scope_identity(
    ctx: &CompileBatch<'_>,
    operations: &[MutationOperation],
    graph_scope: bool,
    scope_override: Option<MutationScopeIdentity>,
) -> Result<(MutationScopeIdentity, VersionExpectation), String> {
    let tenant_id = ScopeTenantId::new(ctx.tenant.to_string())?;
    let resource_name = LogicalName::new(ctx.graph.to_string())?;
    let incarnation_id = IncarnationId::new(COMPILED_BATCH_INCARNATION)
        .expect("COMPILED_BATCH_INCARNATION is a valid static incarnation id");
    let expected_version = ctx.expected_graph_version.ok_or_else(|| {
        "mutation batch requires its actual observed version under v1: VersionExpectation has \
         no unversioned arm available to an ordinary tenant (see validate_version_expectation); \
         pass the real current version instead of None"
            .to_string()
    })?;
    let (identity, version_expectation) = match scope_override {
        Some(identity) => {
            let expectation = if graph_scope {
                VersionExpectation::Graph(expected_version)
            } else {
                VersionExpectation::Native(expected_version)
            };
            (identity, expectation)
        }
        None if graph_scope => (
            MutationScopeIdentity::graph(tenant_id, resource_name, incarnation_id),
            VersionExpectation::Graph(expected_version),
        ),
        None => {
            let domain = operations
                .first()
                .map(|operation| operation.domain)
                .ok_or_else(|| {
                    "mutation batch has no operations to derive its native domain from".to_string()
                })?;
            (
                MutationScopeIdentity::native(tenant_id, domain, resource_name, incarnation_id)?,
                VersionExpectation::Native(expected_version),
            )
        }
    };
    Ok((identity, version_expectation))
}

/// The `finance.events.v1` outbox intent of one finance alert
/// (MEDULLA-FINANCE-PRIMITIVES-R011.2.1). The topic is the finance event topic,
/// the key is the envelope `event_id` (the idempotency identity, so a `finance.flip`
/// event keeps its existing id), the payload is the canonical msgpack of the
/// envelope and the one header `schema` holds the kind's schema name:
/// [`compiled_outbox`] adds the scope and actor headers itself.
pub(super) fn finance_alert_intent(
    envelope: &medulla_types::finance_event_envelope::FinanceAlertEnvelope,
) -> Result<MutationOutboxIntent, String> {
    envelope
        .validate()
        .map_err(|error| format!("ALERT_ENVELOPE_INVALID: {error}"))?;
    Ok(MutationOutboxIntent {
        topic: medulla_types::finance_event_envelope::FINANCE_EVENTS_TOPIC.to_string(),
        key: envelope.event_id.clone(),
        payload: envelope.canonical_msgpack()?,
        headers: BTreeMap::from([(
            "schema".to_string(),
            envelope.outbox_event_schema().to_string(),
        )]),
    })
}

/// The finance alert intents of every `FinanceAlertRaise` in `methods`, in order.
/// An invalid envelope refuses and compiles nothing.
pub(super) fn finance_alert_intents(
    methods: &[medulla_protocol_types::protocol::Method],
) -> Result<Vec<MutationOutboxIntent>, String> {
    methods
        .iter()
        .filter_map(|method| match method {
            medulla_protocol_types::protocol::Method::FinanceAlertRaise { envelope } => {
                Some(finance_alert_intent(envelope))
            }
            _ => None,
        })
        .collect()
}

/// Construct every intent before minting the envelope that covers their bytes.
pub(super) fn compiled_outbox(
    outbox_plan: CompiledOutbox,
    identity: &MutationScopeIdentity,
    summary: Vec<u8>,
    scope_digest: &str,
    actor: &str,
    batch_id: &str,
) -> Result<Vec<MutationOutboxIntent>, String> {
    let terminal_outcome = outbox_plan
        .extra
        .iter()
        .any(|intent| intent.topic == medulla_result_types::outcome_bundle::RUN_EVENT_OUTBOX_TOPIC);
    let mut outbox = if terminal_outcome {
        // A terminal receipt's conditional RunEvent is the batch's one outbox
        // currency. The generic projection wake-up would publish a second row
        // for the same WorkItem transition and would survive no-op/fenced
        // results unless the native terminal result filtered it too.
        outbox_plan.extra
    } else {
        let projection = MutationOutboxIntent {
            topic: "engine.projection.rebuild".to_string(),
            key: batch_id.to_string(),
            payload: summary,
            // `actor` is the verified caller's fingerprint on EVERY batch this
            // module compiles, whichever ledger commits it. It is the single
            // source of caller attribution, so an owner-store batch — whose
            // `context.principal` must be the store's serving principal — loses
            // none, and a graph batch gains no second, divergent copy.
            headers: BTreeMap::from([
                ("scope_sha256".to_string(), scope_digest.to_string()),
                ("actor".to_string(), actor.to_string()),
            ]),
        };
        let mut outbox = vec![projection];
        outbox.extend(outbox_plan.extra);
        outbox
    };
    if terminal_outcome {
        for intent in &mut outbox {
            if intent.topic == medulla_result_types::outcome_bundle::RUN_EVENT_OUTBOX_TOPIC {
                // The conditional terminal event replaces the generic
                // projection row, so carry the same universal attribution and
                // scope binding on the sole row that remains.
                intent
                    .headers
                    .insert("scope_sha256".to_string(), scope_digest.to_string());
                intent
                    .headers
                    .insert("actor".to_string(), actor.to_string());
            }
        }
    }
    if let Some(input_digest) = outbox_plan.semantic_source_dirty_input {
        outbox.push(queue_reembed(identity, batch_id, input_digest)?);
    }
    Ok(outbox)
}

/// The re-embed work item of a SQL write (MEDULLA-UNIFIED-DATA-PLANE-R040.6,
/// D110): one typed source-dirty intent on the existing semantic stage queue.
/// The external stage worker drains it through S1-S5 and completes S6 with
/// `CompleteGenerationStage`, whose promotion gate runs before the
/// re-embedded generation goes `Live`.
fn queue_reembed(
    identity: &MutationScopeIdentity,
    batch_id: &str,
    input_digest: medulla_index_types::semantic_index::SemanticDigest,
) -> Result<MutationOutboxIntent, String> {
    let source_scope_digest = medulla_index_types::semantic_index::SemanticDigest::from_bytes(
        *identity.binding_digest().as_bytes(),
    );
    let intent = medulla_index_types::semantic_index::SemanticSourceDirtyIntent::new(
        source_scope_digest,
        input_digest,
    );
    Ok(MutationOutboxIntent {
        topic: medulla_index_types::semantic_index::SEMANTIC_SOURCE_DIRTY_TOPIC.to_string(),
        key: batch_id.to_string(),
        payload: intent.to_canonical_cbor().map_err(|error| {
            format!("semantic source-dirty intent encoding rejected: {error:?}")
        })?,
        headers: BTreeMap::new(),
    })
}
