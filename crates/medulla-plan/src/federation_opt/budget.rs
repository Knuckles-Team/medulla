//! The per-query network budget and its typed refusals (design §4.5).

use std::time::{Duration, Instant};

use crate::budget::FederationBudget;

/// Error-code prefix of a budget refusal: `FEDERATION_BUDGET_EXCEEDED:<dimension>: …`.
pub const BUDGET_EXCEEDED: &str = "FEDERATION_BUDGET_EXCEEDED";
/// Error code of a key-lookup-only source asked for rows without keys.
pub const REQUIRES_KEYS: &str = "FEDERATION_SOURCE_REQUIRES_KEYS";
/// Error code of a source whose single request hit its own row cap, so completeness cannot
/// be proven.
pub const RESULT_INCOMPLETE: &str = "FEDERATION_RESULT_INCOMPLETE";

/// The typed refusal for one exhausted dimension.
pub(crate) fn refusal(dimension: &str, limit: impl std::fmt::Display) -> String {
    format!("{BUDGET_EXCEEDED}:{dimension}: the federated query exceeded its limit of {limit}")
}

/// What one query has spent so far.
#[derive(Debug)]
pub(crate) struct BudgetMeter {
    budget: FederationBudget,
    requests: u32,
    rows: usize,
    started: Instant,
}

impl BudgetMeter {
    pub(crate) fn new(budget: FederationBudget) -> Self {
        Self {
            budget,
            requests: 0,
            rows: 0,
            started: Instant::now(),
        }
    }

    pub(crate) fn budget(&self) -> FederationBudget {
        self.budget
    }

    pub(crate) fn narrow(&mut self, requested: FederationBudget) {
        self.budget = self.budget.narrowed(requested);
    }

    /// Account one round trip before it is sent.
    pub(crate) fn charge_request(&mut self) -> Result<(), String> {
        if self.requests >= self.budget.max_requests {
            return Err(refusal("requests", self.budget.max_requests));
        }
        self.check_wall()?;
        self.requests += 1;
        Ok(())
    }

    /// The wall budget, checked when a request is charged.
    pub(crate) fn check_wall(&self) -> Result<(), String> {
        if self.started.elapsed().as_millis() > u128::from(self.budget.max_wall_ms) {
            return Err(refusal("wall_ms", self.budget.max_wall_ms));
        }
        Ok(())
    }

    /// The instant the wall budget runs out: no wait for a source may outlast it. A budget
    /// too large to represent as an instant is a year away.
    pub(crate) fn deadline(&self) -> Instant {
        self.started
            .checked_add(Duration::from_millis(self.budget.max_wall_ms))
            .unwrap_or_else(|| self.started + Duration::from_secs(365 * 24 * 60 * 60))
    }

    /// The refusal of a request that could not start before [`Self::deadline`].
    pub(crate) fn wall_refusal(&self) -> String {
        refusal("wall_ms", self.budget.max_wall_ms)
    }

    /// Account rows received.
    pub(crate) fn charge_rows(&mut self, rows: usize) -> Result<(), String> {
        self.rows = self.rows.saturating_add(rows);
        if self.rows > self.budget.max_rows {
            return Err(refusal("rows", self.budget.max_rows));
        }
        Ok(())
    }
}

#[cfg(test)]
mod exhausted_budget_refusal {
    //! MEDULLA-FEDERATED-QUERY-R073.1 acceptance: "Budget ... cases refuse with typed
    //! errors." The per-query budget (FQR-04) is enforced uniformly across every
    //! `ForeignSourceSpec` kind — including the four new operation-bound kinds
    //! (API/MCP/A2A/GraphQL) added for R073 — so this proves the mechanism itself
    //! refuses typed, independent of which source kind would have used it.
    use super::{BudgetMeter, FederationBudget, BUDGET_EXCEEDED};

    // spec: MEDULLA-FEDERATED-QUERY-R073.1
    #[test]
    fn a_zero_request_budget_refuses_the_first_round_trip_with_a_typed_error() {
        let exhausted = FederationBudget {
            max_requests: 0,
            ..FederationBudget::default()
        };
        let mut meter = BudgetMeter::new(exhausted);
        let err = meter
            .charge_request()
            .expect_err("a zero-request budget must refuse the first round trip");
        assert!(
            err.starts_with(BUDGET_EXCEEDED),
            "budget refusal must carry the typed FEDERATION_BUDGET_EXCEEDED code: {err}"
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R073.1
    #[test]
    fn a_zero_row_budget_refuses_once_any_row_arrives_with_a_typed_error() {
        let exhausted = FederationBudget {
            max_rows: 0,
            ..FederationBudget::default()
        };
        let mut meter = BudgetMeter::new(exhausted);
        let err = meter
            .charge_rows(1)
            .expect_err("a zero-row budget must refuse the first row");
        assert!(err.starts_with(BUDGET_EXCEEDED), "unexpected error: {err}");
    }
}
