//! Graph-backed relational projections, read only (MEDULLA-UNIFIED-DATA-PLANE-R026.4.1).
//!
//! An approved, versioned mapping presents the nodes of one graph label as a relational
//! table. `Method::ProjectionRead` reads one such table and binds the answer to the
//! mapping revision and the graph revision it was read at. This module holds the wire
//! shapes and the pure evaluation (typed predicates, column selection, primary-key
//! order and the result digest); the served handler in `medulla-h-query` supplies the
//! caller-visible nodes and never writes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::retrieval::sha256_hex;

/// Label of the graph node that stores one mapping revision.
pub const PROJECTION_MAPPING_LABEL: &str = "ProjectionMapping";

/// Node id of one stored mapping revision of one tenant.
pub fn projection_mapping_node_id(tenant: &str, mapping_id: &str, revision: u64) -> String {
    format!("projection_mapping:{tenant}:{mapping_id}:{revision}")
}

/// Node id prefix shared by every revision of one mapping of one tenant.
pub fn projection_mapping_prefix(tenant: &str, mapping_id: &str) -> String {
    format!("projection_mapping:{tenant}:{mapping_id}:")
}

/// The type of one projected column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProjectionColumnType {
    Text,
    Integer,
    Number,
    Boolean,
}

impl ProjectionColumnType {
    /// Whether `value` is a value of this type.
    pub fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Text => value.is_string(),
            Self::Integer => value.is_i64() || value.is_u64(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
        }
    }
}

/// One projected column: its relational name, the node field it reads and its type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionColumn {
    pub name: String,
    pub field: String,
    pub column_type: ProjectionColumnType,
}

/// One projected table: the node label it presents, its primary key column and columns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionTable {
    pub name: String,
    pub label: String,
    pub primary_key: String,
    pub columns: Vec<ProjectionColumn>,
}

impl ProjectionTable {
    fn column(&self, name: &str) -> Option<&ProjectionColumn> {
        self.columns.iter().find(|column| column.name == name)
    }
}

/// One stored mapping revision. `approval_state` is the text of
/// `MappingApproval` (`approved`, `proposed`, `model-suggested`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionMapping {
    pub mapping_id: String,
    pub revision: u64,
    pub approval_state: String,
    pub tables: Vec<ProjectionTable>,
}

/// A comparison operator of a typed predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProjectionCompare {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// One typed predicate over one projected column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionPredicate {
    pub column: String,
    pub op: ProjectionCompare,
    pub value: Value,
}

/// A projection read: which mapping revision, which table, which columns and rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionReadRequest {
    pub tenant: String,
    pub actor: String,
    pub graph: String,
    pub mapping_id: String,
    pub mapping_revision: u64,
    pub table: String,
    /// Selected columns; empty selects every column of the table.
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub predicates: Vec<ProjectionPredicate>,
    /// SHA-256 of [`ProjectionReadRequest::computed_query_digest`]; empty to skip the check.
    #[serde(default)]
    pub query_digest: String,
}

impl ProjectionReadRequest {
    /// The digest the caller binds the read to: SHA-256 over the canonical JSON of
    /// mapping, table, columns and predicates.
    pub fn computed_query_digest(&self) -> String {
        let canonical = serde_json::json!({
            "mapping_id": self.mapping_id,
            "mapping_revision": self.mapping_revision,
            "table": self.table,
            "columns": self.columns,
            "predicates": self.predicates,
        });
        sha256_hex(canonical.to_string().as_bytes())
    }
}

/// The name and type of one returned column.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionColumnInfo {
    pub name: String,
    pub column_type: ProjectionColumnType,
}

/// A projection read answer, bound to both revisions it was read at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionReadResponse {
    pub mapping_revision: u64,
    pub graph_revision: u64,
    pub column_types: Vec<ProjectionColumnInfo>,
    pub rows: Vec<BTreeMap<String, Value>>,
    /// SHA-256 over the canonical JSON of `rows`, sorted by primary key (64 lowercase hex).
    pub result_digest: String,
}

/// Why a projection read request is not valid against its table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionShapeError {
    UnknownTable(String),
    UnknownColumn(String),
    PredicateType { column: String },
}

impl std::fmt::Display for ProjectionShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTable(table) => write!(f, "mapping names no table `{table}`"),
            Self::UnknownColumn(column) => write!(f, "table has no column `{column}`"),
            Self::PredicateType { column } => {
                write!(
                    f,
                    "predicate value does not match the type of column `{column}`"
                )
            }
        }
    }
}

impl std::error::Error for ProjectionShapeError {}

fn compare(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => {
            if let (Some(a), Some(b)) = (a.as_i64(), b.as_i64()) {
                Some(a.cmp(&b))
            } else {
                a.as_f64()?.partial_cmp(&b.as_f64()?)
            }
        }
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

fn holds(predicate: &ProjectionPredicate, cell: Option<&Value>) -> bool {
    let Some(cell) = cell else {
        return false;
    };
    let Some(order) = compare(cell, &predicate.value) else {
        return false;
    };
    use std::cmp::Ordering::{Equal, Greater, Less};
    match predicate.op {
        ProjectionCompare::Eq => order == Equal,
        ProjectionCompare::Ne => order != Equal,
        ProjectionCompare::Lt => order == Less,
        ProjectionCompare::Le => order != Greater,
        ProjectionCompare::Gt => order == Greater,
        ProjectionCompare::Ge => order != Less,
    }
}

/// One node's fields read as a row of `table`: absent or mistyped fields are left out.
pub fn row_of(
    table: &ProjectionTable,
    fields: &serde_json::Map<String, Value>,
) -> BTreeMap<String, Value> {
    table
        .columns
        .iter()
        .filter_map(|column| {
            let value = fields.get(&column.field)?;
            column
                .column_type
                .accepts(value)
                .then(|| (column.name.clone(), value.clone()))
        })
        .collect()
}

/// The columns, rows and result digest `evaluate` returns.
pub type EvaluatedRead = (
    Vec<ProjectionColumnInfo>,
    Vec<BTreeMap<String, Value>>,
    String,
);

/// Validate the request against `table`, filter `rows`, order them by primary key,
/// project the selected columns and compute the result digest.
pub fn evaluate(
    table: &ProjectionTable,
    request: &ProjectionReadRequest,
    mut rows: Vec<BTreeMap<String, Value>>,
) -> Result<EvaluatedRead, ProjectionShapeError> {
    let selected: Vec<&ProjectionColumn> = if request.columns.is_empty() {
        table.columns.iter().collect()
    } else {
        request
            .columns
            .iter()
            .map(|name| {
                table
                    .column(name)
                    .ok_or_else(|| ProjectionShapeError::UnknownColumn(name.clone()))
            })
            .collect::<Result<_, _>>()?
    };
    for predicate in &request.predicates {
        let column = table
            .column(&predicate.column)
            .ok_or_else(|| ProjectionShapeError::UnknownColumn(predicate.column.clone()))?;
        if !column.column_type.accepts(&predicate.value) {
            return Err(ProjectionShapeError::PredicateType {
                column: predicate.column.clone(),
            });
        }
    }
    rows.retain(|row| {
        request
            .predicates
            .iter()
            .all(|predicate| holds(predicate, row.get(&predicate.column)))
    });
    let key = table.primary_key.as_str();
    rows.sort_by(|left, right| match (left.get(key), right.get(key)) {
        (Some(a), Some(b)) => compare(a, b).unwrap_or(std::cmp::Ordering::Equal),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    let projected: Vec<BTreeMap<String, Value>> = rows
        .into_iter()
        .map(|row| {
            selected
                .iter()
                .filter_map(|column| {
                    row.get(&column.name)
                        .map(|v| (column.name.clone(), v.clone()))
                })
                .collect()
        })
        .collect();
    let digest = result_digest(&projected);
    let infos = selected
        .iter()
        .map(|column| ProjectionColumnInfo {
            name: column.name.clone(),
            column_type: column.column_type,
        })
        .collect();
    Ok((infos, projected, digest))
}

/// SHA-256 (64 lowercase hex) over the canonical JSON of the rows in their given order.
pub fn result_digest(rows: &[BTreeMap<String, Value>]) -> String {
    let canonical = serde_json::to_vec(rows).unwrap_or_default();
    sha256_hex(&canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table() -> ProjectionTable {
        ProjectionTable {
            name: "people".into(),
            label: "Person".into(),
            primary_key: "id".into(),
            columns: vec![
                ProjectionColumn {
                    name: "id".into(),
                    field: "person_id".into(),
                    column_type: ProjectionColumnType::Integer,
                },
                ProjectionColumn {
                    name: "name".into(),
                    field: "full_name".into(),
                    column_type: ProjectionColumnType::Text,
                },
            ],
        }
    }

    fn request() -> ProjectionReadRequest {
        ProjectionReadRequest {
            tenant: "t".into(),
            actor: "a".into(),
            graph: "g".into(),
            mapping_id: "m".into(),
            mapping_revision: 1,
            table: "people".into(),
            columns: Vec::new(),
            predicates: Vec::new(),
            query_digest: String::new(),
        }
    }

    fn rows() -> Vec<BTreeMap<String, Value>> {
        [(10, "b"), (2, "a")]
            .iter()
            .map(|(id, name)| {
                row_of(
                    &table(),
                    json!({"person_id": id, "full_name": name})
                        .as_object()
                        .unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn rows_sort_by_primary_key_numerically_and_digest_is_stable() {
        let (_, first, digest) = evaluate(&table(), &request(), rows()).unwrap();
        assert_eq!(first[0]["id"], json!(2));
        assert_eq!(digest.len(), 64);
        let mut reversed = rows();
        reversed.reverse();
        assert_eq!(evaluate(&table(), &request(), reversed).unwrap().2, digest);
    }

    #[test]
    fn predicates_filter_and_unknown_columns_are_refused() {
        let mut filtered = request();
        filtered.predicates.push(ProjectionPredicate {
            column: "id".into(),
            op: ProjectionCompare::Gt,
            value: json!(5),
        });
        assert_eq!(evaluate(&table(), &filtered, rows()).unwrap().1.len(), 1);
        let mut bad = request();
        bad.columns.push("nope".into());
        assert_eq!(
            evaluate(&table(), &bad, rows()).unwrap_err(),
            ProjectionShapeError::UnknownColumn("nope".into())
        );
        let mut mistyped = request();
        mistyped.predicates.push(ProjectionPredicate {
            column: "id".into(),
            op: ProjectionCompare::Eq,
            value: json!("x"),
        });
        assert!(matches!(
            evaluate(&table(), &mistyped, rows()).unwrap_err(),
            ProjectionShapeError::PredicateType { .. }
        ));
    }
}
