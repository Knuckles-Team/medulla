//! The graph-row table bundle one admitted member writes through.
use medulla_store::redb_store::store_prelude::*;
use medulla_store::redb_store::*;

/// Translate ONE applied method into redb row writes inside an open transaction.
/// Mirrors `crate::mutation_apply::apply`'s method set: the durable DATA mutations only.
/// The row handles every graph-row writer of one member needs, opened once.
///
/// `redb` refuses a second open of a table whose first handle is still alive,
/// and every graph member of a group writes the same `nodes`, so these are
/// opened per member and dropped before the next member's. Bundling them is
/// what keeps the writers below inside the argument cap without threading six
/// same-shaped handles positionally through every one of them.
pub struct GraphRowTables<'g> {
    pub run_checkpoints:
        ScopedOwnerTableMut<'g, (&'static str, &'static str), &'static [u8]>,
    pub current_plans: ScopedOwnerTableMut<'g, (&'static str, &'static str), &'static [u8]>,
    pub nodes: ScopedOwnerTableMut<'g, (&'static str, &'static str), &'static [u8]>,
    pub edges:
        ScopedOwnerTableMut<'g, (&'static str, &'static str, &'static str, u32), &'static [u8]>,
    pub ledger: ScopedOwnerTableMut<'g, (&'static str, u64), &'static str>,
    pub semantic: ScopedOwnerTableMut<'g, &'static str, &'static [u8]>,
    pub command_sequences: ScopedOwnerTableMut<'g, &'static str, u64>,
    pub native_work_items:
        ScopedOwnerTableMut<'g, (&'static str, &'static str), &'static [u8]>,
    #[cfg(feature = "security")]
    pub audit: ScopedOwnerTableMut<'g, (&'static str, u64), &'static [u8]>,
}

/// Borrowed form used by the native operation loop, which already has several
/// table guards open for resource and lane operations. It lets the shared graph
/// row applier enforce the same scope boundary without opening a second handle
/// to any table that the caller owns.
pub struct GraphRowTablesRef<'a, 'n, 'e, 'l, 's, 'w>
where
    'n: 'a,
    'e: 'a,
    'l: 'a,
    's: 'a,
    'w: 'a,
{
    pub nodes: &'a mut ScopedOwnerTableMut<'n, (&'static str, &'static str), &'static [u8]>,
    pub edges: &'a mut ScopedOwnerTableMut<
        'e,
        (&'static str, &'static str, &'static str, u32),
        &'static [u8],
    >,
    pub ledger: &'a mut ScopedOwnerTableMut<'l, (&'static str, u64), &'static str>,
    pub semantic: &'a mut ScopedOwnerTableMut<'s, &'static str, &'static [u8]>,
    pub native_work_items:
        &'a mut ScopedOwnerTableMut<'w, (&'static str, &'static str), &'static [u8]>,
}

impl<'g> GraphRowTables<'g> {
    /// Open one member's graph-row tables, bounded to that member's own scope.
    pub fn open<'txn>(
        member: &'g AdmittedOwnerWrite<'txn, GraphShardOwner>,
    ) -> Result<Self, String> {
        Ok(Self {
            nodes: member.open_scoped_table(NODES)?,
            run_checkpoints: member
                .open_scoped_table(super::work_item::checkpoint::RUN_CHECKPOINTS)?,
            current_plans: member
                .open_scoped_table(super::work_item::current_plan::CURRENT_PLANS)?,
            edges: member.open_scoped_table(EDGES)?,
            ledger: member.open_scoped_table(LEDGER)?,
            semantic: member.open_scoped_table(SEMANTIC)?,
            command_sequences: member.open_scoped_table(WORK_ITEM_COMMAND_SEQUENCE)?,
            native_work_items: member.open_scoped_table(super::work_item_capability::NATIVE_WORK_ITEMS)?,
            #[cfg(feature = "security")]
            audit: member.open_scoped_table(AUDIT)?,
        })
    }
}
