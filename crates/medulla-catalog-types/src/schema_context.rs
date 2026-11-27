//! Typed model for the `schema_context` query surface
//! (MEDULLA-UNIFIED-DATA-PLANE-R029): what a table is, what joins to it, which
//! ontology class and application it belongs to, and the impact of a
//! change -- the same pattern as the existing `code_context` surface. This
//! module holds the typed-model slice (`.1`): the answer shape and the
//! refusal to answer for a table absent from the catalog; plus the `.2`
//! served query entry point (`serve_schema_context`): a structured,
//! wire-serializable request/response pair that Graph OS can dispatch the
//! same way `graph_code action=code_context` dispatches to
//! the meristem harness. The AU client
//! call and the graph-os `graph_schema` intent-tool registration that
//! invoke this entry point are follow-up child rows (see the PR for their
//! proposed IDs); this module only has to serve the request once it
//! arrives.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One join EG's catalog knows reaches this table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaJoin {
    pub joined_table: String,
    pub on_column: String,
    pub joined_column: String,
}

/// The part of the answer one `Method::SchemaContext` call asks for
/// (MEDULLA-UNIFIED-DATA-PLANE-R029.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SchemaContextIntent {
    /// The columns of the table.
    Definition,
    /// The joins that reach the table.
    Joins,
    /// The ontology class the table maps to.
    Ontology,
    /// The agent that registered the source.
    Owner,
    /// What a change to the table's columns reaches.
    Impact,
}

/// One column of the table a `schema_context` answer defines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaColumn {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
}

/// `schema_context`'s answer for one table. It always carries every part the engine can know;
/// a part it cannot know is `None` or empty here and is refused when the intent asks for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaContextAnswer {
    pub table: String,
    pub columns: Vec<SchemaColumn>,
    pub joins: Vec<SchemaJoin>,
    /// The ontology class a registered graph view maps this table to, when exactly one does.
    pub ontology_class: Option<String>,
    /// The attached source the table belongs to.
    pub application: String,
    /// The agent that registered the source.
    pub owner: String,
    /// What a change to this table's columns reaches: the downstream columns, sorted.
    pub change_impact: String,
}

/// A query asked about a table the catalog fixture does not know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownTable(pub String);

impl std::fmt::Display for UnknownTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "schema_context: unknown table {:?}", self.0)
    }
}

impl std::error::Error for UnknownTable {}

/// The catalog fixture `schema_context` answers against: every known
/// table's precomputed answer, keyed by table name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaContextCatalog {
    pub tables: BTreeMap<String, SchemaContextAnswer>,
}

impl SchemaContextCatalog {
    /// Answer `schema_context` for `table`. Refuses an unknown table rather
    /// than returning an empty/default answer that looks like "no joins,
    /// no class, no impact" for a table EG never catalogued.
    pub fn schema_context(&self, table: &str) -> Result<&SchemaContextAnswer, UnknownTable> {
        self.tables
            .get(table)
            .ok_or_else(|| UnknownTable(table.to_string()))
    }
}

/// The request shape the `schema_context` query entry point accepts. A
/// struct (rather than a bare `&str`) so the entry point is a real wire
/// boundary: it decodes the same way a `graph_code` request body decodes
/// before reaching `code_context`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaContextRequest {
    pub table: String,
}

/// The `schema_context` entry point's refusal, serialized across the wire
/// rather than EG's internal `UnknownTable` (which only implements
/// `Display`/`Error` for in-process use).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaContextQueryError {
    pub table: String,
    pub reason: String,
}

impl From<UnknownTable> for SchemaContextQueryError {
    fn from(err: UnknownTable) -> Self {
        SchemaContextQueryError {
            table: err.0.clone(),
            reason: err.to_string(),
        }
    }
}

impl std::fmt::Display for SchemaContextQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for SchemaContextQueryError {}

/// The real `schema_context` query entry point (MEDULLA-UNIFIED-DATA-PLANE-
/// R029.2): the served method this requirement wires up. Graph OS's
/// `code_context` pattern is a composed KG traversal invoked by name
/// (`graph_code action=code_context`); `schema_context` is a precomputed
/// catalog lookup, so its entry point takes the same shape -- a decoded
/// request in, a decoded answer or a decoded refusal out -- without
/// requiring a live graph traversal. This is the function the AU client
/// call and the graph-os intent-tool registration (follow-up child rows)
/// dispatch to.
pub fn serve_schema_context(
    catalog: &SchemaContextCatalog,
    request: &SchemaContextRequest,
) -> Result<SchemaContextAnswer, SchemaContextQueryError> {
    catalog
        .schema_context(&request.table)
        .cloned()
        .map_err(SchemaContextQueryError::from)
}

/// The source named in the request is not one the caller registered (another owner's source
/// of the same name reads exactly like an absent one).
pub const SCHEMA_CONTEXT_SOURCE_UNKNOWN: &str = "SCHEMA_CONTEXT_SOURCE_UNKNOWN";
/// The catalog of the source has no such table.
pub const SCHEMA_CONTEXT_TABLE_UNKNOWN: &str = "SCHEMA_CONTEXT_TABLE_UNKNOWN";
/// No registered graph view maps the table to an ontology class.
pub const SCHEMA_CONTEXT_UNMAPPED: &str = "SCHEMA_CONTEXT_UNMAPPED";
/// More than one ontology class is mapped to the table.
pub const SCHEMA_CONTEXT_CLASS_AMBIGUOUS: &str = "SCHEMA_CONTEXT_CLASS_AMBIGUOUS";

/// The name of `schema.name`, or just `name` for an unqualified source.
pub fn qualified_name(schema: &str, name: &str) -> String {
    if schema.is_empty() {
        name.to_string()
    } else {
        format!("{schema}.{name}")
    }
}

impl SchemaContextCatalog {
    /// One answer per table of `graph`, keyed by its qualified name. The columns and the joins
    /// (the table's own foreign keys, and every foreign key that references it) come from the
    /// catalog; the ontology class and the change impact are filled by the caller that knows
    /// them, so they start as `None` and empty.
    pub fn from_attached(
        graph: &medulla_catalog_source_types::attached_catalog::AttachedCatalogGraph,
        application: &str,
        owner: &str,
    ) -> Self {
        use medulla_catalog_source_types::attached_catalog::CatalogConstraintKind;
        let mut tables = BTreeMap::new();
        for table in &graph.tables {
            let name = qualified_name(&table.schema, &table.name);
            let mut joins = Vec::new();
            for constraint in &table.constraints {
                let (CatalogConstraintKind::ForeignKey, Some(target)) =
                    (constraint.kind, &constraint.references)
                else {
                    continue;
                };
                for (index, column) in constraint.columns.iter().enumerate() {
                    joins.push(SchemaJoin {
                        joined_table: qualified_name(&target.schema, &target.table),
                        on_column: column.clone(),
                        joined_column: target.columns.get(index).cloned().unwrap_or_default(),
                    });
                }
            }
            for other in &graph.tables {
                for constraint in &other.constraints {
                    let (CatalogConstraintKind::ForeignKey, Some(target)) =
                        (constraint.kind, &constraint.references)
                    else {
                        continue;
                    };
                    if target.schema != table.schema || target.table != table.name {
                        continue;
                    }
                    for (index, column) in constraint.columns.iter().enumerate() {
                        joins.push(SchemaJoin {
                            joined_table: qualified_name(&other.schema, &other.name),
                            on_column: target.columns.get(index).cloned().unwrap_or_default(),
                            joined_column: column.clone(),
                        });
                    }
                }
            }
            joins.sort_by(|a, b| {
                (&a.joined_table, &a.on_column, &a.joined_column).cmp(&(
                    &b.joined_table,
                    &b.on_column,
                    &b.joined_column,
                ))
            });
            joins.dedup();
            tables.insert(
                name.clone(),
                SchemaContextAnswer {
                    table: name,
                    columns: table
                        .columns
                        .iter()
                        .map(|column| SchemaColumn {
                            name: column.name.clone(),
                            type_name: column.type_name.clone(),
                            nullable: column.nullable,
                        })
                        .collect(),
                    joins,
                    ontology_class: None,
                    application: application.to_string(),
                    owner: owner.to_string(),
                    change_impact: String::new(),
                },
            );
        }
        SchemaContextCatalog { tables }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> SchemaContextCatalog {
        let mut tables = BTreeMap::new();
        tables.insert(
            "person".to_string(),
            SchemaContextAnswer {
                table: "person".to_string(),
                joins: vec![SchemaJoin {
                    joined_table: "family".to_string(),
                    on_column: "family_id".to_string(),
                    joined_column: "id".to_string(),
                }],
                columns: vec![SchemaColumn {
                    name: "family_id".to_string(),
                    type_name: "integer".to_string(),
                    nullable: false,
                }],
                ontology_class: Some("gramps:Person".to_string()),
                application: "gramps".to_string(),
                owner: "agent:gramps".to_string(),
                change_impact: "breaks family-tree rendering".to_string(),
            },
        );
        SchemaContextCatalog { tables }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R029.1
    #[test]
    fn known_table_returns_its_full_answer() {
        let answer = catalog().schema_context("person").unwrap().clone();
        assert_eq!(answer.ontology_class.as_deref(), Some("gramps:Person"));
        assert_eq!(answer.application, "gramps");
        assert_eq!(answer.joins.len(), 1);
        assert_eq!(answer.joins[0].joined_table, "family");
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R029.1
    #[test]
    fn unknown_table_is_refused_not_defaulted() {
        let err = catalog().schema_context("no_such_table").unwrap_err();
        assert_eq!(err, UnknownTable("no_such_table".to_string()));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R029.1
    #[test]
    fn empty_catalog_refuses_every_table() {
        let catalog = SchemaContextCatalog::default();
        assert!(catalog.schema_context("person").is_err());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R029.1
    #[test]
    fn answer_serializes_round_trip() {
        let answer = catalog().tables["person"].clone();
        let encoded = serde_json::to_string(&answer).unwrap();
        let decoded: SchemaContextAnswer = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, answer);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R029.2
    #[test]
    fn serve_schema_context_returns_the_full_answer_for_a_known_table() {
        let request = SchemaContextRequest {
            table: "person".to_string(),
        };
        // Round-trip the request itself, proving it is a real wire
        // boundary and not just an in-process `&str` call.
        let encoded_request = serde_json::to_string(&request).unwrap();
        let decoded_request: SchemaContextRequest = serde_json::from_str(&encoded_request).unwrap();

        let answer = serve_schema_context(&catalog(), &decoded_request).unwrap();

        assert_eq!(answer.table, "person");
        assert_eq!(answer.ontology_class.as_deref(), Some("gramps:Person"));
        assert_eq!(answer.application, "gramps");
        assert_eq!(answer.change_impact, "breaks family-tree rendering");
        assert_eq!(answer.joins.len(), 1);
        assert_eq!(answer.joins[0].joined_table, "family");
        assert_eq!(answer.joins[0].on_column, "family_id");
        assert_eq!(answer.joins[0].joined_column, "id");
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R029.2
    #[test]
    fn serve_schema_context_refuses_an_unknown_table_with_a_serializable_error() {
        let request = SchemaContextRequest {
            table: "no_such_table".to_string(),
        };

        let err = serve_schema_context(&catalog(), &request).unwrap_err();
        assert_eq!(err.table, "no_such_table");

        // The refusal itself must cross the wire: it is what the served
        // entry point returns to a caller, not an in-process-only error.
        let encoded_err = serde_json::to_string(&err).unwrap();
        let decoded_err: SchemaContextQueryError = serde_json::from_str(&encoded_err).unwrap();
        assert_eq!(decoded_err, err);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R029.2
    #[test]
    fn attached_catalog_answers_carry_columns_and_both_join_directions() {
        use {
            medulla_catalog_source_types::attached_catalog::AttachedCatalogGraph,
            medulla_catalog_source_types::attached_catalog::CatalogColumn,
            medulla_catalog_source_types::attached_catalog::CatalogConstraint,
            medulla_catalog_source_types::attached_catalog::CatalogConstraintKind,
            medulla_catalog_source_types::attached_catalog::CatalogReference,
            medulla_catalog_source_types::attached_catalog::CatalogTable,
        };
        let column = |name: &str| CatalogColumn {
            name: name.to_string(),
            type_name: "integer".to_string(),
            nullable: false,
        };
        let graph = AttachedCatalogGraph {
            tables: vec![
                CatalogTable {
                    schema: "main".to_string(),
                    name: "family".to_string(),
                    columns: vec![column("id")],
                    constraints: Vec::new(),
                },
                CatalogTable {
                    schema: "main".to_string(),
                    name: "person".to_string(),
                    columns: vec![column("id"), column("family_id")],
                    constraints: vec![CatalogConstraint {
                        name: "person_family".to_string(),
                        kind: CatalogConstraintKind::ForeignKey,
                        columns: vec!["family_id".to_string()],
                        references: Some(CatalogReference {
                            schema: "main".to_string(),
                            table: "family".to_string(),
                            columns: vec!["id".to_string()],
                        }),
                    }],
                },
            ],
            views: Vec::new(),
            version: 1,
        };
        let catalog = SchemaContextCatalog::from_attached(&graph, "gramps", "agent:gramps");
        let person = catalog.schema_context("main.person").unwrap();
        assert_eq!(person.columns.len(), 2);
        assert_eq!(person.joins.len(), 1);
        assert_eq!(person.joins[0].joined_table, "main.family");
        assert_eq!(person.joins[0].on_column, "family_id");
        // The referenced table sees the same join from its side.
        let family = catalog.schema_context("main.family").unwrap();
        assert_eq!(family.joins.len(), 1);
        assert_eq!(family.joins[0].joined_table, "main.person");
        assert_eq!(family.joins[0].on_column, "id");
        assert_eq!(family.joins[0].joined_column, "family_id");
        assert_eq!(family.application, "gramps");
        assert_eq!(family.owner, "agent:gramps");
        assert!(catalog.schema_context("main.absent").is_err());
    }
}
