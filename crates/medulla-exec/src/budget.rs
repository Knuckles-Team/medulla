//! Execution budgets (UQL-09): the resource bounds every plan runs under.
//!
//! A plan that would exceed a budget fails with a typed `UQL_BUDGET_EXCEEDED` error that
//! names the budget and the remedy — never a silent truncation, never an unbounded walk.
//! Defaults are deliberately generous for interactive use; a server binds its own via
//! [`crate::exec::PlanCtx::with_budget`].

/// The error-code prefix of every budget refusal.
pub const BUDGET_EXCEEDED: &str = "UQL_BUDGET_EXCEEDED";

/// Resource bounds for one plan execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Most rows the plan may return (checked on the final RowSet).
    pub max_result_rows: usize,
    /// Most nodes one `TRAVERSE` may visit before it is refused (fan-out bound).
    pub max_traversal_visits: usize,
    /// Most distance evaluations one `MOTIF`/`DISCORD` stage's anytime matrix-profile
    /// search may spend (EH-529, ANALYTICS-HARVEST AH-09) before it returns its
    /// best-so-far with `approximate = true` — never a refusal, unlike the two bounds
    /// above: an anytime algorithm degrades gracefully instead of failing the query.
    pub max_series_work: u64,
}

impl Budget {
    /// Default result-row bound.
    pub const DEFAULT_MAX_RESULT_ROWS: usize = 100_000;
    /// Default per-traversal visited-node bound.
    pub const DEFAULT_MAX_TRAVERSAL_VISITS: usize = 1_000_000;
    /// Default `MOTIF`/`DISCORD` work bound (distance evaluations) — the complete
    /// profile of an 8,000-point series (`8000² / 2`), a generous interactive default.
    pub const DEFAULT_MAX_SERIES_WORK: u64 = 32_000_000;

    /// Refuse a final result larger than the bound.
    pub fn check_result(&self, rows: usize) -> Result<(), String> {
        if rows <= self.max_result_rows {
            return Ok(());
        }
        Err(format!(
            "{BUDGET_EXCEEDED}: the result has {rows} rows, above max_result_rows = {}; \
             add `LIMIT <k>` or narrow the query",
            self.max_result_rows
        ))
    }

    /// Refuse a traversal that has visited more nodes than the bound.
    pub fn check_traversal(&self, visited: usize) -> Result<(), String> {
        if visited <= self.max_traversal_visits {
            return Ok(());
        }
        Err(format!(
            "{BUDGET_EXCEEDED}: a TRAVERSE visited more than max_traversal_visits = {} \
             nodes; lower its hop range, filter the seed set, or add edge predicates",
            self.max_traversal_visits
        ))
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_result_rows: Self::DEFAULT_MAX_RESULT_ROWS,
            max_traversal_visits: Self::DEFAULT_MAX_TRAVERSAL_VISITS,
            max_series_work: Self::DEFAULT_MAX_SERIES_WORK,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R070
    #[test]
    fn budgets_refuse_with_a_typed_code() {
        let b = Budget {
            max_result_rows: 2,
            max_traversal_visits: 3,
            max_series_work: 4,
        };
        assert!(b.check_result(2).is_ok());
        assert!(b.check_result(3).unwrap_err().starts_with(BUDGET_EXCEEDED));
        assert!(b
            .check_traversal(4)
            .unwrap_err()
            .contains("max_traversal_visits"));
    }
}

/// Bounds one query may spend across every remote fragment. A request may narrow it
/// ([`Self::narrowed`]), never widen it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FederationBudget {
    /// Remote requests (round trips) across the query.
    pub max_requests: u32,
    /// Rows received from remote sources across the query.
    pub max_rows: usize,
    /// Local ids one bind join may ship as keys.
    pub max_bind_keys: usize,
    /// Wall-clock milliseconds spent in remote fragments.
    pub max_wall_ms: u64,
}

impl Default for FederationBudget {
    fn default() -> Self {
        Self {
            max_requests: 256,
            max_rows: 250_000,
            max_bind_keys: 10_000,
            max_wall_ms: 60_000,
        }
    }
}

impl FederationBudget {
    /// The server budget: the defaults, each overridable by
    /// `MEDULLA_FEDERATION_MAX_{REQUESTS,ROWS,BIND_KEYS,WALL_MS}`.
    pub fn from_env() -> Self {
        let d = Self::default();
        Self {
            max_requests: medulla_types::runtime_limit::positive_from_env(
                "MEDULLA_FEDERATION_MAX_REQUESTS",
                d.max_requests,
            ),
            max_rows: medulla_types::runtime_limit::positive_from_env(
                "MEDULLA_FEDERATION_MAX_ROWS",
                d.max_rows,
            ),
            max_bind_keys: medulla_types::runtime_limit::positive_from_env(
                "MEDULLA_FEDERATION_MAX_BIND_KEYS",
                d.max_bind_keys,
            ),
            max_wall_ms: medulla_types::runtime_limit::positive_from_env(
                "MEDULLA_FEDERATION_MAX_WALL_MS",
                d.max_wall_ms,
            ),
        }
    }

    /// The field-wise minimum of `self` and `other` — a caller can only tighten.
    pub fn narrowed(self, other: Self) -> Self {
        Self {
            max_requests: self.max_requests.min(other.max_requests),
            max_rows: self.max_rows.min(other.max_rows),
            max_bind_keys: self.max_bind_keys.min(other.max_bind_keys),
            max_wall_ms: self.max_wall_ms.min(other.max_wall_ms),
        }
    }
}
