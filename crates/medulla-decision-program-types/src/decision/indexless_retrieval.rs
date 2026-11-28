//! Indexless retrieval over medulla-stored documents
//! (MEDULLA-DECISION-ENGINE-R144.1, R144.2).
//!
//! An [`IndexlessRequest`] runs 1 to 64 [`IndexlessOp`]s -- a raw document
//! read, an id-ordered corpus scan, or a byte-span digest check -- under an
//! [`IndexlessBudget`]. The pure [`BudgetMeter`] admits each operation before
//! it runs and yields the [`CostReport`]. Connector-owned corpora are out of
//! scope; they use the SDK raw-source tools through federated pushdown.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use medulla_query_types::indexless_cost::{CostReport, IndexlessBudget, MAX_INDEXLESS_OPS};
use medulla_types::contract::BoundedVec;

use super::knob_families::indexless::{
    MAX_COST_UNITS, MAX_ROWS, MAX_VISITS, READ_COST, SCAN_COST, VERIFY_COST,
};
use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_statistical_types::decision::policy::DecisionPolicy;
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

/// The stable refusals of the `indexless` family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IndexlessErrorCode {
    /// The request carries no operation.
    IndexlessOpsEmpty,
    /// A span is not on UTF-8 character boundaries, is out of range, or its digest differs.
    IndexlessSpanInvalid,
    /// An operation would exceed the applied budget.
    IndexlessBudgetExhausted,
}

/// The `indexless` family table: every refusal code above, registered with the
/// decision plane's module tables (MEDULLA-DECISION-ENGINE-R144.1, R144.2).
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("INDEXLESS_OPS_EMPTY"),
    RefusalCode::new("INDEXLESS_SPAN_INVALID"),
    RefusalCode::new("INDEXLESS_BUDGET_EXHAUSTED"),
];

impl IndexlessErrorCode {
    /// The stable wire code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IndexlessOpsEmpty => "INDEXLESS_OPS_EMPTY",
            Self::IndexlessSpanInvalid => "INDEXLESS_SPAN_INVALID",
            Self::IndexlessBudgetExhausted => "INDEXLESS_BUDGET_EXHAUSTED",
        }
    }
}

/// One indexless operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IndexlessOp {
    /// Return one document's text.
    RawRead { document_id: String },
    /// Enumerate `Document` nodes in id order.
    CorpusScan { limit: u64 },
    /// Check that `[start_byte, end_byte)` is on character boundaries and its
    /// SHA-256 (64 lowercase hex characters) equals `expected_digest`.
    VerifySpan {
        document_id: String,
        start_byte: u64,
        end_byte: u64,
        expected_digest: String,
    },
}

/// One indexless retrieval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexlessRequest {
    pub tenant_id: String,
    pub graph: String,
    pub ops: BoundedVec<IndexlessOp, MAX_INDEXLESS_OPS>,
    #[serde(default)]
    pub budget: Option<IndexlessBudget>,
    /// The decision policy the knobs are read from: the engine default, or a
    /// pinned published policy of the caller's tenant.
    #[serde(default = "default_policy_ref")]
    pub policy: DecisionPolicyRef,
}

fn default_policy_ref() -> DecisionPolicyRef {
    DecisionPolicyRef::Default
}

impl IndexlessRequest {
    /// The validating check: 1 to 64 operations.
    pub fn checked(self) -> Result<Self, IndexlessErrorCode> {
        if self.ops.is_empty() {
            return Err(IndexlessErrorCode::IndexlessOpsEmpty);
        }
        Ok(self)
    }
}

/// The rows the operations returned and what they cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexlessResult {
    pub rows: Vec<serde_json::Value>,
    pub cost: CostReport,
}

/// `Ok` when `[start, end)` lies on character boundaries of `text` and the
/// slice's SHA-256 hex equals `expected_digest`.
pub fn verify_span(
    text: &str,
    start: u64,
    end: u64,
    expected_digest: &str,
) -> Result<(), IndexlessErrorCode> {
    let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
        return Err(IndexlessErrorCode::IndexlessSpanInvalid);
    };
    let slice = (start <= end)
        .then(|| text.get(start..end))
        .flatten()
        .ok_or(IndexlessErrorCode::IndexlessSpanInvalid)?;
    let digest = hex::encode(Sha256::digest(slice.as_bytes()));
    (digest == expected_digest)
        .then_some(())
        .ok_or(IndexlessErrorCode::IndexlessSpanInvalid)
}

/// The per-unit costs of the three operations, read from the policy
/// (MEDULLA-DECISION-ENGINE-R144.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexlessUnitCosts {
    /// Cost units per document a `RawRead` returns.
    pub read: u64,
    /// Cost units per row a `CorpusScan` returns.
    pub scan: u64,
    /// Cost units per span a `VerifySpan` checks.
    pub verify: u64,
}

fn integer_knob(policy: &DecisionPolicy, spec: &KnobSpec) -> u64 {
    match policy.knob(spec) {
        KnobValue::Integer(n) => u64::try_from(n).unwrap_or(0),
        _ => 0,
    }
}

/// The unit costs the policy sets: each `indexless.*_cost` knob, pinned or default.
pub fn unit_costs(policy: &DecisionPolicy) -> IndexlessUnitCosts {
    IndexlessUnitCosts {
        read: integer_knob(policy, &READ_COST),
        scan: integer_knob(policy, &SCAN_COST),
        verify: integer_knob(policy, &VERIFY_COST),
    }
}

/// The stable id of the operation at `index`: `<index>:<op kind>`
/// (MEDULLA-DECISION-ENGINE-R144.5).
pub fn operation_id(index: usize, op: &IndexlessOp) -> String {
    let kind = match op {
        IndexlessOp::RawRead { .. } => "raw_read",
        IndexlessOp::CorpusScan { .. } => "corpus_scan",
        IndexlessOp::VerifySpan { .. } => "verify_span",
    };
    format!("{index}:{kind}")
}

/// The tenant ceiling the pinned policy sets: the three `indexless.*` knobs,
/// each the pinned value or its default (MEDULLA-DECISION-ENGINE-R144.7).
pub fn tenant_ceiling(policy: &DecisionPolicy) -> IndexlessBudget {
    IndexlessBudget {
        max_rows: integer_knob(policy, &MAX_ROWS),
        max_visits: integer_knob(policy, &MAX_VISITS),
        max_cost_units: integer_knob(policy, &MAX_COST_UNITS),
    }
}

/// The ceiling premises a cost report carries: `knob:indexless.max_rows=<value>`
/// and its two siblings (MEDULLA-DECISION-ENGINE-R144.7).
pub fn ceiling_premises(ceiling: IndexlessBudget) -> Vec<String> {
    vec![
        format!("knob:{}={}", MAX_ROWS.name, ceiling.max_rows),
        format!("knob:{}={}", MAX_VISITS.name, ceiling.max_visits),
        format!("knob:{}={}", MAX_COST_UNITS.name, ceiling.max_cost_units),
    ]
}

/// The applied budget: the field-wise minimum of the caller budget and the
/// tenant ceiling; no caller budget applies the ceiling. An over-ceiling
/// request is clamped, never refused (MEDULLA-DECISION-ENGINE-R144.7).
pub fn clamp_budget(caller: Option<IndexlessBudget>, ceiling: IndexlessBudget) -> IndexlessBudget {
    match caller {
        None => ceiling,
        Some(asked) => IndexlessBudget {
            max_rows: asked.max_rows.min(ceiling.max_rows),
            max_visits: asked.max_visits.min(ceiling.max_visits),
            max_cost_units: asked.max_cost_units.min(ceiling.max_cost_units),
        },
    }
}

/// The pure budget meter: no clock and no store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetMeter {
    budget: IndexlessBudget,
    rows_visited: u64,
    visits: u64,
    cost_used: u64,
    operation_ids: Vec<String>,
}

impl BudgetMeter {
    /// A meter over the applied budget.
    pub fn new(budget: IndexlessBudget) -> Self {
        Self {
            budget,
            rows_visited: 0,
            visits: 0,
            cost_used: 0,
            operation_ids: Vec::new(),
        }
    }

    /// Admit one operation needing `rows`, `visits` and `cost`. A refusal
    /// leaves the meter unchanged; an admission adds all three.
    pub fn charge(
        &mut self,
        operation_id: &str,
        rows: u64,
        visits: u64,
        cost: u64,
    ) -> Result<(), IndexlessErrorCode> {
        let over = |used: u64, add: u64, max: u64| used.checked_add(add).is_none_or(|t| t > max);
        if over(self.rows_visited, rows, self.budget.max_rows)
            || over(self.visits, visits, self.budget.max_visits)
            || over(self.cost_used, cost, self.budget.max_cost_units)
            || self.operation_ids.len() >= MAX_INDEXLESS_OPS
        {
            return Err(IndexlessErrorCode::IndexlessBudgetExhausted);
        }
        self.rows_visited += rows;
        self.visits += visits;
        self.cost_used += cost;
        self.operation_ids.push(operation_id.to_string());
        Ok(())
    }

    /// The cost report at this moment.
    pub fn report(&self, authorization_revision: u64) -> CostReport {
        CostReport {
            rows_visited: self.rows_visited,
            visits: self.visits,
            cost_used: self.cost_used,
            remaining: IndexlessBudget {
                max_rows: self.budget.max_rows - self.rows_visited,
                max_visits: self.budget.max_visits - self.visits,
                max_cost_units: self.budget.max_cost_units - self.cost_used,
            },
            applied_budget: self.budget,
            operation_ids: BoundedVec::new(self.operation_ids.clone())
                .expect("charge admits at most the op bound"),
            authorization_revision,
            premises: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(max_rows: u64) -> IndexlessBudget {
        IndexlessBudget {
            max_rows,
            max_visits: 1_000,
            max_cost_units: 1_000,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R144.1
    #[test]
    fn operations_and_budget_round_trip() {
        let ops = vec![
            IndexlessOp::RawRead {
                document_id: "d1".into(),
            },
            IndexlessOp::CorpusScan { limit: 10 },
            IndexlessOp::VerifySpan {
                document_id: "d1".into(),
                start_byte: 0,
                end_byte: 3,
                expected_digest: hex::encode(Sha256::digest(b"abc")),
            },
        ];
        let request = IndexlessRequest {
            tenant_id: "t".into(),
            graph: "g".into(),
            ops: BoundedVec::new(ops).unwrap(),
            budget: Some(budget(100)),
            policy: DecisionPolicyRef::Default,
        };
        let text = serde_json::to_string(&request).unwrap();
        assert!(text.contains("\"op\":\"corpus_scan\""));
        assert_eq!(
            serde_json::from_str::<IndexlessRequest>(&text).unwrap(),
            request
        );
        let mut meter = BudgetMeter::new(budget(100));
        meter.charge("op-0", 1, 1, 1).unwrap();
        let result = IndexlessResult {
            rows: vec![serde_json::json!({"document_id": "d1"})],
            cost: meter.report(4),
        };
        let text = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serde_json::from_str::<IndexlessResult>(&text).unwrap(),
            result
        );
        assert!(request.clone().checked().is_ok());

        // An empty op list refuses with the stable code.
        let empty = IndexlessRequest {
            ops: BoundedVec::default(),
            ..request
        };
        let refused = empty.checked().unwrap_err();
        assert_eq!(refused.as_str(), "INDEXLESS_OPS_EMPTY");
        assert_eq!(
            serde_json::to_string(&refused).unwrap(),
            "\"INDEXLESS_OPS_EMPTY\""
        );

        // VerifySpan: boundaries and digest.
        let text = "aé b";
        let digest = hex::encode(Sha256::digest("é".as_bytes()));
        assert!(verify_span(text, 1, 3, &digest).is_ok());
        assert_eq!(
            verify_span(text, 1, 2, &digest),
            Err(IndexlessErrorCode::IndexlessSpanInvalid)
        );
        assert_eq!(
            verify_span(text, 0, 1, &digest),
            Err(IndexlessErrorCode::IndexlessSpanInvalid)
        );
        assert_eq!(
            verify_span(text, 3, 99, &digest),
            Err(IndexlessErrorCode::IndexlessSpanInvalid)
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R144.2
    #[test]
    fn budget_enforcement() {
        let mut meter = BudgetMeter::new(budget(100));
        meter.charge("scan-50", 50, 50, 50).unwrap();
        let before = meter.clone();
        assert_eq!(
            meter.charge("scan-101", 101, 101, 101),
            Err(IndexlessErrorCode::IndexlessBudgetExhausted)
        );
        assert_eq!(
            meter, before,
            "a refused operation leaves the meter unchanged"
        );
        let report = meter.report(1);
        assert_eq!(report.rows_visited, 50);
        assert_eq!(report.remaining.max_rows, 50);
        assert_eq!(report.applied_budget, budget(100));

        let mut fresh = BudgetMeter::new(budget(100));
        assert_eq!(
            fresh.charge("scan-101", 101, 1, 1),
            Err(IndexlessErrorCode::IndexlessBudgetExhausted)
        );
        assert_eq!(fresh.report(1).rows_visited, 0);
        assert_eq!(fresh.report(1).remaining.max_rows, 100);
    }

    // spec: MEDULLA-DECISION-ENGINE-R144.7
    #[test]
    fn clamp_takes_the_field_wise_minimum_and_defaults_to_the_ceiling() {
        let ceiling = IndexlessBudget {
            max_rows: 10_000,
            max_visits: 100_000,
            max_cost_units: 1_000_000,
        };
        assert_eq!(tenant_ceiling(&DecisionPolicy::engine_default()), ceiling);
        assert_eq!(
            unit_costs(&DecisionPolicy::engine_default()),
            IndexlessUnitCosts {
                read: 1,
                scan: 1,
                verify: 2,
            }
        );
        assert_eq!(
            operation_id(2, &IndexlessOp::CorpusScan { limit: 1 }),
            "2:corpus_scan"
        );
        assert_eq!(clamp_budget(None, ceiling), ceiling);
        assert_eq!(
            ceiling_premises(ceiling),
            [
                "knob:indexless.max_rows=10000",
                "knob:indexless.max_visits=100000",
                "knob:indexless.max_cost_units=1000000",
            ]
        );
        let asked = IndexlessBudget {
            max_rows: 20_000,
            max_visits: 50,
            max_cost_units: 2_000_000,
        };
        assert_eq!(
            clamp_budget(Some(asked), ceiling),
            IndexlessBudget {
                max_rows: 10_000,
                max_visits: 50,
                max_cost_units: 1_000_000,
            }
        );
    }
}
