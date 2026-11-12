//! Durable named sequences (MEDULLA-ENGINE-PARITY-R013.1).
//!
//! A sequence is four `u64` entries in the existing `__sql_seq__` allocator
//! table, under NUL-prefixed keys no table name can equal: `start`,
//! `increment`, `cache` and `high_mark`. `high_mark` is the first value never
//! reserved. The store reserves a block of `cache` values with one write
//! transaction (`high_mark += cache * increment`), then serves the block from
//! memory. After a crash the block in memory is gone and issuing resumes from
//! the persisted `high_mark`, so no value repeats; the unused part of the block
//! is a gap.

use std::collections::HashMap;
use std::sync::Mutex;

use medulla_tables::sequence::{Sequence, SequenceSource, SEQUENCE_REWIND, SEQUENCE_UNKNOWN};

use super::{map_err, SqlWrite, TableStore, SEQ};

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
    format!("\0seq\0{scope}\0{name}\0{}", field.label())
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
    let seq = wtx.open_table(SEQ)?;
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
                let mut seq = wtx.open_table(SEQ)?;
                if get_field(&seq, &scope, &def.name, Field::HighMark)?.is_some() {
                    if if_not_exists {
                        return Ok(false);
                    }
                    return Err(format!("sequence `{}` already exists", def.name));
                }
                put_field(&mut seq, &scope, &def.name, Field::Start, def.start as u64)?;
                put_field(
                    &mut seq,
                    &scope,
                    &def.name,
                    Field::Increment,
                    def.increment as u64,
                )?;
                put_field(&mut seq, &scope, &def.name, Field::Cache, def.cache)?;
                put_field(
                    &mut seq,
                    &scope,
                    &def.name,
                    Field::HighMark,
                    def.start as u64,
                )?;
                Ok(true)
            })
    }

    /// `DROP SEQUENCE`. `Ok(false)` is the `IF EXISTS` result for an absent one.
    pub fn drop_sequence(&self, name: &str, if_exists: bool) -> Result<bool, String> {
        let scope = self.index_scope().to_string();
        let dropped = self.authority.maintain("drop-sequence", name, |wtx| {
            let mut seq = wtx.open_table(SEQ)?;
            if get_field(&seq, &scope, name, Field::HighMark)?.is_none() {
                if if_exists {
                    return Ok(false);
                }
                return Err(unknown(name));
            }
            for field in [
                Field::Start,
                Field::Increment,
                Field::Cache,
                Field::HighMark,
            ] {
                seq.remove(key(&scope, name, field).as_str())
                    .map_err(map_err)?;
            }
            Ok(true)
        })?;
        self.blocks()?.remove(name);
        Ok(dropped)
    }

    /// The reserved high mark of `name`: the first value never reserved.
    pub fn sequence_high_mark(&self, name: &str) -> Result<i64, String> {
        let rtx = self.authority.read()?;
        let seq = rtx.open_owner_table(SEQ)?;
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
                let mut seq = wtx.open_table(SEQ)?;
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
            let mut seq = wtx.open_table(SEQ)?;
            put_field(&mut seq, &scope, name, Field::HighMark, next_high as u64)?;
            Ok(())
        })?;
        blocks.remove(name);
        Ok(value)
    }
}
