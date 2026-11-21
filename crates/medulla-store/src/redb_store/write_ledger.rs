//! The per-graph write ledger and creator (MEDULLA-IDENTITY-R700.7).
//!
//! Two durable facts of the graph's shard file feed the ownership plan:
//!
//! * `write_days`, one counter per `(graph, principal, day)` where `day` is whole days since the
//!   epoch, from which `writes_30d` is summed;
//! * `graph_creator`, the principal that created the graph, recorded once.
//!
//! A bump is part of the commit of the write it counts: the same transaction, so a crash keeps
//! both or neither, and no second commit and no extra graph-version advance.

use redb::TableDefinition;

use super::shard::{Shard, ShardWrite};

/// Writes per `(graph, principal, day)`.
pub const WRITE_DAYS: TableDefinition<(&str, &str, u32), u64> = TableDefinition::new("write_days");
/// The creator principal of each graph, keyed by graph name.
pub const GRAPH_CREATOR: TableDefinition<&str, &str> = TableDefinition::new("graph_creator");

/// Milliseconds in one ledger day.
pub const DAY_MS: u64 = 86_400_000;
/// Ledger rows older than this many days are deleted when a bump opens a new day.
pub const RETENTION_DAYS: u32 = 400;
/// The most principals `write_days` returns.
pub const MAX_WRITERS: usize = 256;

/// The ledger day of a wall-clock time in milliseconds since the epoch.
pub fn day_of(now_ms: u64) -> u32 {
    u32::try_from(now_ms / DAY_MS).unwrap_or(u32::MAX)
}

/// One counted write: `principal` wrote `graph` on ledger `day`. It travels with the write to the
/// commit chokepoint and is applied by [`bump_write_day`] inside that commit's transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCount {
    pub graph: String,
    pub principal: String,
    pub day: u32,
}

/// Apply, inside `write`, every count of `counts` whose graph is one of `graphs`; the others
/// stay for a later chunk of the same drain.
pub fn bump_counts(
    write: &ShardWrite<'_>,
    graphs: &[String],
    counts: &mut Vec<WriteCount>,
) -> Result<(), String> {
    let (mine, rest): (Vec<WriteCount>, Vec<WriteCount>) = std::mem::take(counts)
        .into_iter()
        .partition(|count| graphs.contains(&count.graph));
    *counts = rest;
    for count in &mine {
        bump_write_day(write, &count.graph, &count.principal, count.day)?;
    }
    Ok(())
}

/// Add one write of `principal` on `day` to the ledger of `graph`. A bump that opens a new
/// `(principal, day)` row also deletes the graph's rows older than [`RETENTION_DAYS`], bounded
/// by the row count of that graph.
pub fn bump_write_day(
    write: &ShardWrite<'_>,
    graph: &str,
    principal: &str,
    day: u32,
) -> Result<(), String> {
    let mut rows = write.graph(graph)?.open_scoped_table(WRITE_DAYS)?;
    let current = rows.get((graph, principal, day))?.map(|row| row.value());
    if let Some(count) = current {
        return rows.insert((graph, principal, day), count.saturating_add(1));
    }
    let oldest_kept = day.saturating_sub(RETENTION_DAYS);
    let mut stale: Vec<(String, u32)> = Vec::new();
    for row in rows.scope_rows()? {
        let (key, _) = row?;
        let (_, row_principal, row_day) = key.value();
        if row_day < oldest_kept {
            stale.push((row_principal.to_string(), row_day));
        }
    }
    for (row_principal, row_day) in &stale {
        rows.remove((graph, row_principal.as_str(), *row_day))?;
    }
    rows.insert((graph, principal, day), 1)
}

/// Record `principal` as the creator of `graph`; a graph that already has a creator keeps it.
pub fn record_graph_creator(
    write: &ShardWrite<'_>,
    graph: &str,
    principal: &str,
) -> Result<(), String> {
    let mut rows = write.graph(graph)?.open_scoped_table(GRAPH_CREATOR)?;
    let present = rows.get(graph)?.is_some();
    if present {
        return Ok(());
    }
    rows.insert(graph, principal)
}

/// The writes of each principal on `graph` for days `>= since_day`, summed per principal,
/// highest count first, then principal id, at most [`MAX_WRITERS`] entries.
pub fn write_days(
    shard: &Shard,
    graph: &str,
    since_day: u32,
) -> Result<Vec<(String, u64)>, String> {
    let handle = shard.graph(graph)?;
    let read = shard.read(&handle)?;
    let rows = read.scoped_owner_table(WRITE_DAYS)?;
    let mut totals: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for row in rows.scope_rows()? {
        let (key, value) = row?;
        let (_, principal, day) = key.value();
        if day >= since_day {
            let total = totals.entry(principal.to_string()).or_insert(0);
            *total = total.saturating_add(value.value());
        }
    }
    let mut sorted: Vec<(String, u64)> = totals.into_iter().collect();
    sorted.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    sorted.truncate(MAX_WRITERS);
    Ok(sorted)
}

/// The recorded creator of `graph`, when there is one.
pub fn graph_creator(shard: &Shard, graph: &str) -> Result<Option<String>, String> {
    let handle = shard.graph(graph)?;
    let read = shard.read(&handle)?;
    Ok(read
        .scoped_owner_table(GRAPH_CREATOR)?
        .get(graph)?
        .map(|row| row.value().to_string()))
}

/// Remove both ledgers of `graph` (a graph clear or purge).
pub fn clear_graph_rows(write: &ShardWrite<'_>, graph: &str) -> Result<(), String> {
    write
        .graph(graph)?
        .open_scoped_table(WRITE_DAYS)?
        .purge_scope_rows()?;
    write
        .graph(graph)?
        .open_scoped_table(GRAPH_CREATOR)?
        .purge_scope_rows()
}

/// Remove both ledgers of the graph a retired scope bound.
pub fn retire_graph_rows(
    write: &medulla_storage::PhysicalWriteCapability<'_, medulla_storage::GraphShardOwner>,
) -> Result<(), String> {
    write.scoped_owner_table_mut(WRITE_DAYS)?.purge_scope_rows()?;
    write
        .scoped_owner_table_mut(GRAPH_CREATOR)?
        .purge_scope_rows()
}
