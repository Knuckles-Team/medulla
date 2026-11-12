//! Named sequences: the definition, its bounds and the closed refusal codes
//! (MEDULLA-ENGINE-PARITY-R013.1). Values never repeat after a crash; gaps are
//! allowed. The durable allocator lives in the table store; the SQL functions
//! `nextval`, `currval` and `setval` reach it through [`SequenceSource`].

use serde::{Deserialize, Serialize};

/// `setval` below the reserved high mark.
pub const SEQUENCE_REWIND: &str = "SEQUENCE_REWIND";
/// No sequence of that name exists.
pub const SEQUENCE_UNKNOWN: &str = "SEQUENCE_UNKNOWN";
/// A definition outside the bounds below.
pub const SEQUENCE_INVALID: &str = "SEQUENCE_INVALID";
/// Smallest and largest `cache` (values reserved per durable write).
pub const SEQUENCE_CACHE_MIN: u64 = 1;
/// See [`SEQUENCE_CACHE_MIN`].
pub const SEQUENCE_CACHE_MAX: u64 = 65_536;
/// Default `cache`.
pub const SEQUENCE_CACHE_DEFAULT: u64 = 32;

/// A sequence definition. `start` is the first value issued; every later value
/// adds `increment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    /// Sequence name (non-empty, NUL-free).
    pub name: String,
    /// First value issued (0 or more).
    pub start: i64,
    /// Step between values (1 or more).
    pub increment: i64,
    /// Values reserved per durable write, in [1, 65536].
    pub cache: u64,
}

impl Sequence {
    /// A sequence with the defaults: start 1, increment 1, cache 32.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            start: 1,
            increment: 1,
            cache: SEQUENCE_CACHE_DEFAULT,
        }
    }

    /// Check the definition against the bounds; the error starts with
    /// `SEQUENCE_INVALID`.
    pub fn validate(&self) -> Result<(), String> {
        let invalid = |why: &str| {
            Err(format!(
                "{SEQUENCE_INVALID}: sequence `{}` {why}",
                self.name
            ))
        };
        if self.name.is_empty() || self.name.contains('\0') {
            return invalid("needs a non-empty name without NUL");
        }
        if self.start < 0 {
            return invalid("start must be 0 or more");
        }
        if self.increment < 1 {
            return invalid("increment must be 1 or more");
        }
        if !(SEQUENCE_CACHE_MIN..=SEQUENCE_CACHE_MAX).contains(&self.cache) {
            return invalid("cache must be within 1..=65536");
        }
        Ok(())
    }
}

/// Where the SQL sequence functions get their values. The table store is the
/// production implementation.
pub trait SequenceSource: Send + Sync {
    /// Issue the next value of `name`; `SEQUENCE_UNKNOWN` when absent.
    fn sequence_next(&self, name: &str) -> Result<i64, String>;
    /// Set `name` so the next value follows `value`. Refuses `SEQUENCE_REWIND`
    /// when `value` is below the reserved high mark. Returns `value`.
    fn sequence_set(&self, name: &str, value: i64) -> Result<i64, String>;
}
