//! PostgreSQL wire protocol operational parity (MEDULLA-DURABLE-KERNEL-R035): the
//! closed set of required operational surfaces (pg_dump/pg_restore,
//! replication, pg_stat views, pg_locks, EXPLAIN, query cancellation, pooler
//! transaction mode) and a fail-closed compatibility matrix. This is the
//! typed model slice (`.1`): no surface is ever assumed supported. The real
//! pg_dump/pg_restore/replication/pg_stat/EXPLAIN/pooler implementations are
//! later children.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A required PostgreSQL wire-protocol operational surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PgOperationalSurface {
    PgDump,
    PgRestore,
    Replication,
    PgStatViews,
    PgLocks,
    Explain,
    QueryCancellation,
    PoolerTransactionMode,
}

impl PgOperationalSurface {
    pub const ALL: [PgOperationalSurface; 8] = [
        PgOperationalSurface::PgDump,
        PgOperationalSurface::PgRestore,
        PgOperationalSurface::Replication,
        PgOperationalSurface::PgStatViews,
        PgOperationalSurface::PgLocks,
        PgOperationalSurface::Explain,
        PgOperationalSurface::QueryCancellation,
        PgOperationalSurface::PoolerTransactionMode,
    ];
}

/// The category of one SQL form in the compatibility matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqlFormKind {
    Statement,
    Function,
    Operator,
    Type,
}

/// How far the engine serves one SQL form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqlFormState {
    Supported,
    Partial,
    Refused,
}

/// One row of the SQL dialect compatibility matrix
/// (MEDULLA-ENGINE-PARITY-R009.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlFormEntry {
    pub name: String,
    pub kind: SqlFormKind,
    pub state: SqlFormState,
}

impl SqlFormEntry {
    /// A new entry: the form starts `Refused` and reaches `target` through the
    /// `SqlFormMachine` event that leads there (MEDULLA-ENGINE-PARITY-R021.2).
    pub fn served_as(name: &str, kind: SqlFormKind, target: SqlFormState) -> Self {
        Self {
            name: name.to_string(),
            kind,
            state: SqlFormState::serve_as(target),
        }
    }
}

/// Statement forms the classifier serves today, one per `StatementKind`
/// variant (`StatementKind::form_name` in `medulla-sql-lang`).
pub const SERVED_STATEMENT_FORMS: &[(&str, SqlFormState)] = &[
    ("read", SqlFormState::Supported),
    ("explain", SqlFormState::Supported),
    ("insert_nodes", SqlFormState::Supported),
    ("insert_nodes_select", SqlFormState::Supported),
    ("update_nodes", SqlFormState::Supported),
    ("update_nodes_join", SqlFormState::Supported),
    ("delete_nodes", SqlFormState::Supported),
    ("delete_nodes_join", SqlFormState::Supported),
    ("create_table", SqlFormState::Supported),
    ("drop_table", SqlFormState::Supported),
    ("alter_table", SqlFormState::Partial),
    ("create_view", SqlFormState::Supported),
    ("drop_view", SqlFormState::Supported),
    ("insert_table", SqlFormState::Supported),
    ("insert_select", SqlFormState::Supported),
    ("update_table", SqlFormState::Partial),
    ("delete_table", SqlFormState::Partial),
    ("property_graph_ddl", SqlFormState::Partial),
    ("property_graph_privilege", SqlFormState::Partial),
    ("graph_table_read", SqlFormState::Partial),
    ("begin", SqlFormState::Supported),
    ("commit", SqlFormState::Supported),
    ("rollback", SqlFormState::Supported),
    ("discard_all", SqlFormState::Supported),
    ("copy_in", SqlFormState::Supported),
    ("copy_out", SqlFormState::Supported),
    ("create_extension", SqlFormState::Partial),
    ("drop_extension", SqlFormState::Partial),
    ("create_sequence", SqlFormState::Supported),
    ("drop_sequence", SqlFormState::Supported),
    ("create_trigger", SqlFormState::Supported),
    ("drop_trigger", SqlFormState::Supported),
    ("create_ann_index", SqlFormState::Supported),
    ("create_index", SqlFormState::Supported),
    ("drop_index", SqlFormState::Supported),
    ("create_hypertable", SqlFormState::Partial),
    ("create_continuous_aggregate", SqlFormState::Partial),
    ("create_function", SqlFormState::Partial),
    ("drop_function", SqlFormState::Partial),
];

/// The operational surfaces the engine serves today. `PgCompatibilityMatrix::with_served_forms`
/// declares every surface listed here; each R035 child adds its own surface.
pub const SERVED_OPERATIONAL_SURFACES: &[PgOperationalSurface] = &[PgOperationalSurface::Explain];

/// Scalar and aggregate functions the engine registers today.
pub const SERVED_FUNCTIONS: &[&str] = &[
    "json_get",
    "json_get_f64",
    "json_get_i64",
    "time_bucket",
    "greatest",
    "least",
    "md5",
    "sha1",
    "sha256",
    "base64_encode",
    "base64_decode",
    "int4range",
    "tsrange",
];

/// The operational surfaces the wire listener serves today. Each R035 child that
/// delivers a surface adds it here, and [`PgCompatibilityMatrix::with_served_forms`]
/// declares every surface this list names.
pub const SERVED_OPERATIONAL_SURFACES: &[PgOperationalSurface] = &[
    PgOperationalSurface::QueryCancellation,
    PgOperationalSurface::PoolerTransactionMode,
];

/// A parity check named one or more surfaces that are not declared
/// supported. Carries every missing surface, not just the first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedSurfaces(pub Vec<PgOperationalSurface>);

impl fmt::Display for UnsupportedSurfaces {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported PostgreSQL operational surfaces: {:?}",
            self.0
        )
    }
}

impl std::error::Error for UnsupportedSurfaces {}

/// Tracks which operational surfaces this engine declares supported. Starts
/// fail-closed: [`PgCompatibilityMatrix::fail_closed`] is the only
/// constructor, and there is no bulk "declare all supported" method, so
/// claiming parity requires naming each surface explicitly.
#[derive(Clone, Debug, Default)]
pub struct PgCompatibilityMatrix {
    supported: BTreeSet<PgOperationalSurface>,
    /// The SQL form table. Closed: a form absent from it is `Refused`.
    pub forms: Vec<SqlFormEntry>,
}

impl PgCompatibilityMatrix {
    pub fn fail_closed() -> Self {
        Self {
            supported: BTreeSet::new(),
            forms: Vec::new(),
        }
    }

    /// The matrix with the form table filled from what the classifier and
    /// the function registry serve today.
    pub fn with_served_forms() -> Self {
        let mut m = Self::fail_closed();
        for (name, state) in SERVED_STATEMENT_FORMS {
            m.forms.push(SqlFormEntry::served_as(
                name,
                SqlFormKind::Statement,
                *state,
            ));
        }
        for name in SERVED_FUNCTIONS {
            m.forms.push(SqlFormEntry::served_as(
                name,
                SqlFormKind::Function,
                SqlFormState::Supported,
            ));
        }
        for surface in SERVED_OPERATIONAL_SURFACES {
            m.declare_supported(*surface);
        }
        m
    }

    /// State of a named form; `None` when the matrix has no entry. Callers
    /// treat `None` as `Refused` (the matrix is closed).
    pub fn form_state(&self, name: &str) -> Option<SqlFormState> {
        self.forms.iter().find(|f| f.name == name).map(|f| f.state)
    }

    /// Closed resolution: an absent form is `Refused`.
    pub fn resolve_form(&self, name: &str) -> SqlFormState {
        self.form_state(name).unwrap_or(SqlFormState::Refused)
    }

    pub fn declare_supported(&mut self, surface: PgOperationalSurface) {
        self.supported.insert(surface);
    }

    pub fn is_supported(&self, surface: PgOperationalSurface) -> bool {
        self.supported.contains(&surface)
    }

    /// Refuses (naming every missing surface) unless every surface in
    /// `required` is declared supported.
    pub fn require_all(
        &self,
        required: &[PgOperationalSurface],
    ) -> Result<(), UnsupportedSurfaces> {
        let missing: Vec<PgOperationalSurface> = required
            .iter()
            .copied()
            .filter(|s| !self.is_supported(*s))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(UnsupportedSurfaces(missing))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R035.1
    #[test]
    fn surfaces_round_trip_through_their_wire_name() {
        for (surface, name) in [
            (PgOperationalSurface::PgDump, "pg_dump"),
            (PgOperationalSurface::PgRestore, "pg_restore"),
            (PgOperationalSurface::Replication, "replication"),
            (PgOperationalSurface::PgStatViews, "pg_stat_views"),
            (PgOperationalSurface::PgLocks, "pg_locks"),
            (PgOperationalSurface::Explain, "explain"),
            (
                PgOperationalSurface::QueryCancellation,
                "query_cancellation",
            ),
            (
                PgOperationalSurface::PoolerTransactionMode,
                "pooler_transaction_mode",
            ),
        ] {
            let wire = serde_json::to_string(&surface).unwrap();
            assert_eq!(wire, format!("\"{name}\""));
            let back: PgOperationalSurface = serde_json::from_str(&wire).unwrap();
            assert_eq!(back, surface);
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.1
    #[test]
    fn fresh_matrix_reports_every_surface_unsupported() {
        let matrix = PgCompatibilityMatrix::fail_closed();
        for surface in PgOperationalSurface::ALL {
            assert!(!matrix.is_supported(surface));
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.1
    #[test]
    fn parity_check_is_refused_and_names_exactly_the_missing_surfaces() {
        let mut matrix = PgCompatibilityMatrix::fail_closed();
        matrix.declare_supported(PgOperationalSurface::PgDump);
        matrix.declare_supported(PgOperationalSurface::Explain);
        let err = matrix.require_all(&PgOperationalSurface::ALL).unwrap_err();
        assert_eq!(err.0.len(), PgOperationalSurface::ALL.len() - 2);
        assert!(!err.0.contains(&PgOperationalSurface::PgDump));
        assert!(!err.0.contains(&PgOperationalSurface::Explain));
        assert!(err.0.contains(&PgOperationalSurface::PgRestore));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.1
    #[test]
    fn parity_check_succeeds_once_every_surface_is_declared() {
        let mut matrix = PgCompatibilityMatrix::fail_closed();
        for surface in PgOperationalSurface::ALL {
            matrix.declare_supported(surface);
        }
        assert!(matrix.require_all(&PgOperationalSurface::ALL).is_ok());
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.5
    #[test]
    fn served_matrix_declares_every_served_operational_surface() {
        let m = PgCompatibilityMatrix::with_served_forms();
        for surface in SERVED_OPERATIONAL_SURFACES {
            assert!(m.is_supported(*surface), "{surface:?} is served but undeclared");
        }
        assert!(m.is_supported(PgOperationalSurface::QueryCancellation));
        assert!(!PgCompatibilityMatrix::fail_closed()
            .is_supported(PgOperationalSurface::QueryCancellation));
    }

    // spec: MEDULLA-ENGINE-PARITY-R009.1
    #[test]
    fn absent_form_resolves_to_refused() {
        let m = PgCompatibilityMatrix::with_served_forms();
        assert_eq!(m.form_state("read"), Some(SqlFormState::Supported));
        assert_eq!(m.form_state("create_policy"), None);
        assert_eq!(m.resolve_form("create_policy"), SqlFormState::Refused);
        assert_eq!(
            PgCompatibilityMatrix::fail_closed().resolve_form("read"),
            SqlFormState::Refused
        );
    }
}
