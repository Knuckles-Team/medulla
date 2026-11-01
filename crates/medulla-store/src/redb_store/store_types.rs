use super::store_prelude::*;
use super::*;

pub fn decode_durable<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    medulla_types::msgpack::decode_bounded(bytes, durable_msgpack_limits())
        .map_err(|_| "durable value is invalid or exceeds resource limits".to_string())
}

pub fn decode_mutation_batch_record(
    bytes: &[u8],
    expected_graph_fname: &str,
    expected_batch_id: &str,
) -> Result<MutationBatchRecord, String> {
    let record: MutationBatchRecord = decode_durable(bytes)?;
    record.validate()?;
    validate_graph_mutation_record_binding(&record, expected_graph_fname, expected_batch_id)?;
    Ok(record)
}

pub fn validate_graph_mutation_record_binding(
    record: &MutationBatchRecord,
    expected_graph_fname: &str,
    expected_batch_id: &str,
) -> Result<(), String> {
    let graph_name = match (
        record.status,
        record.identity.scope(),
        record.committed_version,
    ) {
        (
            MutationBatchStatus::Committed,
            MutationScope::Graph { graph, .. },
            CommittedVersion::Graph { .. },
        ) => graph.as_str(),
        _ => return Err("graph mutation store contains a non-graph committed record".to_string()),
    };
    if sanitize(graph_name) != expected_graph_fname {
        return Err("graph mutation record does not match its requested graph route".to_string());
    }
    if record.batch.batch_id != expected_batch_id {
        return Err("graph mutation record does not match its physical lookup key".to_string());
    }
    Ok(())
}

pub fn decode_mutation_outbox_record(bytes: &[u8]) -> Result<MutationOutboxRecord, String> {
    let record: MutationOutboxRecord = decode_durable(bytes)?;
    record.validate()?;
    if !matches!(record.committed_version, CommittedVersion::Graph { source, .. } if source != 0) {
        return Err("graph mutation store contains a non-graph outbox record".to_string());
    }
    Ok(record)
}

pub fn decode_mutation_projection_cursor(
    bytes: &[u8],
) -> Result<MutationProjectionCursor, String> {
    let cursor: MutationProjectionCursor = decode_durable(bytes)?;
    cursor.validate()?;
    if !matches!(cursor.committed_version, CommittedVersion::Graph { source, .. } if source != 0) {
        return Err("graph mutation store contains a non-graph projection cursor".to_string());
    }
    Ok(cursor)
}

pub fn validate_graph_mutation_record_sizes(
    plaintext: &[u8],
    stored: &[u8],
) -> Result<(), String> {
    if plaintext.len() > MAX_DURABLE_MSGPACK_BYTES {
        return Err("graph mutation record exceeds durable resource limits".to_string());
    }
    if stored.len() > MAX_DURABLE_STORED_BYTES {
        return Err("sealed graph mutation record exceeds durable resource limits".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DurableOutboxDelivery {
    consumer: String,
    lease_epoch: u64,
    lease_until_ms: u64,
    attempt: u32,
    delivered_at_ms: Option<u64>,
}


pub const NODES: TableDefinition<(&str, &str), &[u8]> = TableDefinition::new("nodes");
pub const EDGES: TableDefinition<(&str, &str, &str, u32), &[u8]> = TableDefinition::new("edges");
pub const LEDGER: TableDefinition<(&str, u64), &str> = TableDefinition::new("ledger");
pub const SEMANTIC: TableDefinition<&str, &[u8]> = TableDefinition::new("semantic_store");
// Tamper-evident hash-chained audit log (CONCEPT:MEDULLA-KG.sharding.row-level-security, feature `security`). One
// row per durable mutation, keyed `(graph, seq)`, value = `prev_hash | entry_hash |
// line` (see `medulla_kernel::audit`). Appended in the SAME WriteTransaction as the mutation
// it records, so the audit entry and the data it audits are durable together. The
// table const is always defined (so the layout is stable) but only WRITTEN/READ under
// `security`.
pub const AUDIT: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("audit_chain");
/// Per-graph idempotency index for application audit appends. It commits with
/// the corresponding hash-chain entry and contains only a digest and receipt.
pub const AUDIT_REQUESTS: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("audit_requests");
// Provenance-anchor MEMBER list (CONCEPT:MEDULLA-KG.sharding.row-level-security, provenance anchoring): `(graph,
// audit_seq) -> msgpack Vec<(node_id, leaf_hash_bytes)>` for the `:ToolCall`/`:RunTrace`
// window folded into the audit-chain entry at that exact `seq` (a
// `PROVENANCE_ANCHOR|...` line, see `medulla_kernel::audit::provenance_anchor_line`). Keyed
// by the SAME seq as its audit entry so the two correlate with no extra index. The
// anchored ROOT itself is never trusted from this table -- only from the
// tamper-evident AUDIT entry at that seq -- so tampering this side table cannot
// forge a passing inclusion proof; it can only make an otherwise-valid proof fail
// closed (see `crate::redb_store::prove_inclusion`).
pub const PROVENANCE_ANCHOR_MEMBERS: TableDefinition<(&str, u64), &[u8]> =
    TableDefinition::new("provenance_anchor_members");
pub const GRAPH_META: TableDefinition<&str, &[u8]> = TableDefinition::new("graph_meta");
/// Monotonic native WorkItem command sequence, scoped by graph.  The sequence
/// is allocated in the same transaction as the submitted WorkItem row and its
/// mutation/outbox record, so replay never allocates a second command number.
pub const WORK_ITEM_COMMAND_SEQUENCE: TableDefinition<&str, u64> =
    TableDefinition::new("work_item_command_sequence");
/// Native reservation identity, keyed `(graph, reservation_id)`.
pub const RESOURCE_RESERVATIONS: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("resource_reservations");
/// Tenant-scoped reservation keyset for bounded status pagination. The value
/// repeats the reservation id so a repaired index can be validated without
/// trusting a caller-provided cursor.
pub const RESOURCE_RESERVATION_TENANT_INDEX: TableDefinition<(&str, &str, &str), &str> =
    TableDefinition::new("resource_reservation_tenant_index");
/// One immutable reservation winner per `(graph, WorkItem, attempt)`.
pub const RESOURCE_RESERVATION_ATTEMPTS: TableDefinition<(&str, &str, u64), &str> =
    TableDefinition::new("resource_reservation_attempts");
/// Native host capacity/telemetry and held accounting.
pub const RESOURCE_HOSTS: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("resource_hosts");
/// Global repository/branch/concurrency ownership keys.
pub const RESOURCE_EXCLUSIVITY: TableDefinition<(&str, &str), &str> =
    TableDefinition::new("resource_exclusivity");
/// Native fairness service debt by tenant/group.
pub const RESOURCE_FAIRNESS: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("resource_fairness");
/// Exact active count for each global concurrency key.  The count is updated
/// with the reservation row, so admission never scans an unbounded history.
pub const RESOURCE_CONCURRENCY: TableDefinition<(&str, &str), u64> =
    TableDefinition::new("resource_concurrency");
/// Host-local anti-affinity tag counts, updated with reservation lifecycle rows.
pub const RESOURCE_ANTI_AFFINITY: TableDefinition<(&str, &str, &str), u64> =
    TableDefinition::new("resource_anti_affinity");
/// Per-host/profile disk hysteresis.  The blocked bit is native policy state,
/// never a caller-provided telemetry field, and is retained across restarts.
pub const RESOURCE_DISK_POLICIES: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("resource_disk_policies");
/// Complete governed envelope retained for replay/audit reconciliation.
pub const CHANGE_ENVELOPES: TableDefinition<(&str, &str), &[u8]> =
    TableDefinition::new("change_envelopes");
/// Current content version, scoped by verified tenant and owning graph.
pub const CONTENT_VERSIONS: TableDefinition<(&str, &str, &str), &[u8]> =
    TableDefinition::new("content_versions");
/// Current typed source cursor. No component is converted into a filename.
pub const CHANGE_CURSORS: TableDefinition<(&str, &str, &str, &str), &[u8]> =
    TableDefinition::new("change_cursors");
pub const CHANGE_BLOBS: TableDefinition<(&str, &str, &str), &[u8]> =
    TableDefinition::new("change_blobs");
pub const CHANGE_FEATURES: TableDefinition<(&str, &str, &str), &[u8]> =
    TableDefinition::new("change_features");
pub const CHANGE_EVIDENCE: TableDefinition<(&str, &str, &str), &[u8]> =
    TableDefinition::new("change_evidence");
pub const CHANGE_POLICIES: TableDefinition<(&str, &str, &str), &[u8]> =
    TableDefinition::new("change_policies");
pub const CHANGE_LINEAGE: TableDefinition<(&str, &str, &str), &[u8]> =
    TableDefinition::new("change_lineage");
// Durable Raft log table (CONCEPT:MEDULLA-KG.storage.one-fsync-covers-raft). Defined here so `commit_ops` (shared
// with the server's group-commit writer, which folds replicated log appends into
// the SAME `WriteTransaction` as graph mutations) is self-contained. The embedded
// path never appends log ops, so this table stays empty for an embedded-only DB —
// the const costs nothing and keeps the two callers on one durable layout.
pub const RAFT_LOG: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("raft_log");
// Cross-shard 2PC prepare records (CONCEPT:MEDULLA-KG.storage.lane-n-increment). One row per participant group
// of an in-flight cross-shard transaction, keyed by `(txn_id, group_id)`, holding
// that group's PREPARED-but-not-applied slice (its staged write-set). Durable so an
// in-doubt txn survives a coordinator/participant crash between PREPARE and COMMIT
// and is resolved on restart. Lives in the authoritative shard for the same-file reason
// as the Raft log; the PURE put/clear/scan logic lives here (shared store) next to
// NODES/EDGES/purge_graph_rows, while the writer-thread `Cmd` arms in `redb_backend`
// call into it (mirrors how the graph-row machinery is shared, CONCEPT:MEDULLA-KG.backend.engine-modes).
pub const XSHARD_PREPARE: TableDefinition<(&str, u64), &[u8]> =
    TableDefinition::new("xshard_prepare");
// The coordinator's durable DECISION record for a cross-shard txn, keyed by `txn_id`
// (CONCEPT:MEDULLA-KG.storage.lane-n-increment). Values `0/1` are ordinary ABORT/COMMIT;
// `2/3` are ABORT/COMMIT retained for a separate MutationBatch parent; `4` is a
// recoverable attempt started before phase 1 but not yet decided. Writing a terminal
// value is the ATOMIC COMMIT POINT: once it reads COMMIT every participant applies on
// recovery; absent/pending/ABORT ⇒ no new participant applies (presumed-abort).
pub const XSHARD_DECISION: TableDefinition<&str, u8> = TableDefinition::new("xshard_decision");
// Named distributed-compute MATERIALIZED VIEWS (CONCEPT:MEDULLA-KG.storage.feature). One row per matview
// keyed by `name`, holding the MessagePack-serialized `MatView` (its definition +
// current result rows). Durable so a matview survives restart; the handler reloads the
// in-RAM `MatViewStore` from this table on boot and refreshes incrementally on a delta.
// Lives in the authoritative shard for the same-file reason as the Raft log + xshard rows.
pub const MATVIEWS: TableDefinition<&str, &[u8]> = TableDefinition::new("matviews");

// Named PLAN-BACKED materialized views (CONCEPT:MEDULLA-KG.storage.plan-backed-matview). One
// row per matview keyed by `name`, holding the MessagePack-serialized plan-backed
// DEFINITION (name + target graph + `wire::Plan` bytes + reorder hint) — NOT the result
// rows, which ride the version-keyed result cache. DISJOINT table from the algo-only
// `matviews` table above (and from Lane D's secondary-index tables): a distinct redb
// table name, so the two matview families and the index rows never collide. Reloaded into
// the in-RAM plan-matview manager on boot.
pub const PLAN_MATVIEWS: TableDefinition<&str, &[u8]> = TableDefinition::new("plan_matviews");

// DBSP INCREMENTAL operator state (CONCEPT:MEDULLA-KG.storage.incremental-matview): the durable
// snapshot of an incrementally-maintained plan-backed matview's circuit state (its
// membership map / per-bucket accumulators + CDC watermark), keyed by view name. The
// direct analogue of turso's `dbsp_state` btree, scoped down to redb. DISJOINT from
// `plan_matviews` (that table holds the DEFINITION; this holds the maintained STATE).
// Written when an incremental view is defined and dropped with it.
pub const MATVIEW_OPERATOR_STATE: TableDefinition<&str, &[u8]> =
    TableDefinition::new("matview_operator_state");


/// Sweep one scope-prefixed owner table for the capability's own scope.
pub fn retire_scoped_table<K, V>(
    write: &medulla_storage::PhysicalWriteCapability<'_, GraphShardOwner>,
    definition: TableDefinition<'static, K, V>,
) -> Result<(), String>
where
    K: redb::Key + 'static,
    for<'k> K::SelfType<'k>: medulla_storage::OwnerRowScope,
    V: redb::Value + 'static,
{
    write.scoped_owner_table_mut(definition)?.purge_scope_rows()
}


/// In-doubt cross-shard prepare records `(txn_id, group_id, slice-blob)` returned by
/// the recovery scan (CONCEPT:MEDULLA-KG.storage.lane-n-increment).
pub type XshardPrepareScan = Result<Vec<(String, u64, Vec<u8>)>, String>;
/// Durable 2PC decisions `(opaque_parent_id, outcome, retained_for_parent)`.
/// `outcome=None` is the recoverable protocol-start marker (not yet decided).
pub type XshardDecisionScan = Result<Vec<(String, Option<bool>, bool)>, String>;

/// Persisted materialized views `(name, blob)` returned by the boot reload scan
/// (CONCEPT:MEDULLA-KG.storage.feature). Shared by the algo-only distributed matview
/// (`compute-dist`) and the plan-backed incremental matview (`matview`) scan surfaces.
#[cfg(any(feature = "compute-dist", feature = "matview"))]
pub type MatViewScanResult = Result<Vec<(String, Vec<u8>)>, String>;

/// Encryption-at-rest cipher handle threaded through the durable read/write paths
/// (CONCEPT:MEDULLA-KG.sharding.row-level-security). A thin wrapper so the SAME function signatures carry it
/// whether or not the `security` feature is compiled: without `security` it is a
/// zero-sized no-op (every `seal`/`unseal` is the identity), so the durable format
/// and code path are byte-for-byte unchanged; with `security` + a configured key it
/// holds the AEAD that seals value blobs on write and unseals on read.
#[derive(Clone, Copy, Default)]
pub struct DurableCrypto<'a> {
    #[cfg(feature = "security")]
    cipher: Option<&'a medulla_kernel::crypto::ValueCipher>,
    #[cfg(not(feature = "security"))]
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> DurableCrypto<'a> {
    /// A no-op handle (encryption off / not compiled).
    pub fn none() -> Self {
        DurableCrypto::default()
    }

    /// Wrap an optional cipher (the `security` path).
    #[cfg(feature = "security")]
    pub fn new(cipher: Option<&'a medulla_kernel::crypto::ValueCipher>) -> Self {
        DurableCrypto { cipher }
    }

    /// Seal a value blob for storage. Identity when no cipher is active.
    ///
    /// Returns `Cow` so the **encryption-OFF** path (the default — no key configured)
    /// BORROWS the caller's plaintext with ZERO allocation/copy (CONCEPT:MEDULLA-KG.storage.redb-store #4): the
    /// bytes are handed straight to redb's `insert` via `as_ref()`, byte-for-byte
    /// identical to what `plaintext.to_vec()` produced before, so the on-disk format is
    /// unchanged. Only when a cipher IS active does it allocate the owned ciphertext
    /// (seal + encrypt, behavior unchanged). This removes a per-op heap allocation +
    /// memcpy of every node/edge/property value blob from inside the held write txn on
    /// the CPU-bound writer thread, for the common at-rest-encryption-off deployment.
    #[inline]
    pub fn seal<'b>(&self, plaintext: &'b [u8]) -> Cow<'b, [u8]> {
        #[cfg(feature = "security")]
        if let Some(c) = self.cipher {
            return Cow::Owned(c.seal(plaintext));
        }
        Cow::Borrowed(plaintext)
    }

    /// Unseal a stored value blob. Plaintext is accepted only in a deployment whose
    /// configured current format is plaintext. A sealed blob without its cipher and
    /// an unsealed blob with an active cipher both fail closed.
    #[inline]
    pub fn unseal(&self, stored: &[u8]) -> Result<Vec<u8>, String> {
        self.open(stored)
            .map_err(|failure| failure.message().to_string())
    }

    /// [`Self::unseal`] for one node row: a failure names the row it was
    /// found on (EH-384), so a read or dump of a corrupt node fails closed with
    /// `NODE_UNREADABLE:` and the graph and node id.
    #[inline]
    pub fn unseal_node(
        &self,
        graph: &str,
        node_id: &str,
        stored: &[u8],
    ) -> Result<Vec<u8>, NodeUnreadable> {
        self.open(stored)
            .map_err(|cause| NodeUnreadable::new(graph, node_id, cause))
    }

    /// The typed form every unseal goes through: the closed reason a stored
    /// blob cannot be opened, or its plaintext.
    pub fn open(&self, stored: &[u8]) -> Result<Vec<u8>, UnsealFailure> {
        if stored.len() > MAX_DURABLE_STORED_BYTES {
            return Err(UnsealFailure::Oversize);
        }
        #[cfg(feature = "security")]
        if let Some(c) = self.cipher {
            return open_sealed(c, stored);
        }
        #[cfg(feature = "security")]
        if medulla_kernel::crypto::is_sealed(stored) {
            return Err(UnsealFailure::SealedWithoutKey);
        }
        Ok(stored.to_vec())
    }
}

/// Open a blob under an active cipher: framing first, then AEAD, then the
/// plaintext bound.
#[cfg(feature = "security")]
fn open_sealed(
    cipher: &medulla_kernel::crypto::ValueCipher,
    stored: &[u8],
) -> Result<Vec<u8>, UnsealFailure> {
    if !medulla_kernel::crypto::is_sealed(stored) {
        return Err(UnsealFailure::MissingFraming);
    }
    let plaintext = cipher
        .unseal(stored)
        .map_err(|_| UnsealFailure::Authentication)?;
    if plaintext.len() > MAX_DURABLE_MSGPACK_BYTES {
        return Err(UnsealFailure::Oversize);
    }
    Ok(plaintext)
}
