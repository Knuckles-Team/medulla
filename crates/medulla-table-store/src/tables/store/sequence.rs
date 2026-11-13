//! Durable named sequences (MEDULLA-ENGINE-PARITY-R013.1).
//!
//! A sequence is four `u64` entries in the `__sql_sequences__` table, under
//! NUL-prefixed keys: `start`,
//! `increment`, `cache` and `high_mark`. `high_mark` is the first value never
//! reserved. The store reserves a block of `cache` values with one write
//! transaction (`high_mark += cache * increment`), then serves the block from
//! memory. After a crash the block in memory is gone and issuing resumes from
//! the persisted `high_mark`, so no value repeats; the unused part of the block
//! is a gap.

use std::collections::HashMap;
use std::sync::Mutex;

use medulla_tables::sequence::{Sequence, SequenceSource, SEQUENCE_REWIND, SEQUENCE_UNKNOWN};
use redb::ReadableTable;

use medulla_storage::{sequence_row_key, SQL_SEQUENCES};

use super::{map_err, SqlWrite, TableStore};

/// The values still to be served from one reserved block: `next` is the next
/// value, `end` the first value not in the block.
#[derive(Debug, Clone, Copy)]
pub(super) struct Block {
    next: i64,
    end: i64,
    increment: i64,
}

impl Block {
    fn issue(&mut self) -> i64 {
        let value = self.next;
        self.next += self.increment;
        value
    }
}

/// In-memory blocks, keyed by sequence name.
pub(super) type SequenceBlocks = Mutex<HashMap<String, Block>>;

#[derive(Clone, Copy)]
enum Field {
    Start,
    Increment,
    Cache,
    HighMark,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Increment => "increment",
            Self::Cache => "cache",
            Self::HighMark => "high_mark",
        }
    }
}

fn key(scope: &str, name: &str, field: Field) -> String {
    sequence_row_key(scope, name, field.label())
}

fn unknown(name: &str) -> String {
    format!("{SEQUENCE_UNKNOWN}: sequence `{name}` does not exist")
}

fn get_field(
    seq: &redb::Table<'_, &'static str, u64>,
    scope: &str,
    name: &str,
    field: Field,
) -> Result<Option<u64>, String> {
    Ok(seq
        .get(key(scope, name, field).as_str())
        .map_err(map_err)?
        .map(|v| v.value()))
}

fn put_field(
    seq: &mut redb::Table<'_, &'static str, u64>,
    scope: &str,
    name: &str,
    field: Field,
    value: u64,
) -> Result<(), String> {
    seq.insert(key(scope, name, field).as_str(), value)
        .map_err(map_err)?;
    Ok(())
}

/// Read a sequence's definition and high mark inside `wtx`.
fn load(wtx: &SqlWrite<'_>, scope: &str, name: &str) -> Result<(Sequence, i64), String> {
    let seq = wtx.open_table(SQL_SEQUENCES)?;
    let need = |field| get_field(&seq, scope, name, field)?.ok_or_else(|| unknown(name));
    Ok((
        Sequence {
            name: name.to_string(),
            start: need(Field::Start)? as i64,
            increment: need(Field::Increment)? as i64,
            cache: need(Field::Cache)?,
        },
        need(Field::HighMark)? as i64,
    ))
}

const SEQUENCE_FIELDS: [Field; 4] = [
    Field::Start,
    Field::Increment,
    Field::Cache,
    Field::HighMark,
];

/// Create a sequence inside `wtx`. `Ok(false)` is the `IF NOT EXISTS` result.
pub(super) fn create_in(
    wtx: &SqlWrite<'_>,
    scope: &str,
    def: &Sequence,
    if_not_exists: bool,
) -> Result<bool, String> {
    let mut seq = wtx.open_table(SQL_SEQUENCES)?;
    if get_field(&seq, scope, &def.name, Field::HighMark)?.is_some() {
        if if_not_exists {
            return Ok(false);
        }
        return Err(format!("sequence `{}` already exists", def.name));
    }
    put_field(&mut seq, scope, &def.name, Field::Start, def.start as u64)?;
    put_field(
        &mut seq,
        scope,
        &def.name,
        Field::Increment,
        def.increment as u64,
    )?;
    put_field(&mut seq, scope, &def.name, Field::Cache, def.cache)?;
    put_field(
        &mut seq,
        scope,
        &def.name,
        Field::HighMark,
        def.start as u64,
    )?;
    Ok(true)
}

/// Drop a sequence inside `wtx`. `Ok(false)` is the `IF EXISTS` result.
pub(super) fn drop_in(
    wtx: &SqlWrite<'_>,
    scope: &str,
    name: &str,
    if_exists: bool,
) -> Result<bool, String> {
    let mut seq = wtx.open_table(SQL_SEQUENCES)?;
    if get_field(&seq, scope, name, Field::HighMark)?.is_none() {
        if if_exists {
            return Ok(false);
        }
        return Err(unknown(name));
    }
    for field in SEQUENCE_FIELDS {
        seq.remove(key(scope, name, field).as_str())
            .map_err(map_err)?;
    }
    Ok(true)
}

/// Move a sequence to a new name inside `wtx`, keeping its definition and
/// high mark (a table or column rename moves its serial sequences).
pub(super) fn rename_in(
    wtx: &SqlWrite<'_>,
    scope: &str,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let mut seq = wtx.open_table(SQL_SEQUENCES)?;
    for field in SEQUENCE_FIELDS {
        let value = get_field(&seq, scope, from, field)?.ok_or_else(|| unknown(from))?;
        seq.remove(key(scope, from, field).as_str())
            .map_err(map_err)?;
        put_field(&mut seq, scope, to, field, value)?;
    }
    Ok(())
}

/// Allocate `count` consecutive values of sequence `name` inside the caller's
/// write transaction and return the first. The advance of the high mark commits
/// or rolls back with the statement, so a refused statement leaves at most a
/// gap and never a repeated value (MEDULLA-ENGINE-PARITY-R013.2).
pub(super) fn alloc_in(
    wtx: &SqlWrite<'_>,
    scope: &str,
    name: &str,
    count: u64,
) -> Result<i64, String> {
    let (def, high) = load(wtx, scope, name)?;
    let span = i64::try_from(count)
        .ok()
        .and_then(|count| count.checked_mul(def.increment))
        .and_then(|span| high.checked_add(span))
        .ok_or_else(|| format!("{SEQUENCE_REWIND}: sequence `{name}` is exhausted"))?;
    let mut seq = wtx.open_table(SQL_SEQUENCES)?;
    put_field(&mut seq, scope, name, Field::HighMark, span as u64)?;
    Ok(high)
}

impl TableStore {
    /// `CREATE SEQUENCE`. `Ok(false)` is the deterministic `IF NOT EXISTS`
    /// result.
    pub fn create_sequence(&self, def: &Sequence, if_not_exists: bool) -> Result<bool, String> {
        def.validate()?;
        let scope = self.index_scope().to_string();
        self.authority
            .maintain("create-sequence", &def.name, |wtx| {
                if super::get_schema_in(wtx, &def.name)?.is_some() {
                    return Err(format!(
                        "`{}` is a table; cannot create a sequence",
                        def.name
                    ));
                }
                create_in(wtx, &scope, def, if_not_exists)
            })
    }

    /// `DROP SEQUENCE`. `Ok(false)` is the `IF EXISTS` result for an absent one.
    pub fn drop_sequence(&self, name: &str, if_exists: bool) -> Result<bool, String> {
        let scope = self.index_scope().to_string();
        let dropped = self.authority.maintain("drop-sequence", name, |wtx| {
            drop_in(wtx, &scope, name, if_exists)
        })?;
        self.blocks()?.remove(name);
        Ok(dropped)
    }

    /// Forget every in-memory block.
    pub(super) fn forget_all_sequence_blocks(&self) -> Result<(), String> {
        self.blocks()?.clear();
        Ok(())
    }

    /// The reserved high mark of `name`: the first value never reserved.
    pub fn sequence_high_mark(&self, name: &str) -> Result<i64, String> {
        let rtx = self.authority.read()?;
        let seq = rtx.open_owner_table(SQL_SEQUENCES)?;
        seq.get(key(self.index_scope(), name, Field::HighMark).as_str())
            .map_err(map_err)?
            .map(|v| v.value() as i64)
            .ok_or_else(|| unknown(name))
    }

    /// Reserve a block of `cache` values with one write transaction.
    fn reserve_block(&self, name: &str) -> Result<Block, String> {
        let scope = self.index_scope().to_string();
        self.authority
            .maintain("reserve-sequence-block", name, |wtx| {
                let (def, high) = load(wtx, &scope, name)?;
                let span = (def.cache as i64)
                    .checked_mul(def.increment)
                    .and_then(|span| high.checked_add(span))
                    .ok_or_else(|| format!("{SEQUENCE_REWIND}: sequence `{name}` is exhausted"))?;
                let mut seq = wtx.open_table(SQL_SEQUENCES)?;
                put_field(&mut seq, &scope, name, Field::HighMark, span as u64)?;
                Ok(Block {
                    next: high,
                    end: span,
                    increment: def.increment,
                })
            })
    }
}

impl TableStore {
    fn blocks(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, Block>>, String> {
        self.sequence_blocks
            .lock()
            .map_err(|_| "sequence block lock poisoned".to_string())
    }
}

impl SequenceSource for TableStore {
    fn sequence_next(&self, name: &str) -> Result<i64, String> {
        let mut blocks = self.blocks()?;
        if let Some(block) = blocks.get_mut(name).filter(|b| b.next < b.end) {
            return Ok(block.issue());
        }
        // The lock is held across the reservation so two callers never reserve
        // overlapping blocks; the write is one small transaction.
        let mut block = self.reserve_block(name)?;
        let value = block.issue();
        blocks.insert(name.to_string(), block);
        Ok(value)
    }

    fn sequence_set(&self, name: &str, value: i64) -> Result<i64, String> {
        let scope = self.index_scope().to_string();
        let mut blocks = self.blocks()?;
        self.authority.maintain("set-sequence", name, |wtx| {
            let (def, high) = load(wtx, &scope, name)?;
            if value < high {
                return Err(format!(
                    "{SEQUENCE_REWIND}: setval({value}) is below the reserved high mark {high} of sequence `{name}`"
                ));
            }
            let next_high = value
                .checked_add(def.increment)
                .ok_or_else(|| format!("{SEQUENCE_REWIND}: sequence `{name}` is exhausted"))?;
            let mut seq = wtx.open_table(SQL_SEQUENCES)?;
            put_field(&mut seq, &scope, name, Field::HighMark, next_high as u64)?;
            Ok(())
        })?;
        blocks.remove(name);
        Ok(value)
    }
}
