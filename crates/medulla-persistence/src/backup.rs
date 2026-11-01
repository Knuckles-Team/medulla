//! Online consistent BACKUP + RESTORE + PITR foundation (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore).
//!
//! ## What it solves
//!
//! MEDULLA-030 (`shard_migrate`) can copy a durable store verbatim, but only OFFLINE — the
//! engine must be stopped because it opens each authoritative shard exclusively. A
//! disaster-recovery story needs a **consistent backup taken while the engine RUNS**,
//! and a matching restore. This is that.
//!
//! ## Online consistent backup — no stop-the-world, and no table list
//!
//! [`RedbBackend::backup`](super::redb_backend::RedbBackend::backup) calls
//! `write_bundle_shard` once per shard, which is one call to
//! [`medulla_storage::backup_recovery_store`] on that shard file's `StorageKernel`.
//! The kernel takes its own MVCC snapshot (CONCEPT:MEDULLA-KG.storage.snapshot-read-off-writer) of the LIVE writer's
//! store — redb 4.1 is MVCC, so it sees the shard's LATEST COMMITTED state and runs
//! CONCURRENTLY with the single writer (no writer involvement, no group-commit, no
//! quiesce) — and copies, into a fresh bundle file:
//!
//! * every table of the DECLARED `OwnerLayout::GraphShard` census
//!   (`copy_declared_owner_tables`), and
//! * every ledger table of the authoritative list (`visit_ledger_content_tables!`),
//!   rebinding each logical scope to the destination's own physical root.
//!
//! That census is the whole point. Unlike `shard_migrate`/`online_reshard`, which route
//! rows by graph, **nothing here is graph-filtered — the whole shard moves as one unit**,
//! which is exactly the operation the kernel already owns. WD5-BUG-04 (and the
//! BUG-CX-016/BUG-CX-054 class before it) was a hand-maintained copy list that silently
//! omitted 30 tables; a list nobody maintains cannot omit anything, so "no table is
//! silently missed" stopped being a property this module asserts and became one it
//! cannot violate. The same census carries, for free and without a line of code here:
//!
//! * value blobs byte-for-byte (no decode/unseal), so encryption-at-rest survives
//!   WITHOUT the key and the tamper-evident hash-chained `audit_chain`
//!   (CONCEPT:MEDULLA-KG.sharding.row-level-security) stays verifiable;
//! * `encryption_canary` — a declared FileWide owner table — so a restore retains the
//!   original key identity/version boundary and cannot silently establish a new canary
//!   under a different key;
//! * the file-wide Raft log/meta, the cross-shard 2PC records and the matview key
//!   spaces, so the shard-0-only special case is gone as well;
//! * the MutationKernel ledger (receipts, idempotency, versions, fences, outbox,
//!   deliveries, cursors, replay evidence, classes) — the shard's own retired private
//!   `mutation_*` tables no longer exist, and the ledger that replaced them is copied by
//!   the same authoritative list rather than by eight more hand-written copies.
//!
//! `backup_recovery_store` validates the destination before returning, so a bundle shard
//! is a proven-recoverable owner file, and its [`RecoveryStoreCounts`] census is what the
//! manifest records per shard.
//!
//! **Cross-shard consistency** rides the commit-before-ack guarantee (CONCEPT:MEDULLA-KG.backend.authoritative-dispatch):
//! any ACKED write is already durably committed, so each per-shard snapshot — opened
//! independently — sees a self-consistent committed prefix of the durable history. The
//! backup brackets those copies with cryptographic change tokens for both the admin
//! saga ledger ([`medulla_storage::recovery_store_fingerprint`]) and the shard's cross-shard
//! prepare/decision records (`xshard_recovery_boundary`). If either recovery boundary
//! changes, no manifest is published and the caller retries. A stable prepared parent
//! is safe because its authenticated recovery plan remains available for idempotent
//! startup replay.
//!
//! ## Bundle format — a portable shard set + manifest
//!
//! A backup bundle is a directory holding:
//!
//! * `graph-<n>.redb` — one verbatim kernel owner file per shard for every K,
//!   using the MEDULLA-026 [`shard_filename`](super::redb_backend::shard_filename) names, so
//!   the bundle IS a valid durable shard set on its own.
//! * `admin-mutations.redb` — the portable local projection of placement-group
//!   consensus: admin coordinator receipts, fences, child outbox rows and
//!   authenticated encrypted plans for prepared parents.
//! * the NON-SHARD durable stores a restore is incomplete without — `rbac.redb`
//!   (roles, grants, registered identities, bootstrap lifecycle), `kv.redb`,
//!   `node_info.redb` and `catalog.redb`. See [`super::durable_stores`].
//! * `MANIFEST.json` — [`BackupManifest`]: format version, engine version, shard count K,
//!   caller-supplied timestamp, opaque label reference, the per-shard recovery census,
//!   exact portable-file digests, the `bundled_stores` inventory, and the
//!   `excluded_stores` map naming every durable store this bundle deliberately does NOT
//!   carry, WITH ITS REASON.
//!
//! ## Scope is declared, never implied (BUG-PE-054)
//!
//! Before this the bundle held graph shards + `admin-mutations.redb` and NOTHING else,
//! while the manifest described only what it had captured. A restore therefore came up
//! with no RBAC/identity state at all — a backup that silently cannot restore identity
//! is worse than no backup, because it is trusted. redb's exclusive per-file lock means
//! this module can never simply open a sibling store, so the two halves of the fix are:
//! the caller hands in the live handles it owns ([`medulla_server_api::api::persistence::BundledStoreSource`]),
//! and everything still left out is NAMED IN THE MANIFEST with the reason it was left
//! out. `durable_stores`' `registry_covers_every_redb_store` test is what stops a future
//! `.redb` from being silently forgotten again.
//!
//! ## Restore — verbatim import, re-shard-on-restore
//!
//! [`restore_bundle`] validates the manifest — including re-deriving every bundle
//! shard's census from the FILE and comparing it to what the manifest claims — then
//! rebuilds a persist-dir from the bundle by DELEGATING to MEDULLA-030's
//! [`shard_migrate::migrate_shards`]: the bundle's shard files are exactly the canonical
//! `graph-<n>.redb` set that tool consumes. Restoring at the manifest's own K is a 1:1
//! verbatim row import; restoring at a DIFFERENT K re-shards on restore (each graph
//! re-routed by the SAME MEDULLA-026 `FNV-1a % K`). No decode/re-derive — the audit chain and
//! at-rest ciphertext survive the round trip.
//!
//! ## Point-in-time recovery (PITR)
//!
//! The bundle plus the durable ledger/WAL tail are the low-RPO/RTO DR primitives:
//! restore the latest bundle, then replay the durable ledger tail forward to a target
//! instant. The replay-to-timestamp mechanism is documented in `docs/deployment.md`
//! ("Point-in-time recovery"); this module builds the backup + restore halves it rides.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use medulla_storage::RecoveryStoreCounts;
use redb::ReadableTable;
use sha2::{Digest, Sha256};

use crate::shard_migrate;
use medulla_store::redb_store::shard::Shard;
use medulla_store::redb_store::XSHARD_DECISION;
use medulla_store::redb_store::XSHARD_PREPARE;

/// The bundle manifest file name.
pub const MANIFEST_FILE: &str = "MANIFEST.json";

/// The backup bundle on-disk format version. Bumped only on an incompatible layout
/// change; `restore_bundle` refuses a newer format than it understands.
///
/// Version 5 is the kernel-census manifest: the hand-counted row dimensions
/// (`nodes`/`edges`/`ledger`/`semantic`/`audit`/`auxiliary`/`global`/
/// `capability_and_resource`) are replaced by [`BackupManifest::shard_counts`], the
/// per-shard [`RecoveryStoreCounts`] the kernel derives from the declared census. A
/// version-4 bundle cannot be read as one, and must not be: its counters described a
/// copy list that no longer exists.
pub const BUNDLE_FORMAT_VERSION: u32 = 5;

/// Separate coordinator store file captured with every portable bundle.
pub const ADMIN_MUTATIONS_FILE: &str = "admin-mutations.redb";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_BACKUP_SHARDS: usize = 64;

/// The cross-shard recovery boundary of ONE shard file: a cryptographic change token
/// over the in-doubt participant records and retained coordinator decisions, plus the
/// count of each.
///
/// A backup compares the whole boundary before and after its per-shard copies. Any
/// prepare/decision transition makes the in-progress bundle unpublished, rather than
/// presenting a fuzzy cross-shard cut as a recovery point. The counts ride along
/// because they are read from the same two tables in the same pass — the operator
/// receipt reports "how many in-doubt records is this bundle carrying", and reading
/// them twice would be a second census of the same rows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct XshardRecoveryBoundary {
    pub(crate) fingerprint: [u8; 32],
    /// In-doubt cross-shard participant prepare records.
    pub(crate) prepares: u64,
    /// Retained cross-shard coordinator decisions.
    pub(crate) decisions: u64,
}

/// Read one shard file's cross-shard recovery boundary.
///
/// `xshard_prepare` and `xshard_decision` are FileWide (`TableScope::StorePrivate`)
/// tables, so this is a control-scope read: the boundary belongs to the file, not to
/// any one graph. This stays hand-written — deliberately — because it must be NARROW.
/// `recovery_store_fingerprint` over the shard's whole ledger would also change on
/// every ordinary committed write, which would make an online backup of a serving
/// shard unpublishable by construction; the boundary that invalidates a bundle is a
/// 2PC transition, not any write at all.
pub(crate) fn xshard_recovery_boundary(shard: &Shard) -> Result<XshardRecoveryBoundary, String> {
    fn field(hasher: &mut Sha256, value: &[u8]) {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value);
    }

    let read = shard.control_read()?;
    let mut hasher = Sha256::new();
    let mut prepares = 0u64;
    let mut decisions = 0u64;
    field(&mut hasher, b"prepare");
    let prepare = read.open_owner_table(XSHARD_PREPARE)?;
    for row in prepare.iter().map_err(|error| error.to_string())? {
        let (key, value) = row.map_err(|error| error.to_string())?;
        let (transaction_id, group_id) = key.value();
        field(&mut hasher, transaction_id.as_bytes());
        field(&mut hasher, &group_id.to_be_bytes());
        field(&mut hasher, value.value());
        prepares = prepares.saturating_add(1);
    }
    drop(prepare);
    field(&mut hasher, b"decision");
    let decision = read.open_owner_table(XSHARD_DECISION)?;
    for row in decision.iter().map_err(|error| error.to_string())? {
        let (key, value) = row.map_err(|error| error.to_string())?;
        field(&mut hasher, key.value().as_bytes());
        field(&mut hasher, &[value.value()]);
        decisions = decisions.saturating_add(1);
    }
    drop(decision);
    Ok(XshardRecoveryBoundary {
        fingerprint: hasher.finalize().into(),
        prepares,
        decisions,
    })
}

/// Stable digest-only view used by the online-backup bracket.  Keep the
/// prepare/decision census in [`xshard_recovery_boundary`] as the single read
/// implementation; callers that only need the change token must not duplicate
/// its table walk.
pub(crate) fn xshard_recovery_fingerprint(shard: &Shard) -> Result<[u8; 32], String> {
    Ok(xshard_recovery_boundary(shard)?.fingerprint)
}

/// Outcome of a backup run (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore) — the shard count + the census the kernel
/// derived for every file the bundle carries.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BackupReport {
    /// Number of shard files written into the bundle (= K).
    pub shards: usize,
    /// Per-shard recovery census, in shard order, exactly as
    /// [`medulla_storage::backup_recovery_store`] validated each bundle file.
    ///
    /// This is the ONE counter. The dimensions the hand-written copy loops used to
    /// increment (`nodes`, `edges`, `audit`, `capability_and_resource`, …) were a
    /// second census of the same rows, maintained by the same list that WD5-BUG-04
    /// found 30 tables missing from; a backup's completeness is now read back from the
    /// file it wrote instead of reported by the loop that wrote it.
    pub shard_counts: Vec<RecoveryStoreCounts>,
    /// In-doubt cross-shard participant prepare records the bundle carries.
    pub xshard_prepares: u64,
    /// Retained cross-shard coordinator decisions the bundle carries.
    pub xshard_decisions: u64,
    /// Integrity totals for the separate admin coordinator ledger.
    pub admin_mutations: RecoveryStoreCounts,
    /// Non-shard durable stores copied into the bundle, as `file name → rows copied`
    /// (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore).
    pub bundled_stores: BTreeMap<String, u64>,
    /// Stable, non-secret encryption key identity captured from the live store.
    /// Material is never included in a report or bundle manifest.
    pub encryption_key_id: Option<String>,
    pub encryption_key_version: Option<String>,
}

impl BackupReport {
    /// Fold one shard's census into the report, in shard order.
    pub fn add_shard(&mut self, counts: RecoveryStoreCounts) {
        self.shard_counts.push(counts);
    }

    /// Record one shard's cross-shard recovery boundary counts.
    pub(crate) fn set_xshard_boundary(&mut self, boundary: XshardRecoveryBoundary) {
        self.xshard_prepares = boundary.prepares;
        self.xshard_decisions = boundary.decisions;
    }

    /// Distinct graph scopes captured across every shard.
    pub fn graph_scopes(&self) -> u64 {
        graph_scopes(&self.shard_counts)
    }
}

/// Distinct graph scopes a shard set carries.
///
/// Every shard file binds exactly one reserved control scope at open — that is what
/// lets the boot scan read the catalog before any graph is known — and one serving
/// scope per graph it hosts. So the graph count IS the census: total scope bindings
/// minus one control scope per file. Unlike a row total it is invariant across a
/// re-shard, because re-sharding moves a graph's scope between files without creating
/// or destroying one.
pub fn graph_scopes(counts: &[RecoveryStoreCounts]) -> u64 {
    counts.iter().fold(0u64, |total, shard| {
        total.saturating_add(shard.scope_bindings.saturating_sub(1))
    })
}

/// The bundle manifest (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore) — serialized to `MANIFEST.json` at backup and
/// validated at restore. All non-derived fields (timestamp, label reference, engine version) are
/// CALLER-SUPPLIED — this module never calls `Date::now` (no wall-clock / randomness in
/// library code).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    /// On-disk bundle format version ([`BUNDLE_FORMAT_VERSION`]).
    pub format_version: u32,
    /// Engine version that produced the bundle (caller-supplied, e.g. `CARGO_PKG_VERSION`).
    pub engine_version: String,
    /// Shard count K — the number of `graph*.redb` files in the bundle.
    pub shard_count: usize,
    /// Caller-supplied Unix-seconds timestamp of the backup.
    pub timestamp: u64,
    /// Opaque SHA-256 reference to the caller-supplied label (including empty).
    pub label_ref: String,
    /// Per-shard recovery census, in shard order. [`read_manifest`] re-derives this
    /// from the bundle files themselves and refuses a manifest that disagrees, so the
    /// declaration is checkable rather than merely recorded.
    pub shard_counts: Vec<RecoveryStoreCounts>,
    /// Retained in-doubt participant records needed by recovery.
    pub xshard_prepares: u64,
    /// Retained coordinator decisions needed to resolve prepared participants.
    pub xshard_decisions: u64,
    /// Integrity totals for the separate admin coordinator ledger.
    pub admin_mutations: RecoveryStoreCounts,
    /// Stable, non-secret encryption key identity required to open this bundle.
    /// `None` means the source store used plaintext values; no key material is ever
    /// written to the manifest.
    #[serde(default)]
    pub encryption_key_id: Option<String>,
    #[serde(default)]
    pub encryption_key_version: Option<String>,
    /// Non-shard durable stores captured in this bundle, as `file name → rows copied`.
    #[serde(default)]
    pub bundled_stores: BTreeMap<String, u64>,
    /// Durable stores this bundle deliberately does NOT capture, as
    /// `file name → reason` (from
    /// [`durable_stores::DURABLE_STORES`](super::durable_stores::DURABLE_STORES)).
    /// A bundle that documents its own scope cannot silently mislead an operator into
    /// trusting a restore it was never able to perform.
    #[serde(default)]
    pub excluded_stores: BTreeMap<String, String>,
    /// Exact SHA-256 for every portable graph shard and coordinator store.
    /// Keys are bundle-local generic file names, never host paths.
    pub file_digests: BTreeMap<String, String>,
}

impl BackupManifest {
    /// Distinct graph scopes this bundle carries: total scope bindings minus one
    /// reserved control scope per shard file, which is invariant across a re-shard.
    pub fn graph_scopes(&self) -> u64 {
        graph_scopes(&self.shard_counts)
    }

    fn from_report(
        report: &BackupReport,
        engine_version: &str,
        timestamp: u64,
        label: &str,
        file_digests: BTreeMap<String, String>,
    ) -> Self {
        Self {
            format_version: BUNDLE_FORMAT_VERSION,
            engine_version: engine_version.to_string(),
            shard_count: report.shards,
            timestamp,
            label_ref: opaque_text_ref(label),
            shard_counts: report.shard_counts.clone(),
            xshard_prepares: report.xshard_prepares,
            xshard_decisions: report.xshard_decisions,
            admin_mutations: report.admin_mutations,
            encryption_key_id: report.encryption_key_id.clone(),
            encryption_key_version: report.encryption_key_version.clone(),
            bundled_stores: report.bundled_stores.clone(),
            excluded_stores: super::durable_stores::excluded_store_reasons(),
            file_digests,
        }
    }
}

pub fn opaque_text_ref(value: &str) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(value.as_bytes());
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    format!("sha256:{encoded}")
}

fn is_sha256_ref(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_key_metadata_component(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

pub fn file_sha256(path: &Path) -> Result<String, String> {
    let mut stream = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let count = stream
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn portable_file_digests(
    dir: &Path,
    shard_files: &[PathBuf],
    bundled_stores: &BTreeMap<String, u64>,
) -> Result<BTreeMap<String, String>, String> {
    let mut files = shard_files.to_vec();
    files.push(dir.join(ADMIN_MUTATIONS_FILE));
    for name in bundled_stores.keys() {
        files.push(dir.join(name));
    }
    let mut digests = BTreeMap::new();
    for path in files {
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| "backup contains an unavailable portable file".to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("backup portable files must be regular files".to_string());
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "backup contains a non-portable file name".to_string())?
            .to_string();
        if digests.insert(name, file_sha256(&path)?).is_some() {
            return Err("backup contains a duplicate portable file name".to_string());
        }
    }
    Ok(digests)
}

/// Re-derive the recovery census of a shard set from the FILES, in shard order.
///
/// Opened READ-ONLY, exactly as the coordinator store is: a bundle's bytes are what
/// [`portable_file_digests`] hashes, so validating one may never open a write
/// transaction against it. `open_read_only` derives the store's physical identity from
/// `(dev, ino)` only, so a bundle staged under a different path still validates.
fn shard_census(shard_files: &[PathBuf]) -> Result<Vec<RecoveryStoreCounts>, String> {
    let mut census = Vec::with_capacity(shard_files.len());
    for path in shard_files {
        let store = medulla_storage::open_read_only(path, None).map_err(|error| {
            format!(
                "shard {} is not a readable owner file: {error}",
                path.display()
            )
        })?;
        census.push(medulla_storage::validate_recovery_store_read_only(&store)?);
    }
    Ok(census)
}

/// Copy ONE shard file into `dst_path` (a fresh bundle file) and return the census the
/// kernel validated on it (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore). Called once per shard by
/// [`RedbBackend::backup`](super::redb_backend::RedbBackend::backup).
///
/// The whole shard moves as one unit — no `is_shard0` special case, because the
/// file-wide Raft, 2PC and matview tables are declared owner tables of the same census
/// as the per-graph ones, and a census does not need to be told which file it is
/// looking at.
pub(crate) fn write_bundle_shard(
    source: &Shard,
    dst_path: &Path,
) -> Result<RecoveryStoreCounts, String> {
    medulla_storage::backup_recovery_store(source.kernel(), dst_path)
        .map_err(|error| format!("bundle shard {}: {error}", dst_path.display()))
}

/// Serialize + write the bundle manifest to `<dir>/MANIFEST.json` (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore).
pub(crate) fn write_manifest(
    dir: &Path,
    report: &BackupReport,
    engine_version: &str,
    timestamp: u64,
    label: &str,
) -> Result<BackupManifest, String> {
    let final_path = dir.join(MANIFEST_FILE);
    if final_path.exists() {
        return Err("backup manifest already exists (refusing to overwrite)".to_string());
    }
    let shard_files = medulla_store::redb_layout::discover_current_shards(dir)?;
    if shard_files.len() != report.shards || report.shard_counts.len() != report.shards {
        return Err("backup shard-file count changed before publication".to_string());
    }
    let file_digests = portable_file_digests(dir, &shard_files, &report.bundled_stores)?;
    let manifest =
        BackupManifest::from_report(report, engine_version, timestamp, label, file_digests);
    let json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let temporary_path = dir.join(".MANIFEST.json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|error| error.to_string())?;
    if let Err(error) = file.write_all(&json).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error.to_string());
    }
    drop(file);
    if let Err(error) = std::fs::rename(&temporary_path, &final_path) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error.to_string());
    }
    #[cfg(unix)]
    std::fs::File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())?;
    Ok(manifest)
}

fn read_manifest_file(dir: &Path) -> Result<BackupManifest, String> {
    let path = dir.join(MANIFEST_FILE);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|_| "read bundle manifest failed".to_string())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_MANIFEST_BYTES
    {
        return Err("backup manifest must be a bounded regular file".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(&path)
        .and_then(|file| file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| "read bundle manifest failed".to_string())?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("backup manifest exceeds the size limit".to_string());
    }
    serde_json::from_slice(&bytes).map_err(|_| "parse bundle manifest failed".to_string())
}

fn validate_manifest_header(manifest: &BackupManifest) -> Result<(), String> {
    if manifest.format_version != BUNDLE_FORMAT_VERSION {
        return Err(format!(
            "bundle format version {} is not the portable coordinator-aware format ({})",
            manifest.format_version, BUNDLE_FORMAT_VERSION
        ));
    }
    if !is_sha256_ref(&manifest.label_ref) {
        return Err("backup label reference is not an opaque digest".to_string());
    }
    if manifest.shard_count == 0 || manifest.shard_count > MAX_BACKUP_SHARDS {
        return Err("backup manifest shard count is outside the supported range".to_string());
    }
    if manifest.engine_version.is_empty()
        || manifest.engine_version.len() > 128
        || manifest.engine_version.chars().any(char::is_control)
    {
        return Err("backup manifest engine version is invalid".to_string());
    }
    Ok(())
}

fn validate_manifest_security(manifest: &BackupManifest) -> Result<(), String> {
    match (
        manifest.encryption_key_id.as_deref(),
        manifest.encryption_key_version.as_deref(),
    ) {
        (Some(id), Some(version))
            if valid_key_metadata_component(id, 128)
                && valid_key_metadata_component(version, 64) => {}
        (None, None) => {}
        _ => return Err("backup manifest encryption key reference is invalid".to_string()),
    }
    Ok(())
}

fn validate_manifest_digest_inventory(manifest: &BackupManifest) -> Result<(), String> {
    if manifest.file_digests.len() != manifest.shard_count + 1 + manifest.bundled_stores.len()
        || manifest
            .file_digests
            .values()
            .any(|digest| !is_sha256_ref(digest))
    {
        return Err("backup manifest file digest inventory is invalid".to_string());
    }
    Ok(())
}

fn validate_bundled_store_declarations(manifest: &BackupManifest) -> Result<(), String> {
    // Every declared bundled store must be a KNOWN durable store, and must not also be
    // declared excluded — otherwise the manifest describes a scope it does not have.
    for name in manifest.bundled_stores.keys() {
        match super::durable_stores::lookup(name) {
            Some(store) if matches!(store.scope, super::durable_stores::BackupScope::Bundled) => {}
            _ => {
                return Err(
                    "backup manifest declares a store this build does not bundle".to_string(),
                )
            }
        }
        if manifest.excluded_stores.contains_key(name) {
            return Err("backup manifest declares a store both bundled and excluded".to_string());
        }
    }
    Ok(())
}

fn validate_manifest_shards(dir: &Path, manifest: &BackupManifest) -> Result<Vec<PathBuf>, String> {
    let shard_files = medulla_store::redb_layout::discover_current_shards(dir)?;
    if shard_files.len() != manifest.shard_count
        || manifest.shard_counts.len() != manifest.shard_count
    {
        return Err("backup shard-file count does not match the manifest".to_string());
    }
    // The bundle's own completeness, re-derived from the shard FILES rather than
    // trusted from the manifest: every declared owner table present, every ledger row
    // resolving its scope binding, and the same census the backup recorded. This is
    // where the row-dimension cross-check that used to live in the restore path went —
    // measured from the artifact instead of self-reported by the copy that produced it.
    if shard_census(&shard_files)? != manifest.shard_counts {
        return Err("bundle shard totals do not match the manifest".to_string());
    }
    Ok(shard_files)
}

fn validate_manifest_admin_store(dir: &Path, manifest: &BackupManifest) -> Result<(), String> {
    let admin_path = dir.join(ADMIN_MUTATIONS_FILE);
    let admin_metadata = std::fs::symlink_metadata(&admin_path)
        .map_err(|_| "backup bundle omits the admin mutation coordinator store".to_string())?;
    if admin_metadata.file_type().is_symlink() || !admin_metadata.is_file() {
        return Err("backup bundle omits the admin mutation coordinator store".to_string());
    }
    // SEC-FINDING-V1-INCARNATION-BREAKS-RESTORE-20260903, resolved: this bundle
    // may have been renamed here from a different private staging path than the
    // one it was written at, and it must be validated WITHOUT mutating it —
    // its bytes are exactly what the `portable_file_digests` check in
    // `validate_manifest_file_digests` re-hashes and compares against the
    // manifest. `open_read_only` derives the store's physical identity from
    // `(dev, ino)` only (never the path, so a rename doesn't change it) and never
    // opens a write transaction against the file, so validating it here cannot
    // perturb the digest check that follows.
    let admin = medulla_storage::open_read_only(
        &admin_path,
        crate::redb_backend::admin_mutations_private_integrity(),
    )?;
    let counts = medulla_storage::validate_recovery_store_read_only(&admin)?;
    if counts != manifest.admin_mutations {
        return Err("admin mutation coordinator totals do not match the manifest".to_string());
    }
    drop(admin);
    Ok(())
}

fn validate_manifest_file_digests(
    dir: &Path,
    shard_files: &[PathBuf],
    manifest: &BackupManifest,
) -> Result<(), String> {
    let actual_digests = portable_file_digests(dir, shard_files, &manifest.bundled_stores)?;
    if actual_digests != manifest.file_digests {
        return Err("backup portable-file digests do not match the manifest".to_string());
    }
    Ok(())
}

/// Read + validate a bundle manifest from `<dir>/MANIFEST.json` (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore).
pub fn read_manifest(dir: &Path) -> Result<BackupManifest, String> {
    let manifest = read_manifest_file(dir)?;
    validate_manifest_header(&manifest)?;
    validate_manifest_security(&manifest)?;
    validate_manifest_digest_inventory(&manifest)?;
    validate_bundled_store_declarations(&manifest)?;
    let shard_files = validate_manifest_shards(dir, &manifest)?;
    validate_manifest_admin_store(dir, &manifest)?;
    validate_manifest_file_digests(dir, &shard_files, &manifest)?;
    Ok(manifest)
}

/// Outcome of a restore (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore) — the validated manifest + the verbatim import
/// totals produced by the MEDULLA-030 migration engine.
#[derive(Debug, Clone)]
pub struct RestoreReport {
    /// The bundle's validated manifest.
    pub manifest: BackupManifest,
    /// Explicit shard count the persist-dir was rebuilt at.
    pub restored_shards: usize,
    /// Verbatim row-import totals (MEDULLA-030 `MigrationReport`).
    pub migration: shard_migrate::MigrationReport,
    /// Recovery census re-derived from the RESTORED shard files, in shard order.
    pub restored_counts: Vec<RecoveryStoreCounts>,
    /// Validated coordinator receipts and encrypted staged recovery plans.
    pub admin_mutations: RecoveryStoreCounts,
    /// Non-shard durable store files copied back into the persist dir.
    pub restored_stores: Vec<String>,
}

#[cfg(feature = "security")]
fn validate_restore_key(manifest: &BackupManifest) -> Result<(), String> {
    if let Some(configured) = medulla_kernel::crypto::resolve_key_config()? {
        match (
            manifest.encryption_key_id.as_deref(),
            manifest.encryption_key_version.as_deref(),
        ) {
            (Some(id), Some(version))
                if id == configured.key_ref().id.as_str()
                    && version == configured.key_ref().version.as_str() => {}
            (Some(_), Some(_)) => {
                return Err(
                    "restore key reference does not match the backup; configure the original \
                     key identity/version or complete an explicit offline re-encryption \
                     rotation before restore"
                        .to_string(),
                );
            }
            (None, None) => {
                return Err(
                    "restore bundle has no encryption key reference but the current \
                     deployment supplied an encryption key; refuse ambiguous restore"
                        .to_string(),
                );
            }
            _ => return Err("restore bundle encryption key reference is incomplete".to_string()),
        }
    }
    Ok(())
}

fn restore_graph_shards(
    bundle_dir: &Path,
    persist_dir: &Path,
    manifest: &BackupManifest,
    target_shards: usize,
) -> Result<(shard_migrate::MigrationReport, Vec<RecoveryStoreCounts>), String> {
    // The bundle's graph*.redb files are exactly the source shard set MEDULLA-030 consumes.
    let migration = shard_migrate::migrate_shards(bundle_dir, persist_dir, target_shards)?;
    if migration.source_shards != manifest.shard_count {
        return Err("restored graph totals do not match the backup manifest".to_string());
    }
    // The restore's own output, measured: every target shard is a recoverable owner
    // file, and it carries exactly the graph scopes the bundle declared. A row total
    // cannot be compared across a re-shard (the routing moves rows between files), but
    // a SCOPE cannot be created or destroyed by re-routing one, so this is the
    // completeness claim that survives a K change — and it is read back from the
    // rebuilt files rather than reported by the migration about itself.
    let restored_counts = shard_census(&medulla_store::redb_layout::discover_current_shards(
        persist_dir,
    )?)?;
    if restored_counts.len() != target_shards
        || graph_scopes(&restored_counts) != manifest.graph_scopes()
    {
        return Err("restored graph scopes do not match the backup manifest".to_string());
    }
    Ok((migration, restored_counts))
}

fn restore_admin_store(
    bundle_dir: &Path,
    persist_dir: &Path,
    manifest: &BackupManifest,
) -> Result<(medulla_storage::StorageKernel, RecoveryStoreCounts), String> {
    let admin_source = bundle_dir.join(ADMIN_MUTATIONS_FILE);
    let admin_target = persist_dir.join(ADMIN_MUTATIONS_FILE);
    if admin_target.exists() {
        return Err("restore target already contains an admin mutation store".to_string());
    }
    std::fs::copy(&admin_source, &admin_target).map_err(|error| error.to_string())?;
    // SEC-FINDING-V1-INCARNATION-BREAKS-RESTORE-20260903: the `std::fs::copy` above
    // always allocates a NEW inode, so the copy's incarnation can never match the one
    // the bundle was stamped with and every ordinary open fails closed. A restore is
    // an INTENDED substitution and needs the kernel's explicit staged adoption, which
    // re-anchors the physical root and every scope binding to the new inode after
    // proving the image is byte-identical to what it validated.
    let restored_admin = adopt_bundled_store(
        &admin_target,
        ADMIN_MUTATIONS_FILE,
        crate::redb_backend::admin_mutations_private_integrity(),
    )?
    .ok_or_else(|| "admin mutation store is not a declared bundled owner".to_string())?;
    let admin_mutations = medulla_storage::validate_recovery_store(&restored_admin)?;
    if admin_mutations != manifest.admin_mutations {
        return Err("restored admin mutation coordinator totals changed".to_string());
    }
    Ok((restored_admin, admin_mutations))
}

fn restore_bundled_stores(
    bundle_dir: &Path,
    persist_dir: &Path,
    manifest: &BackupManifest,
) -> Result<Vec<String>, String> {
    // Restore every non-shard durable store the bundle carried. Without this an engine
    // rebuilt from a bundle comes up with no RBAC/identity state at all — the failure
    // this whole scope declaration exists to make impossible.
    let mut restored_stores = Vec::new();
    for name in manifest.bundled_stores.keys() {
        let target = persist_dir.join(name);
        if target.exists() {
            return Err("restore target already contains a bundled durable store".to_string());
        }
        std::fs::copy(bundle_dir.join(name), &target).map_err(|error| error.to_string())?;
        // A copied file is a NEW inode, so a kernel-owned bundled store carries an
        // incarnation that no longer describes it and its owner's ordinary `open`
        // fails closed (what BUG-PE-054's reopen assertion caught). Adoption belongs
        // here, at the restore boundary. The expected authority is DECLARED by
        // `durable_stores::bundled_store_authority` — the kernel no longer infers it.
        adopt_bundled_store(&target, name, None)
            .map_err(|error| format!("restore adopt {name}: {error}"))?;
        restored_stores.push(name.clone());
    }
    Ok(restored_stores)
}

/// Rebuild a persist-dir from a backup bundle (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore). Validates the manifest,
/// then verbatim-imports every bundle shard into `persist_dir` by delegating to MEDULLA-030's
/// [`shard_migrate::migrate_shards`] (the bundle IS a valid `graph*.redb` shard set).
///
/// `target_shards` is mandatory. Passing the manifest's own K performs a 1:1 import;
/// passing a different K performs RE-SHARD ON RESTORE through the same MEDULLA-026
/// `FNV-1a % K` routing rule.
///
/// `persist_dir` must not already hold target shard files (the migration refuses to
/// clobber) — restore into a FRESH dir. OFFLINE with respect to the TARGET: nothing may
/// be serving out of `persist_dir` while it is rebuilt.
pub fn restore_bundle(
    bundle_dir: &Path,
    persist_dir: &Path,
    target_shards: usize,
) -> Result<RestoreReport, String> {
    let manifest = read_manifest(bundle_dir)?;
    #[cfg(feature = "security")]
    validate_restore_key(&manifest)?;
    if !(1..=64).contains(&target_shards) {
        return Err("restore target shard count is outside bounds".to_string());
    }
    let k = target_shards;
    let (migration, restored_counts) = restore_graph_shards(bundle_dir, persist_dir, &manifest, k)?;
    let (_restored_admin, restored_admin_counts) =
        restore_admin_store(bundle_dir, persist_dir, &manifest)?;
    let restored_stores = restore_bundled_stores(bundle_dir, persist_dir, &manifest)?;
    Ok(RestoreReport {
        manifest,
        restored_shards: k,
        migration,
        restored_counts,
        admin_mutations: restored_admin_counts,
        restored_stores,
    })
}

/// Adopt one restored bundled store into its new inode.
///
/// Returns `None` for a bundled file that declares no kernel owner authority —
/// there is none today, and the `None` arm is what makes a future plain bundled
/// file fail loudly at its own call site rather than being adopted as something
/// it is not.
fn adopt_bundled_store(
    path: &Path,
    file_name: &str,
    private_integrity: Option<std::sync::Arc<dyn medulla_storage::PrivatePayloadIntegrity>>,
) -> Result<Option<medulla_storage::StorageKernel>, String> {
    let Some((physical_name, layout)) = super::durable_stores::bundled_store_authority(file_name)
    else {
        return Ok(None);
    };
    let staged = medulla_storage::inspect_staged_mutation_store(
        path,
        medulla_storage::PhysicalStoreIdentity::new(physical_name)?,
        layout,
        private_integrity,
    )?;
    medulla_storage::adopt_staged_mutation_store(staged).map(Some)
}

/// Set one environment variable for the duration of a test and restore its
/// previous value on drop — including on panic, which a bare set/remove pair
/// would leak into every later test in the same process. Parameterised on the
/// var name so both the at-rest encryption key (`crypto::ENCRYPTION_KEY_ENV`,
/// this module's own coverage) and the transaction-recovery key
/// (`crypto::TXN_RECOVERY_KEY_ENV`, `handlers::txn`'s keyed-recovery coverage)
/// share the one guard: `Some(previous) => restore it`, `None => remove the
/// var entirely`.
#[cfg(any(test, feature = "test-support"))]
pub struct EnvVarGuard {
    key: &'static str,
    // `OsString`, not `String`: an environment value that is not valid UTF-8 must
    // still be restored exactly. Reading it through `var()` would drop such a
    // value on the floor and the guard would silently unset it instead.
    previous: Option<std::ffi::OsString>,
}

#[cfg(any(test, feature = "test-support"))]
impl EnvVarGuard {
    pub fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

