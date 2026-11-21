//! The scheduler policy owner table (MEDULLA-DURABLE-KERNEL-R076.2).
//!
//! One row, keyed `scheduler_config`, valued with the JSON `SchedulerConfig` that
//! `SetSchedulerConfig` validated. `medulla-storage` declares the table; the store
//! in `medulla-persistence` only names it.

use redb::TableDefinition;

/// `row key -> json(SchedulerConfig)`.
pub const SCHEDULER_CONFIG: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("scheduler_config");
