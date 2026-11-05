//! Finance wire DTOs.

#[cfg(feature = "finance")]
use serde::{Deserialize, Serialize};

// ── finance ────────────────────────────────────────────────────────────────

/// A single order in the book (matched by `medulla-compute::finance::exchange`).
#[cfg(feature = "finance")]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Order {
    pub id: String,
    pub side: String, // "buy" or "sell"
    pub price: f64,
    pub quantity: f64,
    pub timestamp: u64,
}

/// One fiscal year of standardized financial-statement inputs (forensic scores).
#[cfg(feature = "finance")]
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct YearData {
    pub sales: f64,
    pub cogs: f64,
    pub sga: f64,
    pub net_income: f64,
    pub cfo: f64, // operating cash flow
    pub receivables: f64,
    pub current_assets: f64,
    pub current_liabilities: f64,
    pub ppe_net: f64,
    pub depreciation: f64,
    pub total_assets: f64,
    pub total_liabilities: f64,
    pub long_term_debt: f64,
    pub retained_earnings: f64,
    pub ebit: f64,
    pub market_cap: f64,
    pub shares: f64,
}

// ── finance-v1 record-type wire version gate (MEDULLA-FINANCE-PRIMITIVES-R004.3) ─

/// The finance-v1 record schema version this server accepts over the wire.
#[cfg(feature = "finance")]
pub const FINANCE_V1_SCHEMA_VERSION: &str = "finance-v1";

/// Why a finance-v1 record was refused at the wire boundary.
#[cfg(feature = "finance")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinanceRecordWireRefusal {
    pub reason: String,
}

/// Admit a finance-v1 record's declared schema version, or refuse it
/// (MEDULLA-FINANCE-PRIMITIVES-R004.3): server wiring accepts only the current
/// `finance-v1` schema version; an unknown or empty declared version is
/// refused rather than silently accepted or guessed at as a migration.
#[cfg(feature = "finance")]
pub fn admit_finance_record_version(
    declared_version: &str,
) -> Result<(), FinanceRecordWireRefusal> {
    if declared_version == FINANCE_V1_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(FinanceRecordWireRefusal {
            reason: format!(
                "unknown finance record schema version '{declared_version}'; only '{FINANCE_V1_SCHEMA_VERSION}' is accepted"
            ),
        })
    }
}

// ── served finance records (MEDULLA-FINANCE-PRIMITIVES-R004.3.2) ─────────────

/// The finance-v1 record kinds served by `FinanceRecordPut` / `FinanceRecordGet`.
/// Each kind names the SHACL node shape `<Kind>Shape` and the ontology class
/// `<Kind>` of `crates/medulla-core/ontology/finance-v1.shapes.ttl`.
#[cfg(feature = "finance")]
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FinanceRecordKind {
    Account,
    Activity,
    Lot,
    Position,
    PortfolioGroup,
    StrategySpec,
    StrategyRun,
    LeverageTerms,
}

#[cfg(feature = "finance")]
impl FinanceRecordKind {
    /// The ontology class local name (`:Account`, ...), also the shape stem.
    pub fn class_name(self) -> &'static str {
        match self {
            Self::Account => "Account",
            Self::Activity => "Activity",
            Self::Lot => "Lot",
            Self::Position => "Position",
            Self::PortfolioGroup => "PortfolioGroup",
            Self::StrategySpec => "StrategySpec",
            Self::StrategyRun => "StrategyRun",
            Self::LeverageTerms => "LeverageTerms",
        }
    }

    /// The graph node id of one record of this kind.
    pub fn node_id(self, record_id: &str) -> String {
        format!("finance:{}:{record_id}", self.class_name())
    }

    /// This kind's row of [`FINANCE_RECORD_POLICY`].
    pub fn policy(self) -> FinanceRecordPolicy {
        FINANCE_RECORD_POLICY
            .iter()
            .find(|(kind, _)| *kind == self)
            .map(|(_, policy)| *policy)
            .expect("FINANCE_RECORD_POLICY lists every FinanceRecordKind")
    }
}

/// Who may write a finance record kind.
#[cfg(feature = "finance")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinanceRecordPolicy {
    /// A client puts the record through `FinanceRecordPut`.
    ClientWritable,
    /// The engine derives the record (accounting projection or strategy
    /// evaluator); a client put is refused with `RECORD_DERIVED_READONLY`.
    Derived,
}

/// The per-kind write policy of served finance records.
#[cfg(feature = "finance")]
pub const FINANCE_RECORD_POLICY: &[(FinanceRecordKind, FinanceRecordPolicy)] = &[
    (
        FinanceRecordKind::Account,
        FinanceRecordPolicy::ClientWritable,
    ),
    (
        FinanceRecordKind::Activity,
        FinanceRecordPolicy::ClientWritable,
    ),
    (FinanceRecordKind::Lot, FinanceRecordPolicy::Derived),
    (FinanceRecordKind::Position, FinanceRecordPolicy::Derived),
    (
        FinanceRecordKind::PortfolioGroup,
        FinanceRecordPolicy::ClientWritable,
    ),
    (
        FinanceRecordKind::StrategySpec,
        FinanceRecordPolicy::ClientWritable,
    ),
    (FinanceRecordKind::StrategyRun, FinanceRecordPolicy::Derived),
    (
        FinanceRecordKind::LeverageTerms,
        FinanceRecordPolicy::ClientWritable,
    ),
];

/// One finance-v1 record on the wire. `fields` maps a finance ontology property
/// local name (`accountId`, `baseCurrency`, ...) to its value as a Turtle object
/// term (`"USD"`, `"2.0"^^xsd:decimal`, `<http://example.org/x>`), exactly what
/// the SHACL shape validates.
#[cfg(feature = "finance")]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FinanceRecord {
    pub record_id: String,
    /// The tenant the record belongs to; must be the verified request tenant.
    pub tenant: String,
    pub fields: std::collections::BTreeMap<String, String>,
}

/// Refusal detail codes of the served finance record methods.
#[cfg(feature = "finance")]
pub mod finance_record_refusal {
    pub const VERSION_UNKNOWN: &str = "FINANCE_RECORD_VERSION_UNKNOWN";
    pub const DERIVED_READONLY: &str = "RECORD_DERIVED_READONLY";
    pub const HEAD_CONFLICT: &str = "RECORD_HEAD_CONFLICT";
    pub const SHAPE_INVALID: &str = "RECORD_SHAPE_INVALID";
}

#[cfg(all(test, feature = "finance"))]
mod finance_record_version_tests {
    use super::*;

    // spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.1
    #[test]
    fn the_current_schema_version_is_admitted() {
        assert!(admit_finance_record_version(FINANCE_V1_SCHEMA_VERSION).is_ok());
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.1
    #[test]
    fn an_unknown_schema_version_is_refused() {
        let refusal = admit_finance_record_version("finance-v0").unwrap_err();
        assert!(refusal.reason.contains("finance-v0"));
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.1
    #[test]
    fn an_empty_schema_version_is_refused() {
        assert!(admit_finance_record_version("").is_err());
    }
}
