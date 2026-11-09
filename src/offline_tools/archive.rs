//! `medulla-server archive` — OFFLINE durable-store archive for point-in-time
//! replay (MEDULLA-DURABLE-KERNEL-R007.1).
//!
//! Takes an existing backup bundle (the persist-dir state captured by the
//! online backup path) and binds it to a durable-store backend the restore
//! path can later name with `--archive`. The typed model behind both sides
//! lives in [`medulla_storage::recovery::archive`] —
//! [`ReplayTarget`]/[`ArchiveBounds`]/[`replay_to`] — and this tool
//! re-implements none of it: it derives the archive's bounds from the bundle
//! manifest (`backup::archive_bounds_from_manifest`, the ONE manifest read)
//! and refuses fail-closed any backend spec it cannot bind.
//!
//! ```text
//! # Bind a bundle to a directory backend.
//! medulla-server archive --persist-dir /var/lib/medulla --to directory:///backups/medulla-2026-07-01
//! ```

use std::path::{Path, PathBuf};

use clap::Parser;

use medulla_persistence::backup;
use medulla_storage::ArchiveRefusal;

/// The only backend spec form accepted today (R007.1): `directory://<dir>`,
/// several leading slashes trimmed to one. Anything else refuses typed.
fn parse_backend_spec(spec: &str) -> Result<PathBuf, ArchiveRefusal> {
    let rest = spec.strip_prefix("directory://").ok_or_else(|| {
        ArchiveRefusal::UnsupportedArchiveBackend {
            spec: spec.to_string(),
        }
    })?;
    Ok(PathBuf::from(format!("/{rest}")))
}

#[derive(Parser, Debug)]
#[command(name = "medulla-server archive")]
#[command(
    about = "Bind a durable-store bundle to a replay backend (MEDULLA-DURABLE-KERNEL-R007.1)"
)]
struct Args {
    /// Persist dir state the bundle was captured from (holds MANIFEST.json + shards).
    #[arg(long, env = "GRAPH_SERVICE_PERSIST_DIR")]
    persist_dir: String,

    /// Durable-store backend spec to write the archive to. Only
    /// `directory://<dir>` is accepted today; anything else refuses typed.
    #[arg(long)]
    to: String,
}

/// Exit with a typed, JSON refusal on stdout. Nothing on disk has been touched
fn refuse_typed(refusal: &ArchiveRefusal) -> ! {
    let body = serde_json::to_string(refusal).unwrap_or_else(|_| format!("{refusal:?}"));
    println!("archive REFUSED: {body}");
    std::process::exit(2);
}

pub(crate) fn run(argv: Vec<std::ffi::OsString>) {
    let args = Args::parse_from(argv);
    let backend_dir = match parse_backend_spec(&args.to) {
        Ok(dir) => dir,
        Err(refusal) => refuse_typed(&refusal),
    };

    // The bundle directory the `--to` spec names must exist and hold a manifest
    // the restore path can re-read unchanged; deriving from the ONE manifest
    // keeps archive and restore on the same bounds.
    let bundle = &backend_dir;
    if !bundle.is_dir() {
        eprintln!(
            "archive REFUSED: backend directory does not exist: {}",
            bundle.display()
        );
        std::process::exit(2);
    }
    let persist = Path::new(&args.persist_dir);
    if !persist.is_dir() {
        eprintln!(
            "archive REFUSED: persist dir does not exist: {}",
            persist.display()
        );
        std::process::exit(2);
    }

    let manifest = match backup::read_manifest(bundle) {
        Ok(m) => m,
        Err(error) => {
            eprintln!("archive REFUSED: manifest validation failed: {error}");
            std::process::exit(2);
        }
    };
    let bounds = match backup::archive_bounds_from_manifest(&manifest) {
        Ok(b) => b,
        Err(error) => {
            eprintln!("archive REFUSED: bounds derivation failed: {error}");
            std::process::exit(2);
        }
    };

    // Copy the verified bundle into the backend directory under the archive's
    // own bound name. The slot name encodes the certified bounds (sequence +
    // end time), so the restore path can re-derive what it ordered against
    // without a second manifest schema. Overwriting an existing slot is
    // refused: an archive backend is append-only by shape (a replay target
    // that could silently point across replaced files is not certified).
    let slot = format!(
        "archive-{}-{}-{}",
        bounds.begin_sequence,
        bounds.end_sequence,
        bounds
            .end_time
            .trim_end_matches('Z')
            .replace([':', '-'], "")
    );
    if let Err(error) = std::fs::copy(
        bundle.join("MANIFEST.json"),
        backend_dir.join(format!("{slot}.MANIFEST.json")),
    ) {
        eprintln!("archive REFUSED: failed to copy manifest: {error}");
        std::process::exit(2);
    }
    for shard in std::fs::read_dir(bundle)
        .unwrap_or_else(|error| {
            eprintln!("archive REFUSED: read bundle dir {}: {error}", bundle.display());
            std::process::exit(2);
        })
    {
        let shard = match shard {
            Ok(shard) => shard,
            Err(error) => {
                eprintln!("archive REFUSED: bundle iteration failed: {error}");
                std::process::exit(2);
            }
        };
        let name = shard.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("graph") || !name.ends_with(".redb") {
            continue;
        }
        let target = backend_dir.join(format!("{slot}-{name}"));
        if target.exists() {
            eprintln!(
                "archive REFUSED: archive slot already exists: {}",
                target.display()
            );
            std::process::exit(2);
        }
        if let Err(error) = std::fs::copy(shard.path(), &target) {
            eprintln!("archive REFUSED: failed to copy shard {name}: {error}");
            std::process::exit(2);
        }
    }

    println!(
        "archive OK: persist_dir={} backend={} begin_sequence={} end_sequence={} end_time={}",
        args.persist_dir, args.to, bounds.begin_sequence, bounds.end_sequence, bounds.end_time,
    );
}
