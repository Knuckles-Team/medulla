//! The decoded `CREATE [UNIQUE] INDEX` plan shared by the SQL classifier and the
//! table store (MEDULLA-ENGINE-PARITY-R010.2): the ordinary scalar index, as opposed
//! to the pgvector ANN registration in [`super::ann_plan`].

use medulla_tables::index::IndexExpr;
use medulla_tables::schema::CheckExpr;
use serde::{Deserialize, Serialize};

/// One key column of a `CREATE INDEX`, with the optional closed-set expression
/// applied to it (`lower(email)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexColumnPlan {
    pub column: String,
    pub expr: Option<IndexExpr>,
    pub descending: bool,
}

/// A decoded `CREATE [UNIQUE] INDEX [IF NOT EXISTS] [name] ON table (cols) [WHERE
/// predicate]`. `name` is `None` when the statement leaves it to the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexPlan {
    pub name: Option<String>,
    pub table: String,
    pub columns: Vec<IndexColumnPlan>,
    pub predicate: Option<CheckExpr>,
    pub unique: bool,
    pub if_not_exists: bool,
}

impl IndexPlan {
    /// The index name: the statement's, or `<table>_<columns>_idx` like Postgres.
    pub fn resolved_name(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => {
                let columns: Vec<&str> = self.columns.iter().map(|c| c.column.as_str()).collect();
                format!("{}_{}_idx", self.table, columns.join("_"))
            }
        }
    }
}
