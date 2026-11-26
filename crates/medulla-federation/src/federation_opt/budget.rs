//! The per-query network budget and its typed refusals (design §4.5).

use std::time::{Duration, Instant};

use medulla_exec::budget::FederationBudget;

/// Error-code prefix of a budget refusal: `FEDERATION_BUDGET_EXCEEDED:<dimension>: …`.
pub const BUDGET_EXCEEDED: &str = "FEDERATION_BUDGET_EXCEEDED";
/// Error code of a key-lookup-only source asked for rows without keys.
pub const REQUIRES_KEYS: &str = "FEDERATION_SOURCE_REQUIRES_KEYS";
/// Error code of a source whose single request hit its own row cap, so completeness cannot
/// be proven.
pub const RESULT_INCOMPLETE: &str = "FEDERATION_RESULT_INCOMPLETE";
/// Error code of an argument a web metasearch tool does not declare
/// (MEDULLA-UNIFIED-DATA-PLANE-R038.7.4).
pub const UNSUPPORTED_SEARCH_ARGUMENT: &str = "UNSUPPORTED_SEARCH_ARGUMENT";
/// Error code of a vector query from another embedding space than the source's
/// (MEDULLA-UNIFIED-DATA-PLANE-R038.5.4).
pub const EMBEDDING_SPACE_MISMATCH: &str = "EMBEDDING_SPACE_MISMATCH";
/// Error code of a remote result row that carries no score under a declared score kind
/// (MEDULLA-UNIFIED-DATA-PLANE-R038.5.4).
pub const REMOTE_SCORE_MISSING: &str = "REMOTE_SCORE_MISSING";
/// Error code of a query with two or more fragments of a source that cannot give them one
/// snapshot (MEDULLA-UNIFIED-DATA-PLANE-R038.3.4).
pub const SNAPSHOT_UNAVAILABLE: &str = "SNAPSHOT_UNAVAILABLE";
/// Error code of a declared replica whose replay lag is over `federation.replica_lag_bound_ms`.
pub const REPLICA_LAG_EXCEEDED: &str = "REPLICA_LAG_EXCEEDED";
/// Error code of a read plan that holds a statement other than a `SELECT`.
pub const SOURCE_WRITE_FORBIDDEN: &str = "SOURCE_WRITE_FORBIDDEN";

/// Error code of one source that spent its own quota: rows, request time or concurrent
/// requests (MEDULLA-UNIFIED-DATA-PLANE-R031.2). It names that source and no other.
pub const SOURCE_QUOTA_EXCEEDED: &str = "SOURCE_QUOTA_EXCEEDED";
/// Error code of a source that failed to answer (MEDULLA-UNIFIED-DATA-PLANE-R031.2): its error is
/// the cause and no other source is touched.
pub const SOURCE_UNAVAILABLE: &str = "SOURCE_UNAVAILABLE";

/// The typed refusal of one source over one quota dimension.
pub(crate) fn quota_refusal(label: &str, dimension: &str, limit: impl std::fmt::Display) -> String {
    format!("{SOURCE_QUOTA_EXCEEDED}:{dimension}: source {label} exceeded its quota of {limit}")
}

/// A source's failure as the typed `SOURCE_UNAVAILABLE`, unless it already carries a code
/// (`CODE: detail`), which stays as it is.
pub(crate) fn unavailable(label: &str, error: String) -> String {
    let coded = error.split_once(':').is_some_and(|(code, _)| {
        code.len() > 2
            && code
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    });
    if coded {
        error
    } else {
        format!("{SOURCE_UNAVAILABLE}: source {label}: {error}")
    }
}

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
    scan_bytes: u64,
    started: Instant,
}

impl BudgetMeter {
    pub(crate) fn new(budget: FederationBudget) -> Self {
        Self {
            budget,
            requests: 0,
            rows: 0,
            scan_bytes: 0,
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

    /// The bytes a raw-access source may still scan for this query
    /// (MEDULLA-FEDERATED-QUERY-R729).
    pub(crate) fn remaining_scan_bytes(&self) -> u64 {
        self.budget.max_scan_bytes.saturating_sub(self.scan_bytes)
    }

    /// Account the bytes a raw-access source reports it scanned.
    pub(crate) fn charge_scan_bytes(&mut self, bytes: u64) -> Result<(), String> {
        self.scan_bytes = self.scan_bytes.saturating_add(bytes);
        if self.scan_bytes > self.budget.max_scan_bytes {
            return Err(refusal("scan_bytes", self.budget.max_scan_bytes));
        }
        Ok(())
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

#[cfg(test)]
mod source_fault_codes {
    use super::{quota_refusal, unavailable, SOURCE_QUOTA_EXCEEDED, SOURCE_UNAVAILABLE};

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R031.2
    #[test]
    fn an_uncoded_source_failure_is_unavailable_and_a_coded_one_keeps_its_code() {
        let wrapped = unavailable("http-json#ab12", "connection refused".into());
        assert!(wrapped.starts_with(SOURCE_UNAVAILABLE), "{wrapped}");
        assert!(wrapped.contains("http-json#ab12"), "{wrapped}");
        let coded = unavailable("mcp#cd34", "SOURCE_UNREACHABLE: no port".into());
        assert_eq!(coded, "SOURCE_UNREACHABLE: no port");
        let quota = quota_refusal("sql#ef56", "max_rows", 10);
        assert!(quota.starts_with(SOURCE_QUOTA_EXCEEDED), "{quota}");
        assert!(quota.contains("sql#ef56"), "{quota}");
    }
}
