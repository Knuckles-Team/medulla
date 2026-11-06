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

    /// The committed round-trip vectors of the eight non-research records, built
    /// from the serde types (MEDULLA-FINANCE-PRIMITIVES-R004.3.3.2). Set
    /// `MEDULLA_EMIT_FINANCE_VECTORS=1` to rewrite the fixture; otherwise a drifted
    /// fixture fails.
    fn record_vectors() -> serde_json::Value {
        const TENANT: &str = "integration-test-tenant";
        const MARKETS: &str = "http://example.org/markets#";
        let digest = format!("\"sha256:{}\"", "0".repeat(64));
        let iri = |name: &str| format!("<{MARKETS}{name}>");
        let common = [
            ("recordSchemaVersion", "\"finance-v1\"".to_string()),
            ("recordTenant", format!("\"{TENANT}\"")),
            ("recordSource", "\"broker-export\"".to_string()),
            (
                "recordEffectiveAt",
                "\"2026-10-01T00:00:00Z\"^^xsd:dateTime".to_string(),
            ),
            (
                "recordKnownAt",
                "\"2026-10-02T00:00:00Z\"^^xsd:dateTime".to_string(),
            ),
        ];
        let kinds: Vec<(FinanceRecordKind, Vec<(&str, String)>)> = vec![
            (
                FinanceRecordKind::Account,
                vec![
                    ("accountId", "\"acct-1\"".into()),
                    ("baseCurrency", "\"USD\"".into()),
                    ("accountKind", "\"taxable\"".into()),
                ],
            ),
            (
                FinanceRecordKind::Activity,
                vec![
                    ("activityPosition", iri("pos1")),
                    ("activityKind", "\"buy\"".into()),
                    ("activityQuantity", "\"10\"^^xsd:decimal".into()),
                    (
                        "activityTradeDate",
                        "\"2026-10-01T14:30:00Z\"^^xsd:dateTime".into(),
                    ),
                ],
            ),
            (
                FinanceRecordKind::Lot,
                vec![
                    ("lotPosition", iri("pos1")),
                    ("lotOpenedBy", iri("act1")),
                    ("lotQuantity", "\"10\"^^xsd:decimal".into()),
                    ("lotUnitCostBasis", "\"100.00\"^^xsd:decimal".into()),
                ],
            ),
            (
                FinanceRecordKind::Position,
                vec![
                    ("positionAccount", iri("acct1")),
                    ("positionInstrument", iri("btc")),
                    ("positionQuantity", "\"10\"^^xsd:decimal".into()),
                    ("costBasisMethod", "\"fifo\"".into()),
                ],
            ),
            (
                FinanceRecordKind::PortfolioGroup,
                vec![
                    ("portfolioGroupName", "\"Retirement\"".into()),
                    ("portfolioView", "\"holdings\"".into()),
                    ("portfolioMember", iri("acct1")),
                ],
            ),
            (
                FinanceRecordKind::StrategySpec,
                vec![
                    ("strategySpecId", "\"dca-weekly\"".into()),
                    ("strategySpecVersion", "\"1\"^^xsd:integer".into()),
                    ("strategySpecKind", "\"dca_fixed_amount\"".into()),
                    ("strategySpecDigest", digest.clone()),
                    ("strategySpecUniverse", iri("btc")),
                    ("strategySpecCurrency", "\"USD\"".into()),
                ],
            ),
            (
                FinanceRecordKind::StrategyRun,
                vec![
                    ("strategyRunStatus", "\"proposed\"".into()),
                    ("strategyRunInputDigest", digest),
                ],
            ),
            (
                FinanceRecordKind::LeverageTerms,
                vec![
                    ("leverageInstrument", iri("btc")),
                    ("leverageProductType", "\"margin\"".into()),
                    ("leveragePolicyVersion", "\"us-2026-10\"".into()),
                    ("leverageMultiplier", "\"2.0\"^^xsd:decimal".into()),
                    ("leverageCurrency", "\"USD\"".into()),
                    ("leverageInitialMargin", "\"5000.00\"^^xsd:decimal".into()),
                    (
                        "leverageMaintenanceMargin",
                        "\"4000.00\"^^xsd:decimal".into(),
                    ),
                ],
            ),
        ];
        let vectors: Vec<serde_json::Value> = kinds
            .into_iter()
            .map(|(kind, fields)| {
                let record = FinanceRecord {
                    record_id: format!("rec_{}", kind.class_name().to_lowercase()),
                    tenant: TENANT.to_string(),
                    fields: fields
                        .into_iter()
                        .chain(common.iter().cloned())
                        .map(|(name, value)| (name.to_string(), value))
                        .collect(),
                };
                serde_json::json!({
                    "kind": kind,
                    "schema_version": FINANCE_V1_SCHEMA_VERSION,
                    "client_writable": kind.policy() == FinanceRecordPolicy::ClientWritable,
                    "record": record,
                })
            })
            .collect();
        serde_json::json!({ "vectors": vectors })
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.3.2
    #[test]
    fn emit_record_vectors() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/finance_record_vectors.json");
        let generated = record_vectors();
        if std::env::var_os("MEDULLA_EMIT_FINANCE_VECTORS").is_some() {
            let text = serde_json::to_string_pretty(&generated).expect("vectors serialise");
            std::fs::write(&path, format!("{text}\n")).expect("write fixture");
        }
        let committed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture present"))
                .expect("fixture is JSON");
        assert_eq!(
            committed, generated,
            "tests/fixtures/finance_record_vectors.json drifted from the serde types; rerun with MEDULLA_EMIT_FINANCE_VECTORS=1"
        );
        for vector in generated["vectors"].as_array().expect("vectors") {
            let record: FinanceRecord =
                serde_json::from_value(vector["record"].clone()).expect("record decodes");
            assert_eq!(
                serde_json::to_value(&record).expect("record encodes"),
                vector["record"]
            );
        }
    }
}
