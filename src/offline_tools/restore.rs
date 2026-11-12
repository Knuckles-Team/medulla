//! `medulla-server restore` — rebuild a durable persist-dir from an MEDULLA-090 backup bundle (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore).
//!
//! Reads a backup bundle produced by the online backup path (`Method::Backup` /
//! `RedbBackend::backup`) — a directory of verbatim `graph-<n>.redb` shard
//! files + a `MANIFEST.json` — and rebuilds a fresh persist-dir from it, verbatim (value
//! blobs, encryption-at-rest ciphertext, and the KG-2.231 audit chain are copied
//! byte-for-byte). Mirrors the offline `migrate-shards` CLI (CONCEPT:MEDULLA-KG.sharding.atomic-shard-swap) — restore
//! delegates to that same verbatim-import engine.
//!
//! Run with NOTHING serving out of `--persist-dir` (redb holds an exclusive per-file
//! lock); `--persist-dir` must be empty of target shard files (it refuses to clobber).
//!
//! Point-in-time boundary (MEDULLA-DURABLE-KERNEL-R007.1): `--to-seq N` or
//! `--to-time <RFC3339>` (exactly one) names a typed
//! [`medulla_storage::ReplayTarget`] the restore must replay to; the refusals
//! are typed ([`medulla_storage::ArchiveRefusal`]) and happen before any
//! destination effect, so a refused restore writes no shard file. Without a
//! target the restore behaves exactly as before. `--archive` names the
//! durable-store backend spec the archive is bound to; only `directory://<bundle-dir>`
//! is accepted (the bundle directory itself carries the archive), anything
//! else refuses fail-closed.
//!
//! ```text
//! # Restore at the bundle's own shard count K (state K explicitly).
//! medulla-server restore --bundle /backups/medulla-2026-07-01 --persist-dir /var/lib/medulla --shards 1
//!
//! # Re-shard on restore: rebuild at a DIFFERENT K (each graph re-routed by MEDULLA-026).
//! medulla-server restore --bundle /backups/medulla-2026-07-01 --persist-dir /var/lib/medulla-k8 --shards 8
//!
//! # Point-in-time restore (exactly one of --to-seq / --to-time).
//! medulla-server restore --bundle /backups/medulla-2026-07-01 --persist-dir /var/lib/medulla \
//!     --shards 1 --archive directory:///backups/medulla-2026-07-01 --to-seq 90
//! ```

use std::path::Path;

use clap::Parser;

use medulla_persistence::backup;

#[derive(Parser, Debug)]
#[command(name = "medulla-server restore")]
#[command(
    about = "Rebuild a durable persist-dir from an MEDULLA-090 backup bundle (CONCEPT:MEDULLA-KG.sharding.reshard-on-restore)"
)]
struct Args {
    /// Backup bundle directory (holds graph*.redb + MANIFEST.json).
    #[arg(long)]
    bundle: String,

    /// Destination persist dir to rebuild (must be free of target shard files).
    #[arg(long, env = "GRAPH_SERVICE_PERSIST_DIR")]
    persist_dir: String,

    /// Required target shard count K. Use the bundle's K for a 1:1 restore or a
    /// different K to re-shard through MEDULLA-026 `FNV-1a % K`.
    #[arg(long)]
    shards: usize,

    /// Durable-store backend spec the archive is bound to
    /// (MEDULLA-DURABLE-KERNEL-R007.1). Only `directory://<bundle-dir>` is
    /// accepted — the bundle directory itself carries the archive; anything
    /// else refuses fail-closed before any destination effect.
    #[arg(long, value_name = "BACKEND_SPEC")]
    archive: Option<String>,

    /// Replay to and including this sequence number (exactly one of
    /// `--to-seq`/`--to-time`).
    #[arg(long, value_name = "SEQ")]
    to_seq: Option<u64>,

    /// Replay to and including this RFC3339 UTC timestamp (exactly one of
    /// `--to-seq`/`--to-time`).
    #[arg(long, value_name = "RFC3339")]
    to_time: Option<String>,
}

/// The accepted `--archive` backend specs (R007.1): a `directory://` URL whose
/// path is the bundle directory (several leading slashes trimmed to one). Any
/// other form refuses: restore never guesses at a backend spec it cannot bind.
fn parse_archive_spec(spec: &str) -> Option<String> {
    spec.strip_prefix("directory://")
        .map(|rest| format!("/{}", rest.trim_start_matches('/')))
}

/// Exit with a typed, JSON refusal on stdout — the refusal is already
/// serialized before this returns, and nothing in the caller has touched the
/// destination (fail-closed by construction).
fn refuse_typed(refusal: &medulla_storage::ArchiveRefusal) -> ! {
    let body = serde_json::to_string(refusal).unwrap_or_else(|_| format!("{refusal:?}"));
    println!("restore REFUSED: {body}");
    std::process::exit(2);
}

pub(crate) fn run(argv: Vec<std::ffi::OsString>) {
    let args = Args::parse_from(argv);

    // Exactly one target (MEDULLA-DURABLE-KERNEL-R007.1): both absent or both
    // present refuse at parse time, typed, before the destination is touched.
    let target = match medulla_storage::parse_replay_target(args.to_seq, args.to_time) {
        Ok(t) => t,
        Err(refusal) => refuse_typed(&refusal),
    };

    let archive_dir = match args.archive.as_deref() {
        Some(spec) => match parse_archive_spec(spec) {
            Some(dir) => Some(dir),
            None => refuse_typed(
                &medulla_storage::ArchiveRefusal::UnsupportedArchiveBackend {
                    spec: spec.to_string(),
                },
            ),
        },
        None => None,
    };
    // The archive binding records where the replay target validates against;
    // `restore_bundle` re-reads the same bundle, so this is belt-and-braces
    // that refuses a spec pointing AWAY from `--bundle` before any effect.
    if let (Some(dir), bundle_path) = (&archive_dir, Path::new(&args.bundle)) {
        if Path::new(dir) != bundle_path {
            refuse_typed(
                &medulla_storage::ArchiveRefusal::UnsupportedArchiveBackend {
                    spec: args.archive.clone().unwrap_or_default(),
                },
            );
        }
    }

    match backup::restore_bundle(
        Path::new(&args.bundle),
        Path::new(&args.persist_dir),
        args.shards,
        target,
    ) {
        Ok(r) => {
            let m = &r.manifest;
            let mig = &r.migration;
            println!(
                "restore OK: label_ref={} K={} -> {} shards | \
                 graphs={} nodes={} edges={} ledger={} semantic={} audit={} global={} \
                 admin_batches={} encrypted_recovery_plans={} xshard_decisions={}",
                m.label_ref,
                m.shard_count,
                r.restored_shards,
                mig.graphs,
                mig.nodes,
                mig.edges,
                mig.ledger,
                mig.semantic,
                mig.audit,
                mig.global,
                r.admin_mutations.batches,
                r.admin_mutations.encrypted_private_payloads,
                m.xshard_decisions,
            );
        }
        Err(e) => {
            // Fail-closed contract: a typed replay refusal (serialized JSON
            // with a `code` field) is reported as-is on stdout; every other
            // failure keeps the legacy opaque-reference stderr form.
            if e.starts_with("{\"code\":") {
                println!("restore REFUSED: {e}");
                std::process::exit(2);
            }
            eprintln!("restore FAILED: error_ref={}", opaque_ref(&e));
            std::process::exit(1);
        }
    }
}

fn opaque_ref(value: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    let digest = Sha256::digest(value.as_bytes());
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    format!("sha256:{encoded}")
}
