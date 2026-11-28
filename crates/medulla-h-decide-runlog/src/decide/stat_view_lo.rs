//! The view column type, split out so the reputation relation reaches it.

use medulla_tables::schema::ColumnType;
use serde_json::Value;

/// One column: name, SQL type, and its cell for an item.
pub type Col<T> = (&'static str, ColumnType, fn(&T) -> Value);
