//! Offline operator commands of the `medulla-server` binary.
//!
//! They run instead of the service: the process parses the command, does the
//! work against a stopped engine's data directory, prints a report whose last
//! line is one JSON object, and exits with the report's status.

use std::path::PathBuf;

use medulla_persistence_ops::{
    store_upgrade,
    store_upgrade::{ApplyOptions, RebindOptions},
};

#[derive(clap::Subcommand, Debug)]
pub(super) enum OperatorCommand {
    /// Inspect or apply the registered offline store-format upgrades of a
    /// stopped engine's data directory.
    StoreUpgrade {
        #[command(subcommand)]
        verb: StoreUpgradeVerb,
    },
}

#[derive(clap::Subcommand, Debug)]
pub(super) enum StoreUpgradeVerb {
    /// List every store, its format, and whether a registered upgrade applies.
    /// Read-only. Exit status: 0 nothing to do, 10 upgrade available, 20 a
    /// store is in a format this build neither opens nor upgrades, 30 a store
    /// could not be read, 1 refused.
    Inspect {
        /// The engine's persistence directory.
        #[arg(env = "GRAPH_SERVICE_PERSIST_DIR", value_name = "DATA_DIR")]
        data_dir: PathBuf,
    },
    /// Run every applicable registered upgrade in place. The engine must be
    /// stopped. Exit status: 0 done or nothing to do, 20 a store remains that
    /// this build neither opens nor upgrades, 30 a store could not be read and
    /// no upgrade admitted it, 1 refused or an upgrade failed.
    Apply {
        /// The engine's persistence directory.
        #[arg(env = "GRAPH_SERVICE_PERSIST_DIR", value_name = "DATA_DIR")]
        data_dir: PathBuf,
        /// Required: confirm that store files may be upgraded in place.
        #[arg(long)]
        confirm: bool,
        /// Private local directory for the scratch copy an inspection reads:
        /// mode 0700, inside a mode 0700 directory, not under /tmp. Defaults
        /// to one this command creates inside the data directory.
        #[arg(long, value_name = "DIRECTORY")]
        staging_dir: Option<PathBuf>,
        /// Disk budget of one store's scratch copy. Defaults to the store's
        /// size plus a quarter and 64 MiB.
        #[arg(long, value_name = "BYTES")]
        max_store_bytes: Option<u64>,
    },
    #[cfg(feature = "graph-copy")]
    /// Copy one graph of a stopped engine's data directory to an object-store root with a
    /// verified whole-graph digest (MEDULLA-OBJECT-STORAGE-BACKEND-R003.2). Refuses
    /// `GRAPH_COPY_DIGEST_MISMATCH` (the destination is left empty) and
    /// `GRAPH_COPY_DESTINATION_NOT_EMPTY` (nothing is written). Exit status: 0 done,
    /// 1 refused.
    GraphCopy {
        /// The engine's persistence directory.
        #[arg(env = "GRAPH_SERVICE_PERSIST_DIR", value_name = "DATA_DIR")]
        data_dir: PathBuf,
        /// The graph to copy.
        graph: String,
        /// The tenant the graph's object keys carry.
        #[arg(long, default_value = "default")]
        tenant: String,
        /// The object root: a local directory, or `s3://bucket[/prefix]`.
        #[arg(long, value_name = "OBJECT_ROOT")]
        to: String,
        /// Required: confirm that the object root may be written.
        #[arg(long)]
        confirm: bool,
    },
    /// Print one line per owner-store layout, in registry order:
    /// `<snake_case layout name> <pinned layout digest>`
    /// (MEDULLA-OBJECT-STORAGE-BACKEND-R002.3). Read-only; opens no store.
    /// Exit status: 0.
    Layouts,
    /// Re-bind a data directory that was copied from another one. Each store
    /// binds to the device and inode of its file, so a copy refuses the ordinary
    /// open. The engine must be stopped. A store that already carries the root of
    /// its own path is skipped. Exit status: 0 done or nothing to do, 1 refused.
    Rebind {
        /// The copied data directory.
        #[arg(env = "GRAPH_SERVICE_PERSIST_DIR", value_name = "DATA_DIR")]
        data_dir: PathBuf,
        /// The data directory the copy was made from. Its stores are read only.
        #[arg(long, value_name = "SOURCE_DATA_DIR")]
        copied_from: PathBuf,
        /// Required: confirm that the copied store roots may be rewritten.
        #[arg(long)]
        confirm: bool,
    },
}

/// Run an offline operator command instead of the service, when the command
/// line asked for one. Returns only when it did not.
pub(super) fn exit_if_requested(command: Option<OperatorCommand>) {
    if let Some(command) = command {
        std::process::exit(command.run());
    }
}

impl OperatorCommand {
    /// Run the command, print its report, and return the exit status.
    pub(super) fn run(self) -> i32 {
        let Self::StoreUpgrade { verb } = self;
        if matches!(verb, StoreUpgradeVerb::Layouts) {
            for line in store_upgrade::layouts_listing() {
                println!("{line}");
            }
            return store_upgrade::EXIT_NOTHING_TO_DO;
        }
        let report = match verb {
            StoreUpgradeVerb::Inspect { data_dir } => store_upgrade::inspect(&data_dir),
            StoreUpgradeVerb::Apply {
                data_dir,
                confirm,
                staging_dir,
                max_store_bytes,
            } => store_upgrade::apply(
                &data_dir,
                &ApplyOptions {
                    confirm,
                    staging_dir,
                    max_store_bytes,
                },
            ),
            #[cfg(feature = "graph-copy")]
            StoreUpgradeVerb::GraphCopy {
                data_dir,
                graph,
                tenant,
                to,
                confirm,
            } => store_upgrade::graph_copy::graph_copy(
                &data_dir,
                &store_upgrade::graph_copy::GraphCopyOptions {
                    graph,
                    tenant,
                    to,
                    confirm,
                },
            ),
            StoreUpgradeVerb::Layouts => return store_upgrade::EXIT_NOTHING_TO_DO,
            StoreUpgradeVerb::Rebind {
                data_dir,
                copied_from,
                confirm,
            } => store_upgrade::rebind(
                &data_dir,
                &RebindOptions {
                    copied_from,
                    confirm,
                },
            ),
        };
        print!("{}", report.render());
        if let Some(error) = &report.error {
            eprintln!("error: store-upgrade {} refused: {error}", report.verb);
        }
        report.exit_code
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The real command line of the server binary.
    type Cli = super::super::Args;

    #[test]
    fn the_documented_command_lines_parse() {
        let inspect = Cli::try_parse_from(["server", "store-upgrade", "inspect", "/data"]).unwrap();
        assert!(matches!(
            inspect.command,
            Some(OperatorCommand::StoreUpgrade {
                verb: StoreUpgradeVerb::Inspect { .. }
            })
        ));
        let apply = Cli::try_parse_from(["server", "store-upgrade", "apply", "/data", "--confirm"])
            .unwrap();
        assert!(matches!(
            apply.command,
            Some(OperatorCommand::StoreUpgrade {
                verb: StoreUpgradeVerb::Apply { confirm: true, .. }
            })
        ));
        let rebind = Cli::try_parse_from([
            "server",
            "store-upgrade",
            "rebind",
            "/copy",
            "--copied-from",
            "/data",
            "--confirm",
        ])
        .unwrap();
        assert!(matches!(
            rebind.command,
            Some(OperatorCommand::StoreUpgrade {
                verb: StoreUpgradeVerb::Rebind { confirm: true, .. }
            })
        ));
        assert!(Cli::try_parse_from(["server"]).unwrap().command.is_none());
    }

    #[test]
    fn apply_without_confirmation_is_refused_before_anything_is_opened() {
        let command = OperatorCommand::StoreUpgrade {
            verb: StoreUpgradeVerb::Apply {
                data_dir: PathBuf::from("/nonexistent-store-upgrade-directory"),
                confirm: false,
                staging_dir: None,
                max_store_bytes: None,
            },
        };
        assert_eq!(command.run(), store_upgrade::EXIT_FAILED);
    }

    // spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002.3
    #[test]
    fn layouts_verb_prints_every_pinned_digest() {
        let parsed = Cli::try_parse_from(["server", "store-upgrade", "layouts"]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(OperatorCommand::StoreUpgrade {
                verb: StoreUpgradeVerb::Layouts
            })
        ));
        let lines = store_upgrade::layouts_listing();
        assert_eq!(lines.len(), medulla_storage::ALL_LAYOUTS.len());
        for (line, layout) in lines.iter().zip(medulla_storage::ALL_LAYOUTS) {
            let (name, digest) = line.split_once(' ').unwrap();
            assert_eq!(name, layout.canonical_name());
            assert_eq!(digest, medulla_storage::pinned_layout_digest(layout));
        }
        let command = OperatorCommand::StoreUpgrade {
            verb: StoreUpgradeVerb::Layouts,
        };
        assert_eq!(command.run(), store_upgrade::EXIT_NOTHING_TO_DO);
    }
}
