//! MEDULLA-UNIFIED-DATA-PLANE-R003 — the typed, hashed attached-source catalog graph. This is the
//! `.1` typed-model slice: [`AttachedCatalogGraph`] and its content hash, plus the refusal of an
//! ambiguous catalog (two tables sharing the same schema-qualified name). Extracting a real
//! catalog from a live dialect adapter's reader, and wiring an OBDA/`schema_context` consumer to
//! it, are later children.
//!
//! MEDULLA-UNIFIED-DATA-PLANE-R763.7: the catalog also carries view definitions
//! ([`CatalogView`]), which column lineage reads.

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::lineage::LineageDialect;

/// One column of an attached-source table, as discovered by a catalog reader.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CatalogColumn {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
}

/// The kind of one [`CatalogConstraint`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CatalogConstraintKind {
    PrimaryKey,
    Unique,
    ForeignKey,
    /// A non-unique secondary index.
    Index,
}

/// The `(schema, table, columns)` a foreign key points at.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CatalogReference {
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
}

/// One key or index of a table, over its columns in key order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CatalogConstraint {
    pub name: String,
    pub kind: CatalogConstraintKind,
    pub columns: Vec<String>,
    pub references: Option<CatalogReference>,
}

/// One table discovered in an attached source's catalog. Views are [`CatalogView`]s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<CatalogColumn>,
    /// Keys, foreign keys and indexes; empty when the reader did not read them.
    pub constraints: Vec<CatalogConstraint>,
}

/// One view of an attached source: its name and the SQL that defines it.
///
/// A definition larger than `knob lineage.max_sql_bytes` is kept with an empty
/// `definition_sql` and `definition_gap` set, so the catalog still names the view and a
/// reader can tell a gap from a view that has no text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogView {
    pub schema: String,
    pub name: String,
    pub dialect: LineageDialect,
    pub definition_sql: String,
    /// The definition was over the size bound and was not kept.
    pub definition_gap: bool,
}

impl CatalogView {
    /// A view whose definition is kept, or kept as a gap when it is over `max_sql_bytes`.
    pub fn bounded(
        schema: &str,
        name: &str,
        dialect: LineageDialect,
        definition_sql: &str,
        max_sql_bytes: usize,
    ) -> Self {
        let gap = definition_sql.len() > max_sql_bytes;
        Self {
            schema: schema.to_string(),
            name: name.to_string(),
            dialect,
            definition_sql: if gap {
                String::new()
            } else {
                definition_sql.to_string()
            },
            definition_gap: gap,
        }
    }

    fn dialect_tag(&self) -> &'static str {
        match self.dialect {
            LineageDialect::Postgres => "postgres",
            LineageDialect::MySql => "mysql",
        }
    }
}

/// A versioned, typed catalog graph extracted from one attached source. `content_hash` is
/// stable under reordering: it sorts tables and columns before hashing, so two reads of the
/// same unchanged catalog hash identically regardless of the order the source reported them in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachedCatalogGraph {
    pub tables: Vec<CatalogTable>,
    pub views: Vec<CatalogView>,
    pub version: u32,
}

/// Two tables (or two views) in the same catalog share a schema-qualified name: the catalog is
/// ambiguous and is refused rather than silently accepted with one shadowing the other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateCatalogTable {
    pub schema: String,
    pub name: String,
}

impl fmt::Display for DuplicateCatalogTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "catalog names table {:?}.{:?} more than once",
            self.schema, self.name
        )
    }
}

impl std::error::Error for DuplicateCatalogTable {}

impl AttachedCatalogGraph {
    /// Refuses a catalog that names the same `(schema, table)` pair more than once.
    pub fn validate(&self) -> Result<(), DuplicateCatalogTable> {
        let mut seen: Vec<(&str, &str)> = Vec::with_capacity(self.tables.len());
        for table in &self.tables {
            let key = (table.schema.as_str(), table.name.as_str());
            if seen.contains(&key) {
                return Err(DuplicateCatalogTable {
                    schema: table.schema.clone(),
                    name: table.name.clone(),
                });
            }
            seen.push(key);
        }
        let mut seen_views: Vec<(&str, &str)> = Vec::with_capacity(self.views.len());
        for view in &self.views {
            let key = (view.schema.as_str(), view.name.as_str());
            if seen_views.contains(&key) {
                return Err(DuplicateCatalogTable {
                    schema: view.schema.clone(),
                    name: view.name.clone(),
                });
            }
            seen_views.push(key);
        }
        Ok(())
    }

    /// A deterministic hash of this catalog's shape: tables sorted by `(schema, name)`, each
    /// table's columns sorted by name, and views sorted by `(schema, name)`, before hashing. Two graphs built from the same tables in
    /// a different discovery order hash identically.
    pub fn content_hash(&self) -> u64 {
        let mut tables: Vec<&CatalogTable> = self.tables.iter().collect();
        tables.sort_by(|a, b| {
            (a.schema.as_str(), a.name.as_str()).cmp(&(b.schema.as_str(), b.name.as_str()))
        });

        let mut hasher = DefaultHasher::new();
        self.version.hash(&mut hasher);
        for table in tables {
            table.schema.hash(&mut hasher);
            table.name.hash(&mut hasher);
            let mut columns: Vec<&CatalogColumn> = table.columns.iter().collect();
            columns.sort_by(|a, b| a.name.cmp(&b.name));
            for column in columns {
                column.hash(&mut hasher);
            }
            let mut constraints: Vec<&CatalogConstraint> = table.constraints.iter().collect();
            constraints.sort();
            for constraint in constraints {
                constraint.hash(&mut hasher);
            }
        }
        let mut views: Vec<&CatalogView> = self.views.iter().collect();
        views.sort_by(|a, b| {
            (a.schema.as_str(), a.name.as_str()).cmp(&(b.schema.as_str(), b.name.as_str()))
        });
        for view in views {
            view.schema.hash(&mut hasher);
            view.name.hash(&mut hasher);
            view.dialect_tag().hash(&mut hasher);
            view.definition_sql.hash(&mut hasher);
            view.definition_gap.hash(&mut hasher);
        }
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, type_name: &str) -> CatalogColumn {
        CatalogColumn {
            name: name.to_string(),
            type_name: type_name.to_string(),
            nullable: false,
        }
    }

    fn table(schema: &str, name: &str, columns: Vec<CatalogColumn>) -> CatalogTable {
        CatalogTable {
            schema: schema.to_string(),
            name: name.to_string(),
            columns,
            constraints: vec![],
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R003.1
    #[test]
    fn content_hash_is_stable_under_table_and_column_reordering() {
        let a = AttachedCatalogGraph {
            version: 1,
            views: vec![],
            tables: vec![
                table(
                    "public",
                    "users",
                    vec![column("id", "int8"), column("email", "text")],
                ),
                table("public", "orders", vec![column("id", "int8")]),
            ],
        };
        let b = AttachedCatalogGraph {
            version: 1,
            views: vec![],
            tables: vec![
                table("public", "orders", vec![column("id", "int8")]),
                table(
                    "public",
                    "users",
                    vec![column("email", "text"), column("id", "int8")],
                ),
            ],
        };
        assert_eq!(a.content_hash(), b.content_hash());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R003.1
    #[test]
    fn content_hash_changes_with_the_catalog() {
        let a = AttachedCatalogGraph {
            version: 1,
            views: vec![],
            tables: vec![table("public", "users", vec![column("id", "int8")])],
        };
        let b = AttachedCatalogGraph {
            version: 1,
            views: vec![],
            tables: vec![table("public", "users", vec![column("id", "int4")])],
        };
        assert_ne!(a.content_hash(), b.content_hash());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R003.1
    #[test]
    fn a_catalog_with_one_table_per_name_validates() {
        let graph = AttachedCatalogGraph {
            version: 1,
            views: vec![],
            tables: vec![
                table("public", "users", vec![]),
                table("public", "orders", vec![]),
                table("reporting", "users", vec![]),
            ],
        };
        graph.validate().unwrap();
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R003.1
    #[test]
    fn a_duplicated_schema_qualified_table_name_is_refused() {
        let graph = AttachedCatalogGraph {
            version: 1,
            views: vec![],
            tables: vec![
                table("public", "users", vec![column("id", "int8")]),
                table("public", "users", vec![column("id", "int4")]),
            ],
        };
        let err = graph.validate().unwrap_err();
        assert_eq!(err.schema, "public");
        assert_eq!(err.name, "users");
        assert!(err.to_string().contains("more than once"));
    }

    fn view(schema: &str, name: &str, sql: &str) -> CatalogView {
        CatalogView::bounded(schema, name, LineageDialect::Postgres, sql, 1_024)
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.7
    #[test]
    fn view_order_does_not_change_hash() {
        let with = |views: Vec<CatalogView>| AttachedCatalogGraph {
            version: 1,
            tables: vec![table("public", "orders", vec![column("id", "int8")])],
            views,
        };
        let a = with(vec![
            view("public", "v1", "SELECT id FROM orders"),
            view("public", "v2", "SELECT id FROM v1"),
        ]);
        let b = with(vec![
            view("public", "v2", "SELECT id FROM v1"),
            view("public", "v1", "SELECT id FROM orders"),
        ]);
        assert_eq!(a.content_hash(), b.content_hash());
        a.validate().unwrap();
        // The hash covers a view's definition and the catalog without views.
        let changed = with(vec![
            view("public", "v1", "SELECT id + 1 FROM orders"),
            view("public", "v2", "SELECT id FROM v1"),
        ]);
        assert_ne!(a.content_hash(), changed.content_hash());
        assert_ne!(a.content_hash(), with(vec![]).content_hash());
        // A duplicate view name is refused with the existing error.
        let dup = with(vec![
            view("public", "v1", "SELECT 1"),
            view("public", "v1", "SELECT 2"),
        ]);
        let err = dup.validate().unwrap_err();
        assert_eq!((err.schema.as_str(), err.name.as_str()), ("public", "v1"));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.7
    #[test]
    fn an_oversized_view_definition_is_kept_as_a_gap() {
        let big = "x".repeat(2_000);
        let kept = CatalogView::bounded("s", "v", LineageDialect::MySql, &big, 1_024);
        assert!(kept.definition_gap);
        assert!(kept.definition_sql.is_empty());
        let small = CatalogView::bounded("s", "v", LineageDialect::MySql, "SELECT 1", 1_024);
        assert!(!small.definition_gap);
        assert_eq!(small.definition_sql, "SELECT 1");
    }
}
