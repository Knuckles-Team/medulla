//! The startup report owner table (MEDULLA-OBSERVE-R008.2).
//!
//! One row per boot, keyed by the boot sequence number and valued with the MessagePack of that
//! boot's startup report. The last `observe.startup_reports_kept` rows are kept. `medulla-storage`
//! declares the table; the store in `medulla-persistence` only names it.

use redb::TableDefinition;

/// `boot_seq -> msgpack(StartupReport)`.
pub const OBSERVE_STARTUP_REPORT: TableDefinition<'static, u64, &[u8]> =
    TableDefinition::new("observe_startup_report");
