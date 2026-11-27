//! The native-component projection: the second Agent Library outbox consumer
//! (MEDULLA-TYPED-PACKS-R002.12).
//!
//! Every committed component revision publishes one event on the component
//! revision topic. This consumer claims those events and, for a published
//! blueprint prompt, writes one `PromptRevision` node per component and entry
//! revision into the tenant's `component__<digest>` graph, with a typed
//! `requires` edge to one `ComponentRef` node per pinned dependency. The write
//! goes through the same graph gateway commit the pack projection uses
//! ([`crate::graph_schema::commit_internal_graph_write`]), under the same
//! projection service identity.
//!
//! * A node is keyed by its component and revision, so a re-delivered or stale
//!   event finds the node already there and writes nothing new: an old event
//!   never overwrites or hides a newer revision.
//! * An event that is not a published blueprint prompt projects nothing and is
//!   acknowledged.
//! * A failed write that may succeed later leaves the lease unresolved, so the
//!   row is redelivered; a refused one is rejected as a failed projection and
//!   can be rewound through the Agent Library outbox operator.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use medulla_agent_types::agent_component::{
    AgentComponentEntry, AgentComponentMutationKind, AgentComponentOutboxEvent,
    PromptBlueprintIdentity,
};
use medulla_agent_types::agent_library::AgentLibraryLifecycle;
use medulla_catalog_types::connector_pack::ids::{
    escape_pack_name, native_component_projection_graph_name,
};
use medulla_catalog_types::graph_schema::{
    GraphSchemaCommitted, GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
};
use medulla_mutation_types::mutation_scope::MutationOutboxLease;
use medulla_persistence_records::agent_component::AGENT_COMPONENT_OUTBOX_TOPIC;
use medulla_transaction::OutboxRejectReason;

use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// The native projection's one durable consumer name on every tenant's outbox.
pub const NATIVE_CONSUMER: &str = "native-component-projection";

/// The projection source the gateway commit is attributed to.
const NATIVE_SOURCE: &str = "native-component-projection";

/// The label a failed blocking store task names.
const BLOCKING_TASK: &str = "native component projection";

/// Idle back-off between sweeps.
const IDLE: Duration = Duration::from_millis(500);

/// The ontology namespace the projected nodes and predicates live in.
const CORE: &str = "https://knuckles-team.github.io/ontology/core#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

/// How one claimed row ends.
enum Settle {
    /// Retry later: the lease is left unresolved so the row is redelivered.
    Retry(String),
    /// Resolve the row as not projectable.
    Reject(OutboxRejectReason, String),
}

pub(crate) async fn run(state: Arc<RwLock<ServerState>>) {
    let subscribed = AtomicBool::new(false);
    let mut turns = 0_u64;
    loop {
        turns += 1;
        medulla_observe::spawn::progress(
            medulla_observe::registry::tasks::ProgressCursor::Counter(turns),
            turns,
        );
        match sweep(&state, &subscribed).await {
            Ok(true) => {}
            Ok(false) => tokio::time::sleep(IDLE).await,
            Err(error) => {
                tracing::warn!(
                    code = "NATIVE_COMPONENT_PROJECTION_SWEEP_FAILED",
                    %error,
                    "native component projection sweep failed"
                );
                tokio::time::sleep(IDLE).await;
            }
        }
    }
}

/// One sweep over the deployment tenant's component revision stream. Returns
/// whether it did any work, so an idle worker backs off.
pub async fn sweep(
    state: &Arc<RwLock<ServerState>>,
    subscribed: &AtomicBool,
) -> Result<bool, String> {
    let tenant = super::deployment_tenant(state).await?;
    let store = super::open_agent_library(state).await?;
    if !subscribed.load(Ordering::Acquire) {
        let (store, tenant) = (Arc::clone(&store), tenant.clone());
        medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
            store.outbox_subscribe(&tenant, NATIVE_CONSUMER, AGENT_COMPONENT_OUTBOX_TOPIC)
        })
        .await?;
        subscribed.store(true, Ordering::Release);
    }
    let outcome = super::claim(&store, &tenant, NATIVE_CONSUMER).await?;
    super::report(&outcome, NATIVE_CONSUMER, AGENT_COMPONENT_OUTBOX_TOPIC);
    let progressed = !outcome.claims.is_empty();
    // The stream is strictly ordered: rows settle in claim order, and a row
    // left unresolved holds the rows after it until its lease runs out.
    for lease in outcome.claims {
        match project_row(state, &tenant, &lease).await {
            Ok(()) => settle(&store, &tenant, lease, None).await,
            Err(Settle::Reject(reason, error)) => {
                tracing::warn!(
                    code = "NATIVE_COMPONENT_PROJECTION_REJECTED",
                    %error,
                    "native component projection refused one event"
                );
                settle(&store, &tenant, lease, Some(reason)).await;
            }
            Err(Settle::Retry(error)) => {
                tracing::warn!(
                    code = "NATIVE_COMPONENT_PROJECTION_RETRY",
                    %error,
                    "native component projection will redeliver one event"
                );
                break;
            }
        }
    }
    Ok(progressed)
}

async fn settle(
    store: &Arc<medulla_persistence_records::agent_library::AgentLibraryStore>,
    tenant: &str,
    lease: MutationOutboxLease,
    reason: Option<OutboxRejectReason>,
) {
    if let Err(error) = super::resolve(
        store,
        tenant,
        lease,
        reason,
        NATIVE_CONSUMER,
        AGENT_COMPONENT_OUTBOX_TOPIC,
    )
    .await
    {
        tracing::warn!(
            code = "NATIVE_COMPONENT_PROJECTION_SETTLE_FAILED",
            %error,
            "native component projection: one row could not be resolved; the sweep continues"
        );
    }
}

/// Project one claimed event.
async fn project_row(
    state: &Arc<RwLock<ServerState>>,
    tenant: &str,
    lease: &MutationOutboxLease,
) -> Result<(), Settle> {
    let invalid = |error: String| Settle::Reject(OutboxRejectReason::InvalidEvent, error);
    let event: AgentComponentOutboxEvent =
        medulla_storage::decode_ledger_record(&lease.record.intent.payload)
            .map_err(|error| invalid(error.to_string()))?;
    let component = &event.component;
    if component.tenant_id != tenant {
        return Err(invalid(
            "the component event names another tenant than its outbox".to_string(),
        ));
    }
    // Only a published revision of a pinned blueprint prompt has a node.
    if component.lifecycle != AgentLibraryLifecycle::Published
        || !matches!(
            event.kind,
            AgentComponentMutationKind::Publish | AgentComponentMutationKind::Republish
        )
    {
        return Ok(());
    }
    let Some(identity) = PromptBlueprintIdentity::of_entry(component) else {
        return Ok(());
    };
    let document = prompt_revision_document(component, &identity);
    let graph = native_component_projection_graph_name(tenant).map_err(invalid)?;
    write_revision(state, &graph, &identity, document).await
}

/// Write one revision's document into the tenant's native-component graph.
async fn write_revision(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    identity: &PromptBlueprintIdentity,
    document: String,
) -> Result<(), Settle> {
    let key = format!("{}:{}", identity.component_id, identity.entry_revision);
    let (anonymous, server) = {
        let state = state.read().await;
        (state.local_anonymous, state.context.clone())
    };
    let verified = if anonymous {
        VerifiedRequestContext::anonymous_local(Some(format!("{NATIVE_CONSUMER}:{key}")))
    } else {
        super::service_context_for(&server, NATIVE_CONSUMER, &key).map_err(Settle::Retry)?
    };
    if !anonymous {
        grant_access(state, graph).await.map_err(Settle::Retry)?;
    }
    let source = medulla_types::contract::ResourceId::new(NATIVE_SOURCE)
        .map_err(|error| Settle::Reject(OutboxRejectReason::InvalidEvent, error.to_string()))?;
    crate::graph_schema::resident_internal_graph(state, 0, &verified, graph, verified.idempotency_key())
        .await
        .map_err(classify)?;
    let graph_name = graph.to_string();
    crate::graph_schema::commit_internal_graph_write(
        state,
        0,
        &verified,
        graph,
        verified.idempotency_key(),
        &source,
        move |core| {
            let triples = medulla_rdf::mapping::parse_ntriples(&document)?;
            medulla_rdf::update::insert_triples(core, &triples)?;
            Ok(GraphSchemaCommitted {
                schema_version: GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
                graph: graph_name,
                composed_digest: core.schema_sources().composed_digest().to_hex(),
                graph_version: core.version().saturating_add(1),
                changed: true,
                edit_head: None,
                ontology_diff: None,
                revision: None,
            })
        },
    )
    .await
    .map(drop)
    .map_err(classify)
}

/// Let the projection actor write exactly this native-component graph. A build
/// without the RBAC policy decides graph access by graph type alone, so there
/// is nothing to provision there.
async fn grant_access(state: &Arc<RwLock<ServerState>>, graph: &str) -> Result<(), String> {
    #[cfg(feature = "security")]
    {
        let mut guard = state.write().await;
        super::super::grant::ensure(&mut guard.isolation, graph)
            .map_err(|error| format!("PACK_PROJECTION_GRANT_FAILED: {error}"))
    }
    #[cfg(not(feature = "security"))]
    {
        let _ = (state, graph);
        Ok(())
    }
}

/// A failure that may succeed later is redelivered; any other is a refusal.
fn classify(error: String) -> Settle {
    let error = if crate::admin::connector_pack::reproject::projection_error_code(&error) == "BUSY"
    {
        format!("PACK_PROJECTION_STORE_BUSY: {error}")
    } else {
        error
    };
    if crate::admin::connector_pack::reproject::is_retryable_projection_error(&error) {
        Settle::Retry(error)
    } else {
        Settle::Reject(OutboxRejectReason::ProjectionFailed, error)
    }
}

/// The node of one prompt revision.
fn revision_iri(identity: &PromptBlueprintIdentity) -> String {
    format!(
        "urn:medulla:prompt-revision:{}:{}",
        escape_pack_name(&identity.component_id),
        identity.entry_revision
    )
}

/// The node of one pinned dependency: keyed by the component and the exact
/// definition digest the revision requires.
fn reference_iri(component_id: &str, definition_digest: &str) -> String {
    format!(
        "urn:medulla:component-ref:{}:{}",
        escape_pack_name(component_id),
        escape_pack_name(definition_digest)
    )
}

/// An N-Triples string literal.
fn literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn integer(value: u64) -> String {
    format!("\"{value}\"^^<{XSD_INTEGER}>")
}

/// The N-Triples document one prompt revision projects: the `PromptRevision`
/// node naming its committed identity, schema pin and body pin, and a typed
/// `requires` edge to a `ComponentRef` node for every pinned dependency.
fn prompt_revision_document(
    component: &AgentComponentEntry,
    identity: &PromptBlueprintIdentity,
) -> String {
    let node = revision_iri(identity);
    let mut lines = vec![
        format!("<{node}> <{RDF_TYPE}> <{CORE}PromptRevision> ."),
        format!(
            "<{node}> <{CORE}componentId> {} .",
            literal(&identity.component_id)
        ),
        format!(
            "<{node}> <{CORE}entryRevision> {} .",
            integer(identity.entry_revision)
        ),
        format!(
            "<{node}> <{CORE}schemaRevision> {} .",
            integer(u64::from(identity.schema_revision))
        ),
        format!(
            "<{node}> <{CORE}bodySha256> {} .",
            literal(&identity.body_sha256)
        ),
        format!(
            "<{node}> <{CORE}definitionDigest> {} .",
            literal(&component.definition_digest)
        ),
    ];
    for dependency in &component.requires {
        let reference = reference_iri(&dependency.component_id, &dependency.definition_digest);
        lines.push(format!(
            "<{reference}> <{RDF_TYPE}> <{CORE}ComponentRef> ."
        ));
        lines.push(format!(
            "<{reference}> <{CORE}componentId> {} .",
            literal(&dependency.component_id)
        ));
        lines.push(format!(
            "<{reference}> <{CORE}definitionDigest> {} .",
            literal(&dependency.definition_digest)
        ));
        lines.push(format!("<{node}> <{CORE}requires> <{reference}> ."));
    }
    lines.join("\n") + "\n"
}
