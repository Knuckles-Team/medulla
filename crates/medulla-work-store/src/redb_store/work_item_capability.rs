//! Native WorkItem claim-capability authority (RMDD-29, checkpoint 4).
//!
//! This module is deliberately not a generic bearer-token framework.  A
//! capability is an unguessable opaque nonce whose digest indexes a private,
//! encrypted native row.  The row binds the nonce to the authenticated worker,
//! session, graph lifecycle, and the exact live WorkItem lease tuple.  The
//! control row is checked first on every verification; private capability
//! metadata is never consulted for a caller that does not currently own a
//! live lease.

use medulla_store::{redb_store::decode_durable, redb_store::property_string, redb_store::DurableCrypto, redb_store::NODES};
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityDecision;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityMintRequest;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityRequestSchemaVersion;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityResult;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityResultSchemaVersion;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityVerifyRequest;
use medulla_storage::{GraphShardOwner, PhysicalWriteCapability, ScopedOwnerTableMut};
use medulla_transaction::{AdmittedOwnerWrite, OwnerPayloadWrite};
use rand::RngCore;
use redb::TableDefinition;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use medulla_store::redb_store::shard::ShardWrite;

pub mod lease;
use lease::{read_live_lease, record_matches_live};

/// Private tables are intentionally disjoint from WorkItem/status/list
/// projections, MutationBatch/outbox rows, and CDC/audit records.
pub const CAPABILITIES: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("work_item_claim_capabilities");
pub const INVOCATIONS: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("work_item_claim_capability_invocations");
/// Native provenance is the only durable proof that a WorkItem has crossed the
/// engine's claim authority.  It is intentionally separate from the public
/// WorkItem row: a caller may submit a ready/submitted row, but it cannot turn a
/// generic row into an active lease or replace a claimed row through AddNode,
/// CAS, row-delta, or cross-modal projection.
pub const NATIVE_WORK_ITEMS: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("native_work_item_authority");

/// Stable privacy-safe refusal used when a capability operation reaches a
/// replicated state-machine apply without the original authenticated envelope.
/// The apply path must return this before leader/barrier/private-ledger work.
pub const AUTHORITY_UNAVAILABLE: &str = "authority_unavailable";

// Private exact-input storage does not exist in this checkpoint.  This
// test-only boundary counter is wired to the actual private-row accesses
// below (the `CAPABILITIES` table get/decode in `mint_claim_capability`/
// `verify_claim_capability`) so a future payload reader placed before capability
// verification trips a failing test rather than a silent regression. The
// `NATIVE_WORK_ITEMS`/`NODES` reads performed by `read_live_lease` are the
// authorization check itself and are deliberately NOT counted here.
//
// `thread_local!`, NOT a process-global `AtomicUsize` (its original shape):
// `cargo test`'s default harness runs every `#[test]` function on its own
// dedicated OS thread from a pool, all sharing ONE process. A process-global
// counter is incremented by EVERY concurrently-running test that reads a
// private capability row — not just the one test measuring it — which is
// exactly the parallel-run flake this shape fixes: `private_work_item_body_
// reads()` observing 1 read instead of the expected 0, attributed to an
// unrelated sibling test's `mint`/`verify` call landing on a different OS
// thread during this test's measurement window. `mint_claim_capability` and
// `verify_claim_capability` run synchronously on the caller's thread (no internal thread spawn), and
// this is a plain (non-async) `#[test]`, so a thread-local counter isolates
// this test's own count perfectly — see `redb_store.rs`'s
// `COLD_SEED_ROWS_TOUCHED` for the identical fix applied to the same class
// of bug.
#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static PRIVATE_WORK_ITEM_BODY_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(any(test, feature = "test-support"))]
pub fn reset_private_work_item_body_reads() {
    PRIVATE_WORK_ITEM_BODY_READS.with(|c| c.set(0));
}

#[cfg(any(test, feature = "test-support"))]
pub fn private_work_item_body_reads() -> usize {
    PRIVATE_WORK_ITEM_BODY_READS.with(|c| c.get())
}

/// Record one access to the private `CAPABILITIES` row store — the capability
/// material itself, as opposed to the public WorkItem control row or the
/// native claim-provenance row consulted to establish authority. Call this at
/// the exact point the private row is fetched/decoded, never earlier. A
/// no-op outside `#[cfg(test)]` builds so call sites compile unconditionally
/// and impose no production cost.
#[cfg(any(test, feature = "test-support"))]
fn record_private_work_item_body_read() {
    PRIVATE_WORK_ITEM_BODY_READS.with(|c| c.set(c.get() + 1));
}

#[cfg(not(any(test, feature = "test-support")))]
#[inline(always)]
fn record_private_work_item_body_read() {}

pub const CAPABILITY_MAGIC: &[u8; 4] = b"WIC1";
pub const CAPABILITY_NONCE_BYTES: usize = 32;
pub const CAPABILITY_BYTES: usize = CAPABILITY_MAGIC.len() + CAPABILITY_NONCE_BYTES;
pub const MAX_CAPABILITY_BYTES: usize = 128;
const MAX_WORK_ITEM_ID_BYTES: usize = 512;
const MAX_AUTHORITY_TEXT_BYTES: usize = 512;
pub const MAX_CAPABILITY_RECORD_BYTES: usize = 16 * 1024;
pub const MAX_CAPABILITY_ROWS: usize = 4096;
const MAX_INVOCATION_ROWS: usize = 4096;
const MAX_NATIVE_WORK_ITEM_ROWS: usize = 65_536;

/// The server constructs this only after the signed request context has passed
/// authentication, replay, graph ACL, and scope checks.  It is crate-private so
/// Python/public callers cannot manufacture authority from an owner/lease tuple.
#[derive(Clone, Debug)]
pub struct AuthenticatedAuthority {
    pub tenant: String,
    pub audience: String,
    pub principal: String,
    pub agent_id: String,
    pub session: String,
    pub authority_epoch: u64,
    pub incarnation_id: String,
    pub now_ms: u64,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // fields kept for the capability record layout
pub struct LiveLease {
    pub tenant: String,
    pub agent_id: String,
    pub attempt: u64,
    pub lease_epoch: u64,
    pub fencing_token: u64,
    pub work_item_fence: String,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRecord {
    pub schema_version: u8,
    pub graph: String,
    pub tenant: String,
    pub audience: String,
    pub work_item_id: String,
    pub principal: String,
    pub agent_id: String,
    pub session: String,
    pub attempt: u64,
    pub lease_epoch: u64,
    pub fencing_token: u64,
    pub work_item_fence: String,
    pub authority_epoch: u64,
    pub incarnation_id: String,
    pub expires_at_ms: u64,
    pub created_at_ms: u64,
    /// The opaque nonce is retained only inside this private native row so an
    /// exact idempotent retry can return the same bytes after restart.
    #[serde(with = "serde_bytes")]
    pub capability: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvocationRecord {
    pub schema_version: u8,
    pub request_digest: String,
    pub capability_digest: String,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeWorkItemRecord {
    pub schema_version: u8,
    pub graph: String,
    pub work_item_id: String,
    pub tenant: String,
    pub kind: String,
    pub claimed_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NotFound,
    Unauthorized,
    Expired,
    Stale,
    Malformed,
    InputConflict,
    RetentionExhausted,
}

impl Refusal {
    fn decision(self) -> WorkItemClaimCapabilityDecision {
        match self {
            Self::NotFound => WorkItemClaimCapabilityDecision::NotFound,
            Self::Unauthorized => WorkItemClaimCapabilityDecision::Unauthorized,
            Self::Expired => WorkItemClaimCapabilityDecision::Expired,
            Self::Stale => WorkItemClaimCapabilityDecision::Stale,
            Self::Malformed => WorkItemClaimCapabilityDecision::Malformed,
            Self::InputConflict => WorkItemClaimCapabilityDecision::InputConflict,
            Self::RetentionExhausted => WorkItemClaimCapabilityDecision::RetentionExhausted,
        }
    }
}

/// One private capability table opened for writing on one graph's scope.
type CapabilityRows<'a> = ScopedOwnerTableMut<'a, (&'static str, &'static str), &'static [u8]>;

/// How many rows of the bound graph one private table holds, counted no
/// further than `ceiling + 1`.
///
/// Enough to decide any retention bound without walking a whole scope, and
/// bounded by the capability rather than by a starting key: `scope_rows` is
/// already confined to this table's own graph.
fn scope_row_count(table: &CapabilityRows<'_>, ceiling: usize) -> Result<usize, String> {
    let mut count = 0usize;
    for row in table.scope_rows()? {
        row?;
        count = count.saturating_add(1);
        if count > ceiling {
            break;
        }
    }
    Ok(count)
}

/// Remove every row of the bound graph from one of this module's tables,
/// refusing a scope that already exceeds the table's native retention bound.
///
/// The bound is checked before the first removal, so a scope outside contract
/// is refused whole rather than half-swept -- and it is checked identically
/// whether the sweep came from a graph clear or from a scope retirement,
/// because those are the same operation. The removal itself is the kernel's
/// `ScopedOwnerTableMut::purge_scope_rows`, whose scope comes from the
/// capability that opened the table and never from an argument.
fn purge_within_retention(
    table: &mut CapabilityRows<'_>,
    retention: usize,
    exceeded: &str,
) -> Result<(), String> {
    if scope_row_count(table, retention)? > retention {
        return Err(exceeded.to_string());
    }
    table.purge_scope_rows()
}

/// Purge private capability and invocation rows under the caller's bound
/// owner scope. Both admitted writes and graft payload writes implement this
/// narrow capability; neither can name a different graph here.
fn purge_private_rows(write: &impl OwnerPayloadWrite) -> Result<(), String> {
    purge_within_retention(
        &mut write.open_scoped_table(CAPABILITIES)?,
        MAX_CAPABILITY_ROWS,
        "native capability retention bound exceeded",
    )?;
    purge_within_retention(
        &mut write.open_scoped_table(INVOCATIONS)?,
        MAX_INVOCATION_ROWS,
        "native capability invocation retention bound exceeded",
    )
}

/// Drop this module's rows for one graph inside an admitted group, opening the
/// native provenance table itself.
pub fn clear_graph_rows(write: &ShardWrite<'_>, graph: &str) -> Result<(), String> {
    let mut native_work_items = write.graph(graph)?.open_scoped_table(NATIVE_WORK_ITEMS)?;
    clear_graph_rows_with_native(write, graph, &mut native_work_items)
}

/// Clear private capability state when the caller already owns the native
/// provenance table in this transaction.  redb intentionally rejects opening
/// one table twice in a write transaction, so graph clear/restore paths must
/// reuse the existing handle to preserve atomic purge semantics.
pub fn clear_graph_rows_with_native(
    write: &ShardWrite<'_>,
    graph: &str,
    native_work_items: &mut CapabilityRows<'_>,
) -> Result<(), String> {
    let rows = write.graph(graph)?;
    purge_private_rows(rows)?;
    purge_within_retention(
        native_work_items,
        MAX_NATIVE_WORK_ITEM_ROWS,
        "native WorkItem authority retention bound exceeded",
    )
}

/// Clear this module's graph rows through an already-admitted graph member.
///
/// Online reshard/import uses a member of the shard's maintenance group rather
/// than a [`ShardWrite`] wrapper.  The member's bound identity is the authority
/// for the scope; the graph argument is checked only to reject a mismatched
/// caller before any table is opened.
pub fn clear_graph_rows_in_wtx(
    write: &AdmittedOwnerWrite<'_, GraphShardOwner>,
    graph: &str,
) -> Result<(), String> {
    if write
        .identity()
        .scope()
        .graph_name()
        .map(|name| name.as_str())
        != Some(graph)
    {
        return Err("native capability clear graph does not match admitted scope".to_string());
    }
    let mut native_work_items = write.open_scoped_table(NATIVE_WORK_ITEMS)?;
    clear_graph_rows_in_wtx_with_native(write, &mut native_work_items)
}

/// Clear this module's graph rows through the narrow owner-payload capability
/// used by a graft reservation.  It intentionally has no graph-name argument:
/// the capability's scope is the authority, and unlike an admitted member it
/// cannot reach mutation-ledger tables or another graph.
pub fn clear_graph_rows_in_payload<W: OwnerPayloadWrite>(write: &W) -> Result<(), String> {
    let mut native_work_items = write.open_scoped_table(NATIVE_WORK_ITEMS)?;
    purge_private_rows(write)?;
    purge_within_retention(
        &mut native_work_items,
        MAX_NATIVE_WORK_ITEM_ROWS,
        "native WorkItem authority retention bound exceeded",
    )
}

/// Variant used when the caller already owns the native provenance handle in
/// this admitted transaction.  Opening a table twice is rejected by redb, so
/// import/replace paths must reuse that handle while purging the two private
/// capability tables.
pub fn clear_graph_rows_in_wtx_with_native(
    write: &AdmittedOwnerWrite<'_, GraphShardOwner>,
    native_work_items: &mut CapabilityRows<'_>,
) -> Result<(), String> {
    purge_private_rows(write)?;
    purge_within_retention(
        native_work_items,
        MAX_NATIVE_WORK_ITEM_ROWS,
        "native WorkItem authority retention bound exceeded",
    )
}

/// Retire this module's rows for the capability's own graph scope.
///
/// The payload half of a scope retirement (`OwnerPayloadRetirement`): the scope
/// comes from the capability, never from an argument, so one graph's handle can
/// never sweep another's rows in the file they share. All three of this
/// module's tables are listed -- missing one would hand the retired
/// generation's private capability material to the next binding of the same
/// graph name, which is the forgery this module exists to make impossible.
///
/// The row work is [`purge_within_retention`], the same sweep
/// [`clear_graph_rows_with_native`] performs and under the same retention
/// bounds; the two differ only in the capability the caller holds.
pub fn retire_graph_rows(
    write: &PhysicalWriteCapability<'_, GraphShardOwner>,
) -> Result<(), String> {
    purge_within_retention(
        &mut write.scoped_owner_table_mut(CAPABILITIES)?,
        MAX_CAPABILITY_ROWS,
        "native capability retention bound exceeded",
    )?;
    purge_within_retention(
        &mut write.scoped_owner_table_mut(INVOCATIONS)?,
        MAX_INVOCATION_ROWS,
        "native capability invocation retention bound exceeded",
    )?;
    purge_within_retention(
        &mut write.scoped_owner_table_mut(NATIVE_WORK_ITEMS)?,
        MAX_NATIVE_WORK_ITEM_ROWS,
        "native WorkItem authority retention bound exceeded",
    )
}

/// Record a successful native claim in the same WriteTransaction as the lease
/// transition.  The record is deliberately non-reconstructible from a public
/// owner/lease tuple: capability verification requires this engine-created row.
pub fn record_native_claim(
    native_work_items: &mut CapabilityRows<'_>,
    graph: &str,
    work_item_id: &str,
    props: &serde_json::Map<String, serde_json::Value>,
    claimed_at_ms: u64,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    let record = NativeWorkItemRecord {
        schema_version: 1,
        graph: graph.to_string(),
        work_item_id: work_item_id.to_string(),
        tenant: property_string(props, "tenant").to_string(),
        kind: property_string(props, "kind").to_string(),
        claimed_at_ms,
    };
    let encoded = encode_private(&record, crypto)?;
    native_work_items.insert((graph, work_item_id), encoded.as_slice())
}

/// Name retained for the claim row's admitted-write call site.  The native
/// provenance row is written through the caller-owned scoped handle, in the
/// same transaction as the lease transition.
pub fn record_native_claim_in_wtx(
    native_work_items: &mut ScopedOwnerTableMut<'_, (&str, &str), &[u8]>,
    graph: &str,
    work_item_id: &str,
    props: &serde_json::Map<String, serde_json::Value>,
    claimed_at_ms: u64,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    record_native_claim(
        native_work_items,
        graph,
        work_item_id,
        props,
        claimed_at_ms,
        crypto,
    )
}

fn native_claim_exists(
    native_work_items: &CapabilityRows<'_>,
    graph: &str,
    work_item_id: &str,
    crypto: DurableCrypto<'_>,
) -> Result<bool, Refusal> {
    let Some(row) = native_work_items
        .get((graph, work_item_id))
        .map_err(|_| Refusal::Stale)?
    else {
        return Ok(false);
    };
    let record: NativeWorkItemRecord =
        decode_private(row.value(), crypto).map_err(|_| Refusal::Stale)?;
    Ok(record.schema_version == 1 && record.graph == graph && record.work_item_id == work_item_id)
}

/// Require engine-created claim provenance in the caller's native transaction.
pub fn require_native_claim(
    native_work_items: &CapabilityRows<'_>,
    graph: &str,
    work_item_id: &str,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    match native_claim_exists(native_work_items, graph, work_item_id, crypto) {
        Ok(true) => Ok(()),
        _ => Err("native WorkItem claim provenance unavailable".into()),
    }
}

const WORK_ITEM_AUTHORITY_KEYS: &[&str] = &[
    "node_type",
    "kind",
    "status",
    "tenant",
    "lease_owner",
    "last_lease_owner",
    "worker_ref",
    "session",
    "idempotency_key",
    "lease_epoch",
    "fencing_token",
    "work_item_fence",
    "lease_expires_at",
    "authority_epoch",
    "incarnation_id",
    "attempt",
    "metadata",
    "execution_payload",
    "operation_payload",
    "result_ref",
    "error_ref",
    "completed_at",
];

fn is_work_item(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    property_string(props, "node_type") == "WorkItem"
}

fn has_active_value(props: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    let Some(value) = props.get(key) else {
        return false;
    };
    if value.is_null() {
        return false;
    }
    match value {
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|number| number > 0.0),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Null => false,
    }
}

fn validate_submission_properties(
    props: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if !is_work_item(props) {
        return Ok(());
    }
    let status = property_string(props, "status");
    if !matches!(status, "submitted" | "ready") {
        return Err("native WorkItem authority required for active lease fields".to_string());
    }
    for key in [
        "lease_owner",
        "last_lease_owner",
        "worker_ref",
        "session",
        "lease_epoch",
        "fencing_token",
        "work_item_fence",
        "lease_expires_at",
        "authority_epoch",
        "incarnation_id",
        "attempt",
    ] {
        if has_active_value(props, key) {
            return Err("native WorkItem authority required for active lease fields".to_string());
        }
    }
    Ok(())
}

fn authority_update_keys(props: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    props.keys().find_map(|key| {
        WORK_ITEM_AUTHORITY_KEYS
            .contains(&key.as_str())
            .then_some(key.as_str())
    })
}

type WorkItemTable<'txn> = CapabilityRows<'txn>;

fn native_claimed(
    native_work_items: &WorkItemTable<'_>,
    graph: &str,
    work_item_id: &str,
) -> Result<bool, String> {
    native_work_items
        .get((graph, work_item_id))
        .map(|row| row.is_some())
}

fn existing_work_item(
    nodes: &WorkItemTable<'_>,
    graph: &str,
    work_item_id: &str,
    crypto: DurableCrypto<'_>,
) -> Result<bool, String> {
    let Some(row) = nodes.get((graph, work_item_id))? else {
        return Ok(false);
    };
    let bytes = crypto.unseal(row.value())?;
    let props = decode_durable::<serde_json::Map<String, serde_json::Value>>(&bytes)?;
    Ok(is_work_item(&props))
}

struct GenericValidationContext<'borrow, 'txn, 'crypto> {
    pub graph: &'borrow str,
    pub node_id: &'borrow str,
    pub tables: GenericValidationTables<'borrow, 'txn, 'crypto>,
}

#[derive(Clone, Copy)]
struct GenericValidationTables<'borrow, 'txn, 'crypto> {
    pub nodes: &'borrow WorkItemTable<'txn>,
    pub native_work_items: &'borrow WorkItemTable<'txn>,
    pub crypto: DurableCrypto<'crypto>,
}

impl<'borrow, 'txn, 'crypto> GenericValidationTables<'borrow, 'txn, 'crypto> {
    pub fn new(
        nodes: &'borrow WorkItemTable<'txn>,
        native_work_items: &'borrow WorkItemTable<'txn>,
        crypto: DurableCrypto<'crypto>,
    ) -> Self {
        Self {
            nodes,
            native_work_items,
            crypto,
        }
    }
}

impl<'borrow, 'txn, 'crypto> GenericValidationContext<'borrow, 'txn, 'crypto> {
    pub fn new(
        graph: &'borrow str,
        node_id: &'borrow str,
        tables: GenericValidationTables<'borrow, 'txn, 'crypto>,
    ) -> Self {
        Self {
            graph,
            node_id,
            tables,
        }
    }

    fn native_claimed(&self) -> Result<bool, String> {
        native_claimed(self.tables.native_work_items, self.graph, self.node_id)
    }

    fn existing_work_item(&self) -> Result<bool, String> {
        existing_work_item(
            self.tables.nodes,
            self.graph,
            self.node_id,
            self.tables.crypto,
        )
    }
}

fn validate_generic_add_node(
    context: GenericValidationContext<'_, '_, '_>,
    properties_msgpack: &[u8],
    replacement_error: &str,
) -> Result<(), String> {
    if context.native_claimed()? || context.existing_work_item()? {
        return Err(replacement_error.to_string());
    }
    // Ordinary graph nodes may retain legacy opaque property bytes; only a
    // structurally valid WorkItem map is subject to the native authority guard.
    if let Ok(props) =
        decode_durable::<serde_json::Map<String, serde_json::Value>>(properties_msgpack)
    {
        validate_submission_properties(&props)?;
    }
    Ok(())
}

fn validate_generic_update(
    context: GenericValidationContext<'_, '_, '_>,
    updates_msgpack: &[u8],
) -> Result<(), String> {
    if context.native_claimed()? {
        return Err("native WorkItem authority required for generic update".to_string());
    }
    if context.existing_work_item()? {
        let updates = decode_durable::<serde_json::Map<String, serde_json::Value>>(updates_msgpack)
            .map_err(|_| "invalid WorkItem update properties".to_string())?;
        if let Some(key) = authority_update_keys(&updates) {
            return Err(format!(
                "native WorkItem authority required for protected field '{key}'"
            ));
        }
    }
    Ok(())
}

/// What one decoded generic batch does to node rows -- the only rows the
/// WorkItem guards own. Edge and embedding operations touch none.
pub enum NodeTouch {
    Write {
        id: String,
        properties_msgpack: Vec<u8>,
    },
    Remove {
        id: String,
    },
}

/// Decode a generic batch into its node touches, in operation order: the one
/// reading both the native-authority check and the row guard share.
pub fn batch_node_touches(operations_msgpack: &[u8]) -> Result<Vec<NodeTouch>, String> {
    use medulla_graph_algos::algorithms::BatchOperation;

    Ok(
        medulla_graph_algos::algorithms::decode_batch_operations(operations_msgpack)?
            .into_iter()
            .filter_map(|operation| match operation {
                BatchOperation::AddNode {
                    id,
                    properties_msgpack,
                    ..
                } => Some(NodeTouch::Write {
                    id,
                    properties_msgpack,
                }),
                BatchOperation::RemoveNode { id } => Some(NodeTouch::Remove { id }),
                BatchOperation::AddEdge { .. }
                | BatchOperation::RemoveEdge { .. }
                | BatchOperation::AddEmbedding { .. } => None,
            })
            .collect(),
    )
}

fn validate_generic_batch(
    graph: &str,
    operations_msgpack: &[u8],
    tables: GenericValidationTables<'_, '_, '_>,
) -> Result<(), String> {
    for touch in batch_node_touches(operations_msgpack)? {
        match touch {
            NodeTouch::Write {
                id,
                properties_msgpack,
            } => validate_generic_add_node(
                GenericValidationContext::new(graph, &id, tables),
                &properties_msgpack,
                "native WorkItem authority required for generic batch replacement",
            )?,
            NodeTouch::Remove { id } if native_claimed(tables.native_work_items, graph, &id)? => {
                return Err(
                    "native WorkItem authority required for generic batch removal".to_string(),
                )
            }
            NodeTouch::Remove { .. } => {}
        }
    }
    Ok(())
}

/// Guard generic graph-row writers before they can manufacture or replace
/// native WorkItem authority.  Submission is intentionally the one exception:
/// a new `submitted`/`ready` WorkItem may be projected by the existing AU
/// submitter.  Once ClaimWorkItem records the private native marker, even
/// otherwise harmless generic CAS/AddNode/Set operations are refused.
pub fn validate_generic_method(
    graph: &str,
    method: &medulla_protocol_types::protocol::Method,
    nodes: &CapabilityRows<'_>,
    native_work_items: &CapabilityRows<'_>,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    let tables = GenericValidationTables::new(nodes, native_work_items, crypto);
    match method {
        medulla_protocol_types::protocol::Method::AddNode {
            node_id,
            properties_msgpack,
        } => validate_generic_add_node(
            GenericValidationContext::new(graph, node_id, tables),
            properties_msgpack,
            "native WorkItem authority required for generic replacement",
        ),
        medulla_protocol_types::protocol::Method::RemoveNode { node_id } => {
            if native_claimed(tables.native_work_items, graph, node_id)? {
                return Err("native WorkItem authority required for generic removal".to_string());
            }
            Ok(())
        }
        medulla_protocol_types::protocol::Method::CompareAndSetNodeFields {
            node_id,
            updates_msgpack,
            ..
        } => validate_generic_update(
            GenericValidationContext::new(graph, node_id, tables),
            updates_msgpack,
        ),
        medulla_protocol_types::protocol::Method::BatchUpdate { operations_msgpack } => {
            validate_generic_batch(graph, operations_msgpack, tables)
        }
        medulla_protocol_types::protocol::Method::SetPose { node_id, .. } => {
            if native_claimed(tables.native_work_items, graph, node_id)? {
                return Err("native WorkItem authority required for generic update".to_string());
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn validate_snapshot_nodes(nodes: &[(String, Vec<u8>)]) -> Result<(), String> {
    for (_, bytes) in nodes {
        // Checkpoint images may contain legacy opaque ordinary-node payloads;
        // a decoded WorkItem map is the only shape whose authority fields are
        // meaningful to this guard.
        if let Ok(props) = decode_durable::<serde_json::Map<String, serde_json::Value>>(bytes) {
            validate_submission_properties(&props)?;
        }
    }
    Ok(())
}

/// Atomically mint or replay the capability while the caller's write
/// transaction is held.  The request contains no mutable authority fields.
pub fn mint_claim_capability(
    write: &ShardWrite<'_>,
    graph: &str,
    request: &WorkItemClaimCapabilityMintRequest,
    authority: &AuthenticatedAuthority,
    crypto: DurableCrypto<'_>,
) -> Result<WorkItemClaimCapabilityResult, String> {
    validate_authority(graph, authority)?;
    if request.schema_version != WorkItemClaimCapabilityRequestSchemaVersion::V1
        || !bounded_text(&request.work_item_id, MAX_WORK_ITEM_ID_BYTES)
    {
        return Ok(refusal_result(Refusal::Malformed));
    }
    let rows = write.graph(graph)?;
    let native_work_items = rows.open_scoped_table(NATIVE_WORK_ITEMS)?;
    let nodes = rows.open_scoped_table(NODES)?;
    // Control-row authorization happens before either private capability table
    // is opened/read.  This is the no-private-row-read-before-authorization
    // checkpoint required by RMDD-29.
    let live = match read_live_lease(
        &nodes,
        &native_work_items,
        graph,
        &request.work_item_id,
        authority,
        crypto,
    ) {
        Ok(lease) => lease,
        Err(error) => return Ok(refusal_result(error)),
    };
    drop(nodes);

    let mut capabilities = rows.open_scoped_table(CAPABILITIES)?;
    let mut invocations = rows.open_scoped_table(INVOCATIONS)?;
    prune_expired(
        &mut capabilities,
        &mut invocations,
        authority.now_ms,
        crypto,
    )?;

    let request_digest = request_digest(graph, request, authority, &live);
    // Same private-row boundary as `verify_claim_capability`: these reads happen only
    // after read_live_lease above has already established authority for this
    // mint/replay request.
    record_private_work_item_body_read();
    if let Some(stored) = invocations.get((graph, authority.session.as_str()))? {
        let invocation: InvocationRecord = decode_private(stored.value(), crypto)?;
        if invocation.request_digest != request_digest {
            return Ok(refusal_result(Refusal::InputConflict));
        }
        record_private_work_item_body_read();
        let Some(capability) = capabilities.get((graph, invocation.capability_digest.as_str()))?
        else {
            return Ok(refusal_result(Refusal::Stale));
        };
        let record: CapabilityRecord = decode_private(capability.value(), crypto)?;
        if !record_matches_live(&record, graph, &request.work_item_id, authority, &live) {
            return Ok(refusal_result(
                if record.expires_at_ms <= authority.now_ms {
                    Refusal::Expired
                } else {
                    Refusal::Stale
                },
            ));
        }
        return Ok(WorkItemClaimCapabilityResult {
            schema_version: WorkItemClaimCapabilityResultSchemaVersion::V1,
            decision: WorkItemClaimCapabilityDecision::Replayed,
            valid: true,
            capability: Some(record.capability),
        });
    }

    // An exact retry must remain replayable even when the bounded native
    // tables are full.  Retention applies only to a new session/capability;
    // it must never turn a lost-ack retry into a second mint or a refusal.
    if scope_row_count(&capabilities, MAX_CAPABILITY_ROWS)? >= MAX_CAPABILITY_ROWS
        || scope_row_count(&invocations, MAX_INVOCATION_ROWS)? >= MAX_INVOCATION_ROWS
    {
        return Ok(refusal_result(Refusal::RetentionExhausted));
    }

    let mut nonce = [0_u8; CAPABILITY_NONCE_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut capability = Vec::with_capacity(CAPABILITY_BYTES);
    capability.extend_from_slice(CAPABILITY_MAGIC);
    capability.extend_from_slice(&nonce);
    let capability_digest = digest_capability(&capability);
    let record = CapabilityRecord {
        schema_version: 1,
        graph: graph.to_string(),
        tenant: authority.tenant.clone(),
        audience: authority.audience.clone(),
        work_item_id: request.work_item_id.clone(),
        principal: authority.principal.clone(),
        agent_id: authority.agent_id.clone(),
        session: authority.session.clone(),
        attempt: live.attempt,
        lease_epoch: live.lease_epoch,
        fencing_token: live.fencing_token,
        work_item_fence: live.work_item_fence,
        authority_epoch: authority.authority_epoch,
        incarnation_id: authority.incarnation_id.clone(),
        expires_at_ms: live.expires_at_ms,
        created_at_ms: authority.now_ms,
        capability: capability.clone(),
    };
    let invocation = InvocationRecord {
        schema_version: 1,
        request_digest,
        capability_digest: capability_digest.clone(),
        expires_at_ms: live.expires_at_ms,
    };
    let record_bytes = encode_private(&record, crypto)?;
    let invocation_bytes = encode_private(&invocation, crypto)?;
    capabilities.insert((graph, capability_digest.as_str()), record_bytes.as_slice())?;
    invocations.insert(
        (graph, authority.session.as_str()),
        invocation_bytes.as_slice(),
    )?;
    Ok(WorkItemClaimCapabilityResult {
        schema_version: WorkItemClaimCapabilityResultSchemaVersion::V1,
        decision: WorkItemClaimCapabilityDecision::Minted,
        valid: true,
        capability: Some(capability),
    })
}

/// Verify the opaque capability after the authoritative WorkItem control row
/// has been checked.  No private payload/blob row is touched by this function.
pub fn verify_claim_capability(
    write: &ShardWrite<'_>,
    graph: &str,
    request: &WorkItemClaimCapabilityVerifyRequest,
    authority: &AuthenticatedAuthority,
    crypto: DurableCrypto<'_>,
) -> Result<WorkItemClaimCapabilityResult, String> {
    if validate_authority(graph, authority).is_err() {
        return Ok(verification_refusal_result());
    }
    if request.schema_version != WorkItemClaimCapabilityRequestSchemaVersion::V1
        || !bounded_text(&request.work_item_id, MAX_WORK_ITEM_ID_BYTES)
    {
        return Ok(verification_refusal_result());
    }
    if request.capability.len() > MAX_CAPABILITY_BYTES
        || request.capability.len() != CAPABILITY_BYTES
        || !request.capability.starts_with(CAPABILITY_MAGIC)
    {
        return Ok(verification_refusal_result());
    }
    let rows = write.graph(graph)?;
    let native_work_items = rows.open_scoped_table(NATIVE_WORK_ITEMS)?;
    let nodes = rows.open_scoped_table(NODES)?;
    // The live lease is authoritative.  The private capability table is not
    // consulted until this check succeeds, preventing a forged public tuple or
    // a scope-mismatched caller from probing private rows.
    let live = match read_live_lease(
        &nodes,
        &native_work_items,
        graph,
        &request.work_item_id,
        authority,
        crypto,
    ) {
        Ok(lease) => lease,
        Err(_) => return Ok(verification_refusal_result()),
    };
    drop(nodes);
    let capabilities = rows.open_scoped_table(CAPABILITIES)?;
    let digest = digest_capability(&request.capability);
    // This is the private-row access the zero-private-body-read ordering
    // requirement guards: the capability material itself, fetched only after
    // `read_live_lease` above has already established authority.
    record_private_work_item_body_read();
    let Some(stored) = capabilities.get((graph, digest.as_str()))? else {
        return Ok(verification_refusal_result());
    };
    let record: CapabilityRecord = match decode_private(stored.value(), crypto) {
        Ok(record) => record,
        Err(_) => return Ok(verification_refusal_result()),
    };
    if record.expires_at_ms <= authority.now_ms {
        return Ok(verification_refusal_result());
    }
    if !record_matches_live(&record, graph, &request.work_item_id, authority, &live)
        || record.capability != request.capability
    {
        return Ok(verification_refusal_result());
    }
    Ok(WorkItemClaimCapabilityResult {
        schema_version: WorkItemClaimCapabilityResultSchemaVersion::V1,
        decision: WorkItemClaimCapabilityDecision::Verified,
        valid: true,
        capability: None,
    })
}

fn request_digest(
    graph: &str,
    request: &WorkItemClaimCapabilityMintRequest,
    authority: &AuthenticatedAuthority,
    live: &LiveLease,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"medulla.work-item-claim-capability.v1\0");
    for value in [
        graph.as_bytes(),
        authority.tenant.as_bytes(),
        authority.audience.as_bytes(),
        request.work_item_id.as_bytes(),
        authority.principal.as_bytes(),
        authority.agent_id.as_bytes(),
        authority.session.as_bytes(),
        authority.incarnation_id.as_bytes(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    digest.update(authority.authority_epoch.to_be_bytes());
    digest.update(live.attempt.to_be_bytes());
    digest.update(live.lease_epoch.to_be_bytes());
    digest.update(live.fencing_token.to_be_bytes());
    digest.update(live.expires_at_ms.to_be_bytes());
    hex::encode(digest.finalize())
}

fn digest_capability(capability: &[u8]) -> String {
    hex::encode(Sha256::digest(capability))
}

fn encode_private<T: Serialize>(value: &T, crypto: DurableCrypto<'_>) -> Result<Vec<u8>, String> {
    let plain =
        rmp_serde::to_vec_named(value).map_err(|_| "capability record is invalid".to_string())?;
    if plain.len() > MAX_CAPABILITY_RECORD_BYTES {
        return Err("capability record exceeds native bound".to_string());
    }
    Ok(crypto.seal(&plain).into_owned())
}

fn decode_private<T: serde::de::DeserializeOwned>(
    stored: &[u8],
    crypto: DurableCrypto<'_>,
) -> Result<T, String> {
    if stored.len() > MAX_CAPABILITY_RECORD_BYTES {
        return Err("capability record exceeds native bound".to_string());
    }
    let plain = crypto.unseal(stored)?;
    decode_durable(&plain).map_err(|_| "capability record is invalid".to_string())
}

pub mod authority;
use authority::{bounded_text, validate_authority};

fn prune_expired(
    capabilities: &mut CapabilityRows<'_>,
    invocations: &mut CapabilityRows<'_>,
    now_ms: u64,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    let mut capability_keys = Vec::new();
    let mut capability_scanned = 0usize;
    for row in capabilities.scope_rows()? {
        let (key, value) = row?;
        let (row_graph, digest) = key.value();
        capability_scanned = capability_scanned.saturating_add(1);
        if capability_scanned > MAX_CAPABILITY_ROWS {
            return Err("native capability retention bound exceeded".to_string());
        }
        let record = decode_private::<CapabilityRecord>(value.value(), crypto)
            .map_err(|_| "native capability record is invalid".to_string())?;
        if record.expires_at_ms <= now_ms {
            capability_keys.push((row_graph.to_string(), digest.to_string()));
        }
    }
    for (row_graph, digest) in capability_keys {
        capabilities.remove((row_graph.as_str(), digest.as_str()))?;
    }
    let mut invocation_keys = Vec::new();
    let mut invocation_scanned = 0usize;
    for row in invocations.scope_rows()? {
        let (key, value) = row?;
        let (row_graph, session) = key.value();
        invocation_scanned = invocation_scanned.saturating_add(1);
        if invocation_scanned > MAX_INVOCATION_ROWS {
            return Err("native capability invocation retention bound exceeded".to_string());
        }
        let record = decode_private::<InvocationRecord>(value.value(), crypto)
            .map_err(|_| "native capability invocation is invalid".to_string())?;
        if record.expires_at_ms <= now_ms {
            invocation_keys.push((row_graph.to_string(), session.to_string()));
        }
    }
    for (row_graph, session) in invocation_keys {
        invocations.remove((row_graph.as_str(), session.as_str()))?;
    }
    Ok(())
}

fn refusal_result(refusal: Refusal) -> WorkItemClaimCapabilityResult {
    WorkItemClaimCapabilityResult {
        schema_version: WorkItemClaimCapabilityResultSchemaVersion::V1,
        decision: refusal.decision(),
        valid: false,
        capability: None,
    }
}

fn verification_refusal_result() -> WorkItemClaimCapabilityResult {
    // Verification has one externally observable denial.  The detailed
    // refusal remains an internal control-flow value so callers cannot probe
    // tenant, item, lease, expiry, or private-ledger state.
    refusal_result(Refusal::Unauthorized)
}
