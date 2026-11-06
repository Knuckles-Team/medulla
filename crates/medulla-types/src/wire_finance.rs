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
    // Strategy research records (MEDULLA-FINANCE-PRIMITIVES-R751.750): create-only.
    DebateRound,
    TradeJournalEntry,
    SentimentFusion,
    GeopoliticalRisk,
    StrategyCard,
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
            Self::DebateRound => "DebateRound",
            Self::TradeJournalEntry => "TradeJournalEntry",
            Self::SentimentFusion => "SentimentFusion",
            Self::GeopoliticalRisk => "GeopoliticalRisk",
            Self::StrategyCard => "StrategyCard",
        }
    }

    /// The graph node id of one record of this kind.
    pub fn node_id(self, record_id: &str) -> String {
        format!("finance:{}:{record_id}", self.class_name())
    }

    /// Whether this kind is a strategy research record (MEDULLA-FINANCE-PRIMITIVES-R751.750):
    /// its `fields` hold JSON text and the write handler computes its scores.
    pub fn is_research(self) -> bool {
        self.policy() == FinanceRecordPolicy::CreateOnly
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
    /// A client creates the record once; a rewrite is refused with
    /// `RECORD_IMMUTABLE` (the strategy research records).
    CreateOnly,
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
    (
        FinanceRecordKind::DebateRound,
        FinanceRecordPolicy::CreateOnly,
    ),
    (
        FinanceRecordKind::TradeJournalEntry,
        FinanceRecordPolicy::CreateOnly,
    ),
    (
        FinanceRecordKind::SentimentFusion,
        FinanceRecordPolicy::CreateOnly,
    ),
    (
        FinanceRecordKind::GeopoliticalRisk,
        FinanceRecordPolicy::CreateOnly,
    ),
    (
        FinanceRecordKind::StrategyCard,
        FinanceRecordPolicy::CreateOnly,
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

/// The closed refusal codes of `FinanceRecordPut`; each is a registered
/// `ServerErrorCode` (MEDULLA-FINANCE-PRIMITIVES-R004.3.2).
#[cfg(feature = "finance")]
pub mod finance_record_refusal {
    pub const VERSION_UNKNOWN: &str = "FINANCE_RECORD_VERSION_UNKNOWN";
    pub const DERIVED_READONLY: &str = "RECORD_DERIVED_READONLY";
    pub const HEAD_CONFLICT: &str = "RECORD_HEAD_CONFLICT";
    pub const SHAPE_INVALID: &str = "RECORD_SHAPE_INVALID";
    // Strategy research records (MEDULLA-FINANCE-PRIMITIVES-R751.752).
    pub const SCORE_EVIDENCE_MISSING: &str = "SCORE_EVIDENCE_MISSING";
    pub const SCORE_RANGE_INVALID: &str = "SCORE_RANGE_INVALID";
    pub const SCORE_WEIGHTS_INVALID: &str = "SCORE_WEIGHTS_INVALID";
    pub const DEBATE_ROUND_OUT_OF_ORDER: &str = "DEBATE_ROUND_OUT_OF_ORDER";
    pub const STRATEGY_CARD_LICENCE_MISSING: &str = "STRATEGY_CARD_LICENCE_MISSING";
    pub const RECORD_TENANT_MISMATCH: &str = "RECORD_TENANT_MISMATCH";
    pub const RECORD_IMMUTABLE: &str = "RECORD_IMMUTABLE";
}

#[cfg(all(test, feature = "finance"))]
mod finance_record_version_tests {
    use super::*;

    #[test]
    fn record_refusals_are_registered_server_error_codes() {
        use crate::contract::ServerErrorCode;
        for code in [
            finance_record_refusal::VERSION_UNKNOWN,
            finance_record_refusal::DERIVED_READONLY,
            finance_record_refusal::HEAD_CONFLICT,
            finance_record_refusal::SHAPE_INVALID,
            finance_record_refusal::SCORE_EVIDENCE_MISSING,
            finance_record_refusal::SCORE_RANGE_INVALID,
            finance_record_refusal::SCORE_WEIGHTS_INVALID,
            finance_record_refusal::DEBATE_ROUND_OUT_OF_ORDER,
            finance_record_refusal::STRATEGY_CARD_LICENCE_MISSING,
            finance_record_refusal::RECORD_TENANT_MISMATCH,
            finance_record_refusal::RECORD_IMMUTABLE,
        ] {
            assert!(
                ServerErrorCode::ALL
                    .iter()
                    .any(|item| item.as_str() == code),
                "{code} is not a ServerErrorCode"
            );
        }
    }

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
