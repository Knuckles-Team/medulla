//! The constraint rows of the synthetic catalogs (MEDULLA-ENGINE-PARITY-R008.1).
//!
//! `pg_catalog.pg_constraint`, `information_schema.table_constraints` and
//! `information_schema.key_column_usage` all describe the same thing: the
//! constraints of the user tables. This module derives that one list from the
//! stored [`TableSchema`]s, so the three views can never disagree. Column-level
//! flags (`PRIMARY KEY`, `UNIQUE`, a column `CHECK`) become constraints named the
//! way `TableSchema::constraint_display_name` names them, which is the name a
//! `DROP CONSTRAINT` matches.

use medulla_tables::schema::{RefAction, TableConstraint, TableSchema};

/// The closed set of constraint kinds a user table carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConstraintKind {
    PrimaryKey,
    Unique,
    ForeignKey,
    Check,
}

impl ConstraintKind {
    /// `pg_constraint.contype`.
    pub(crate) fn contype(self) -> &'static str {
        match self {
            ConstraintKind::PrimaryKey => "p",
            ConstraintKind::Unique => "u",
            ConstraintKind::ForeignKey => "f",
            ConstraintKind::Check => "c",
        }
    }

    /// `information_schema.table_constraints.constraint_type`.
    pub(crate) fn constraint_type(self) -> &'static str {
        match self {
            ConstraintKind::PrimaryKey => "PRIMARY KEY",
            ConstraintKind::Unique => "UNIQUE",
            ConstraintKind::ForeignKey => "FOREIGN KEY",
            ConstraintKind::Check => "CHECK",
        }
    }
}

/// The referenced side of a foreign key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignTarget {
    pub(crate) table: String,
    pub(crate) columns: Vec<String>,
    pub(crate) on_update: &'static str,
    pub(crate) on_delete: &'static str,
}

/// One constraint of one user table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConstraintRow {
    pub(crate) name: String,
    pub(crate) table: String,
    pub(crate) kind: ConstraintKind,
    pub(crate) columns: Vec<String>,
    pub(crate) target: Option<ForeignTarget>,
    pub(crate) deferrable: bool,
    pub(crate) initially_deferred: bool,
}

/// `pg_constraint.confupdtype` / `confdeltype` for a referential action.
fn action_code(action: &RefAction) -> &'static str {
    match action {
        RefAction::NoAction => "a",
        RefAction::Restrict => "r",
        RefAction::Cascade => "c",
        RefAction::SetNull => "n",
    }
}

fn row(table: &str, name: String, kind: ConstraintKind, columns: Vec<String>) -> ConstraintRow {
    ConstraintRow {
        name,
        table: table.to_string(),
        kind,
        columns,
        target: None,
        deferrable: false,
        initially_deferred: false,
    }
}

/// Every constraint of `schema`: the primary key, the column-level `UNIQUE`s and
/// `CHECK`s, then the table-level constraints in declaration order. A column-level
/// flag already stated by a table-level constraint is not listed twice.
fn table_constraint_rows(schema: &TableSchema) -> Vec<ConstraintRow> {
    let table = schema.name.as_str();
    let declared = schema.constraints();
    let mut out = Vec::new();

    let has_table_pk = declared
        .iter()
        .any(|c| matches!(c, TableConstraint::PrimaryKey { .. }));
    if !has_table_pk {
        let pk_columns: Vec<String> = schema
            .columns()
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| c.name.clone())
            .collect();
        if !pk_columns.is_empty() {
            out.push(row(
                table,
                format!("{table}_pkey"),
                ConstraintKind::PrimaryKey,
                pk_columns,
            ));
        }
    }
    for column in schema.columns() {
        if column.unique && !column.primary_key {
            let covered = declared.iter().any(|c| {
                matches!(c, TableConstraint::Unique { columns, .. }
                    if columns.len() == 1 && columns[0] == column.name)
            });
            if !covered {
                out.push(row(
                    table,
                    format!("{table}_{}_key", column.name),
                    ConstraintKind::Unique,
                    vec![column.name.clone()],
                ));
            }
        }
        if column.check.is_some() {
            out.push(row(
                table,
                format!("{table}_{}_check", column.name),
                ConstraintKind::Check,
                vec![column.name.clone()],
            ));
        }
    }
    for constraint in declared {
        let name = TableSchema::constraint_display_name(table, constraint);
        out.push(match constraint {
            TableConstraint::PrimaryKey { columns, .. } => {
                row(table, name, ConstraintKind::PrimaryKey, columns.clone())
            }
            TableConstraint::Unique { columns, .. } => {
                row(table, name, ConstraintKind::Unique, columns.clone())
            }
            TableConstraint::ForeignKey {
                columns,
                ref_table,
                ref_columns,
                on_delete,
                on_update,
                deferrable,
                initially_deferred,
                ..
            } => ConstraintRow {
                target: Some(ForeignTarget {
                    table: ref_table.clone(),
                    columns: ref_columns.clone(),
                    on_update: action_code(on_update),
                    on_delete: action_code(on_delete),
                }),
                deferrable: *deferrable,
                initially_deferred: *initially_deferred,
                ..row(table, name, ConstraintKind::ForeignKey, columns.clone())
            },
            TableConstraint::Check { .. } => row(table, name, ConstraintKind::Check, Vec::new()),
        });
    }
    out
}

/// The constraints of every table, tables in the order given.
pub(crate) fn constraint_rows(tables: &[TableSchema]) -> Vec<ConstraintRow> {
    tables.iter().flat_map(table_constraint_rows).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_tables::schema::{Column, ColumnType};

    fn parent() -> TableSchema {
        TableSchema::new(
            "parent",
            vec![
                Column::new("id", ColumnType::BigInt, false, true),
                Column::new("code", ColumnType::Text, false, false),
            ],
        )
    }

    // spec: MEDULLA-ENGINE-PARITY-R008.1
    #[test]
    fn column_flags_and_table_constraints_become_one_list() {
        let mut code = Column::new("code", ColumnType::Text, false, false);
        code.unique = true;
        let child = TableSchema::new(
            "child",
            vec![
                Column::new("id", ColumnType::BigInt, false, true),
                Column::new("parent_id", ColumnType::BigInt, true, false),
                code,
            ],
        )
        .with_constraints(vec![TableConstraint::ForeignKey {
            name: None,
            columns: vec!["parent_id".into()],
            ref_table: "parent".into(),
            ref_columns: vec!["id".into()],
            on_delete: RefAction::Cascade,
            on_update: RefAction::NoAction,
            deferrable: true,
            initially_deferred: false,
        }]);
        let rows = constraint_rows(&[parent(), child]);
        let listed: Vec<(&str, &str, ConstraintKind)> = rows
            .iter()
            .map(|r| (r.table.as_str(), r.name.as_str(), r.kind))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("parent", "parent_pkey", ConstraintKind::PrimaryKey),
                ("child", "child_pkey", ConstraintKind::PrimaryKey),
                ("child", "child_code_key", ConstraintKind::Unique),
                ("child", "child_parent_id_fkey", ConstraintKind::ForeignKey),
            ]
        );
        let fk = rows.last().unwrap();
        let target = fk.target.as_ref().unwrap();
        assert_eq!(target.table, "parent");
        assert_eq!((target.on_delete, target.on_update), ("c", "a"));
        assert!(fk.deferrable && !fk.initially_deferred);
    }

    // spec: MEDULLA-ENGINE-PARITY-R008.1
    #[test]
    fn a_unique_flag_already_declared_is_listed_once() {
        let mut code = Column::new("code", ColumnType::Text, false, false);
        code.unique = true;
        let table =
            TableSchema::new("t", vec![code]).with_constraints(vec![TableConstraint::Unique {
                name: Some("t_code_uq".into()),
                columns: vec!["code".into()],
            }]);
        let rows = constraint_rows(&[table]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "t_code_uq");
    }
}
