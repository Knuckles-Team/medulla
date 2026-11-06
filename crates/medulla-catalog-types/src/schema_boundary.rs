//! Authorization-segmented schema projections (MEDULLA-FEDERATED-QUERY-R724).
//!
//! A [`SchemaBoundary`] is one tenant's authorization boundary over a set of
//! tables exposed by one virtual mapping. Records are graph nodes of the tenant
//! graph, one per `(tenant, boundary_id)`, written through
//! `Method::SchemaProjections` with [`SchemaProjectionsOp::Put`] on the mutation
//! gateway. [`SchemaProjectionsOp::List`] returns only the boundaries whose
//! `required_scopes` the verified caller holds. Pure data and bounds; the
//! handlers live in the server.

use serde::{Deserialize, Serialize};

/// The node `type` (label) of a stored boundary record.
pub const SCHEMA_BOUNDARY_LABEL: &str = "SchemaBoundary";
/// The node property holding the opaque verified tenant scope of the writer.
pub const SCHEMA_BOUNDARY_TENANT_KEY: &str = "schema_boundary_tenant_scope";
/// The operator scope that may see the tenant's total boundary count.
pub const BOUNDARY_COUNT_SCOPE: &str = "audit:operator";

/// Inclusive `(min, max)` element count of `required_scopes`.
pub const REQUIRED_SCOPES_BOUNDS: (usize, usize) = (1, 16);
/// Inclusive `(min, max)` element count of `tables`.
pub const TABLES_BOUNDS: (usize, usize) = (1, 1024);
/// The longest accepted `boundary_id`, `mapping_ref` or table name, in bytes.
pub const NAME_MAX_BYTES: usize = 256;

/// One authorization boundary of a tenant schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaBoundary {
    /// The boundary id, unique within the tenant.
    pub boundary_id: String,
    /// Every scope a caller must hold to see this boundary (1 to 16 registered names).
    pub required_scopes: Vec<String>,
    /// The tables the boundary covers (1 to 1024 names).
    pub tables: Vec<String>,
    /// The id of the `vg:VirtualMapping` node that exposes the tables.
    pub mapping_ref: String,
}

/// Why a boundary record is refused before any lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryShapeError {
    /// `boundary_id`, `mapping_ref` or a table name is empty or too long.
    Name,
    /// `required_scopes` is outside [`REQUIRED_SCOPES_BOUNDS`].
    ScopeCount,
    /// `tables` is outside [`TABLES_BOUNDS`].
    TableCount,
}

fn name_ok(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= NAME_MAX_BYTES
}

fn within((min, max): (usize, usize), len: usize) -> bool {
    (min..=max).contains(&len)
}

impl SchemaBoundary {
    /// Check the record's bounds (names, counts). Scope registration and the
    /// mapping lookup are the server's checks.
    pub fn check_shape(&self) -> Result<(), BoundaryShapeError> {
        let names_ok = name_ok(&self.boundary_id)
            && name_ok(&self.mapping_ref)
            && self.tables.iter().all(|table| name_ok(table));
        if !names_ok {
            return Err(BoundaryShapeError::Name);
        }
        if !within(REQUIRED_SCOPES_BOUNDS, self.required_scopes.len()) {
            return Err(BoundaryShapeError::ScopeCount);
        }
        if !within(TABLES_BOUNDS, self.tables.len()) {
            return Err(BoundaryShapeError::TableCount);
        }
        Ok(())
    }

    /// The node id of this boundary under one opaque tenant scope.
    pub fn node_id(tenant_scope: &str, boundary_id: &str) -> String {
        format!("{SCHEMA_BOUNDARY_LABEL}:{tenant_scope}:{boundary_id}")
    }

    /// Whether a caller holding the sorted `scopes` covers every required scope.
    pub fn covered_by(&self, sorted_scopes: &[String]) -> bool {
        self.required_scopes
            .iter()
            .all(|scope| sorted_scopes.binary_search(scope).is_ok())
    }
}

/// One schema projections operation (`Method::SchemaProjections`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SchemaProjectionsOp {
    /// Store (or replace) one boundary record of the verified tenant.
    Put {
        /// The record to store.
        boundary: SchemaBoundary,
    },
    /// List the boundaries the verified caller's scopes cover.
    List,
}

impl SchemaProjectionsOp {
    /// Whether this operation changes durable state.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Put { .. })
    }
}

/// The answer of [`SchemaProjectionsOp::List`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaProjections {
    /// The covered boundaries, sorted by `boundary_id`.
    pub entries: Vec<SchemaBoundary>,
    /// The tenant's total boundary count; present only for `audit:operator`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary_count: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary(scopes: &[&str], tables: usize) -> SchemaBoundary {
        SchemaBoundary {
            boundary_id: "sales".into(),
            required_scopes: scopes.iter().map(|s| s.to_string()).collect(),
            tables: (0..tables).map(|i| format!("t{i}")).collect(),
            mapping_ref: "urn:mapping".into(),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R724
    #[test]
    fn shape_bounds_and_coverage() {
        let cases: &[(SchemaBoundary, Result<(), BoundaryShapeError>)] = &[
            (boundary(&["kg:read"], 1), Ok(())),
            (boundary(&[], 1), Err(BoundaryShapeError::ScopeCount)),
            (
                boundary(&["kg:read"; 17], 1),
                Err(BoundaryShapeError::ScopeCount),
            ),
            (
                boundary(&["kg:read"], 0),
                Err(BoundaryShapeError::TableCount),
            ),
            (
                boundary(&["kg:read"], 1025),
                Err(BoundaryShapeError::TableCount),
            ),
        ];
        for (record, expected) in cases {
            assert_eq!(record.check_shape(), *expected);
        }
        let held = vec!["kg:read".to_string(), "sales:read".to_string()];
        assert!(boundary(&["sales:read"], 1).covered_by(&held));
        assert!(!boundary(&["hr:read"], 1).covered_by(&held));
    }
}
