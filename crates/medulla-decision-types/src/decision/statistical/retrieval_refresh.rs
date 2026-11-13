//! The freshness refresh model (MEDULLA-DECISION-ENGINE-R174.9.1): a request
//! to re-read fresh data through a search source, and one result it returns.
//!
//! Types only. They carry the policy references the later behaviour checks;
//! this module refuses nothing but a malformed shape.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

/// Format identity of both types.
pub const FRESHNESS_SCHEMA_VERSION: u16 = 1;
/// Most query texts one refresh may carry.
pub const MAX_REFRESH_QUERIES: usize = 32;

/// Why a refresh was asked for. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RefreshTrigger {
    DriftSignal,
    Staleness,
    JobDemand,
}

/// What a refresh may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RefreshBudget {
    pub max_requests: u32,
    pub max_cost_units: u64,
}

/// A request to refresh one logical graph from a search source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FreshnessRefresh {
    pub schema_version: u16,
    pub logical_graph: String,
    /// `1..=32` query texts.
    pub queries: BoundedVec<String, MAX_REFRESH_QUERIES>,
    pub trigger: RefreshTrigger,
    /// Lower runs first.
    pub priority: u8,
    pub acl_policy_ref: String,
    pub privacy_policy_ref: String,
    pub budget: RefreshBudget,
}

/// One fresh result a refresh stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FreshResult {
    pub schema_version: u16,
    pub url: String,
    /// SHA-256 of the content: 64 lowercase hex characters.
    pub content_digest: String,
    pub source_connection_iri: String,
    /// Epoch milliseconds.
    pub retrieved_at: u64,
    pub query_text: String,
    pub query_digest: String,
    pub refresh_id: String,
}

/// One result row as the refresh holds it, keyed by graph and canonical URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StoredFreshRow {
    pub schema_version: u16,
    pub logical_graph: String,
    pub canonical_url: String,
    pub result: FreshResult,
    /// The digest of the row this one superseded, when it superseded one.
    pub supersedes: Option<String>,
}

/// One region a refresh marked changed in the drift bitmap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegionMark {
    pub shard_id: u32,
    pub node_type: String,
    /// The refresh revision the mark was written at.
    pub revision: u64,
}

/// What one refresh did: the evidence `DecisionLog` writes for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FreshnessReceipt {
    pub schema_version: u16,
    pub refresh_id: String,
    pub logical_graph: String,
    pub source_connection_iri: String,
    pub trigger: RefreshTrigger,
    pub new_count: u32,
    pub unchanged_count: u32,
    pub superseded_count: u32,
    pub requests_spent: u32,
    pub cost_units_spent: u64,
    pub regions: Vec<RegionMark>,
    /// The rows stored (new and superseding), in read order.
    pub stored: Vec<FreshResult>,
    /// Epoch milliseconds the refresh ran at.
    pub completed_at_ms: u64,
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl FreshnessRefresh {
    /// The validating check: the served version and `1..=32` non-empty queries.
    pub fn check(&self) -> Result<(), String> {
        if self.schema_version != FRESHNESS_SCHEMA_VERSION {
            return Err(format!(
                "freshness refresh version {} is not served",
                self.schema_version
            ));
        }
        if self.queries.is_empty() || self.queries.iter().any(|q| q.trim().is_empty()) {
            return Err("a refresh carries 1 to 32 non-empty query texts".to_string());
        }
        Ok(())
    }
}

impl FreshResult {
    /// The validating check: the served version and a well-formed digest.
    pub fn check(&self) -> Result<(), String> {
        if self.schema_version != FRESHNESS_SCHEMA_VERSION {
            return Err(format!(
                "fresh result version {} is not served",
                self.schema_version
            ));
        }
        if !is_sha256_hex(&self.content_digest) {
            return Err("content_digest is 64 lowercase hex characters".to_string());
        }
        Ok(())
    }
}

/// What storing one fresh result did to the held rows. Closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DedupeOutcome {
    /// No row held this URL: the result is stored.
    New,
    /// The URL is held with the same content digest: nothing is written.
    Unchanged,
    /// The URL is held with another digest: the new row supersedes it.
    Superseded {
        /// The digest of the row that is superseded.
        previous_digest: String,
    },
}

impl DedupeOutcome {
    /// Whether the result changes what is held, and so marks its region
    /// changed in the drift bitmap: a new row or a superseding one does, an
    /// unchanged one does not.
    pub fn marks_region(&self) -> bool {
        !matches!(self, Self::Unchanged)
    }
}

/// Which default port a scheme drops during canonicalization.
fn default_port(scheme: &str) -> Option<&'static str> {
    match scheme {
        "http" => Some("80"),
        "https" => Some("443"),
        _ => None,
    }
}

/// The canonical form of a result URL: scheme and host in lower case, the
/// scheme's default port and the fragment removed, and an empty path of an
/// `http` or `https` URL written as `/`. Anything without a scheme and a host
/// is refused.
pub fn canonical_url(raw: &str) -> Result<String, String> {
    let without_fragment = raw.trim().split('#').next().unwrap_or_default();
    let (scheme, rest) = without_fragment
        .split_once("://")
        .ok_or_else(|| format!("`{raw}` has no scheme"))?;
    let scheme = scheme.to_ascii_lowercase();
    let scheme_ok = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_ok {
        return Err(format!("`{raw}` has an invalid scheme"));
    }
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    };
    let (host, port) = match hostport.rfind(':') {
        Some(at) if !hostport[at..].contains(']') => (&hostport[..at], &hostport[at + 1..]),
        _ => (hostport, ""),
    };
    if host.is_empty() {
        return Err(format!("`{raw}` has no host"));
    }
    if !port.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("`{raw}` has an invalid port"));
    }
    let keep_port = !port.is_empty() && default_port(&scheme) != Some(port);
    let mut out = format!("{scheme}://");
    if let Some(user) = userinfo {
        out.push_str(user);
        out.push('@');
    }
    out.push_str(&host.to_ascii_lowercase());
    if keep_port {
        out.push(':');
        out.push_str(port);
    }
    if tail.is_empty() && default_port(&scheme).is_some() {
        out.push('/');
    } else {
        out.push_str(tail);
    }
    Ok(out)
}

/// Classify one result against the digest held for its canonical URL.
pub fn classify_result(held_digest: Option<&str>, content_digest: &str) -> DedupeOutcome {
    match held_digest {
        None => DedupeOutcome::New,
        Some(held) if held == content_digest => DedupeOutcome::Unchanged,
        Some(held) => DedupeOutcome::Superseded {
            previous_digest: held.to_string(),
        },
    }
}

/// Classify a batch of results against the held rows (canonical URL to
/// digest). The held map is updated as the batch is read, so a URL that
/// repeats inside one batch is stored once. Results come back in input order
/// with their canonical URL. A result with a malformed digest or URL refuses
/// the whole batch before anything is stored.
pub fn dedupe_batch(
    held: &mut std::collections::BTreeMap<String, String>,
    results: &[FreshResult],
) -> Result<Vec<(String, DedupeOutcome)>, String> {
    let mut prepared = Vec::with_capacity(results.len());
    for result in results {
        result.check()?;
        prepared.push((canonical_url(&result.url)?, &result.content_digest));
    }
    let mut outcomes = Vec::with_capacity(prepared.len());
    for (url, digest) in prepared {
        let outcome = classify_result(held.get(&url).map(String::as_str), digest);
        if outcome.marks_region() {
            held.insert(url.clone(), digest.clone());
        }
        outcomes.push((url, outcome));
    }
    Ok(outcomes)
}

/// Which budget a refused request ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetRefusal {
    /// The per-window request allowance is spent.
    Requests,
    /// The declared cost units are spent.
    Cost,
}

/// One tenant's spending against a refresh budget: a token bucket of
/// `max_requests` tokens refilled evenly over the window (the
/// `freshness.window` knob) and a running total of cost units against
/// `max_cost_units`. A refused request changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshSpend {
    capacity: u32,
    window_ms: u64,
    max_cost_units: u64,
    tokens_milli: u64,
    refilled_at_ms: u64,
    cost_spent: u64,
}

const MILLI: u64 = 1_000;

impl RefreshSpend {
    /// A full bucket at `now_ms` for `budget`, over `window_ms` (at least 1).
    pub fn new(budget: &RefreshBudget, window_ms: u64, now_ms: u64) -> Self {
        Self {
            capacity: budget.max_requests,
            window_ms: window_ms.max(1),
            max_cost_units: budget.max_cost_units,
            tokens_milli: u64::from(budget.max_requests) * MILLI,
            refilled_at_ms: now_ms,
            cost_spent: 0,
        }
    }

    fn refilled(&self, now_ms: u64) -> u64 {
        let elapsed = u128::from(now_ms.saturating_sub(self.refilled_at_ms));
        let gained =
            elapsed * u128::from(self.capacity) * u128::from(MILLI) / u128::from(self.window_ms);
        let full = u128::from(self.capacity) * u128::from(MILLI);
        (u128::from(self.tokens_milli) + gained).min(full) as u64
    }

    /// Spend one request of `cost_units` at `now_ms`.
    pub fn charge(&mut self, cost_units: u64, now_ms: u64) -> Result<(), BudgetRefusal> {
        let tokens = self.refilled(now_ms);
        if tokens < MILLI {
            return Err(BudgetRefusal::Requests);
        }
        let spent = self
            .cost_spent
            .checked_add(cost_units)
            .filter(|total| *total <= self.max_cost_units)
            .ok_or(BudgetRefusal::Cost)?;
        self.tokens_milli = tokens - MILLI;
        self.refilled_at_ms = self.refilled_at_ms.max(now_ms);
        self.cost_spent = spent;
        Ok(())
    }

    /// Whether this spending was opened for `budget`: a changed budget starts
    /// a new bucket.
    pub fn same_budget(&self, budget: &RefreshBudget) -> bool {
        self.capacity == budget.max_requests && self.max_cost_units == budget.max_cost_units
    }

    /// Cost units spent so far.
    pub fn cost_spent(&self) -> u64 {
        self.cost_spent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refresh_json(queries: usize, trigger: &str) -> String {
        let queries = vec!["\"q\""; queries].join(",");
        format!(
            r#"{{"schema_version":1,"logical_graph":"g","queries":[{queries}],
            "trigger":"{trigger}","priority":3,"acl_policy_ref":"acl","privacy_policy_ref":"p",
            "budget":{{"max_requests":4,"max_cost_units":9}}}}"#
        )
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.9.1
    #[test]
    fn refresh_round_trips() {
        for trigger in ["drift_signal", "staleness", "job_demand"] {
            let refresh: FreshnessRefresh =
                serde_json::from_str(&refresh_json(32, trigger)).unwrap();
            refresh.check().unwrap();
            let again = serde_json::to_string(&refresh).unwrap();
            assert_eq!(
                serde_json::from_str::<FreshnessRefresh>(&again).unwrap(),
                refresh
            );
        }
        assert!(serde_json::from_str::<FreshnessRefresh>(&refresh_json(33, "staleness")).is_err());
        assert!(serde_json::from_str::<FreshnessRefresh>(&refresh_json(1, "whim")).is_err());
        let empty: FreshnessRefresh = serde_json::from_str(&refresh_json(0, "staleness")).unwrap();
        assert!(empty.check().is_err());

        let result = FreshResult {
            schema_version: FRESHNESS_SCHEMA_VERSION,
            url: "https://example.org/a".into(),
            content_digest: "a".repeat(64),
            source_connection_iri: "urn:conn".into(),
            retrieved_at: 7,
            query_text: "q".into(),
            query_digest: "b".repeat(64),
            refresh_id: "r1".into(),
        };
        result.check().unwrap();
        let again = serde_json::to_string(&result).unwrap();
        assert_eq!(serde_json::from_str::<FreshResult>(&again).unwrap(), result);
        let upper = FreshResult {
            content_digest: "A".repeat(64),
            ..result.clone()
        };
        assert!(upper.check().is_err());
        let short = FreshResult {
            content_digest: "a".repeat(63),
            ..result
        };
        assert!(short.check().is_err());
    }

    fn fresh(url: &str, digest: char) -> FreshResult {
        FreshResult {
            schema_version: FRESHNESS_SCHEMA_VERSION,
            url: url.into(),
            content_digest: digest.to_string().repeat(64),
            source_connection_iri: "urn:conn".into(),
            retrieved_at: 1,
            query_text: "q".into(),
            query_digest: "b".repeat(64),
            refresh_id: "r".into(),
        }
    }

    /// Verifies MEDULLA-DECISION-ENGINE-R174.9.2: Behavior: canonical URLs and dedupe outcomes.
    // spec: MEDULLA-DECISION-ENGINE-R174.9.2
    #[test]
    fn canonical_urls_and_dedupe_outcomes() {
        assert_eq!(
            canonical_url("HTTPS://Example.ORG:443/A/b?q=1#frag").unwrap(),
            "https://example.org/A/b?q=1"
        );
        assert_eq!(
            canonical_url("http://example.org:80").unwrap(),
            "http://example.org/"
        );
        assert_eq!(
            canonical_url("http://Example.org:8080/x").unwrap(),
            "http://example.org:8080/x"
        );
        assert_eq!(
            canonical_url("https://u@Host.example/p#a").unwrap(),
            "https://u@host.example/p"
        );
        for bad in [
            "example.org/x",
            "://x",
            "https:///x",
            "ht tp://a/",
            "http://a:b/",
        ] {
            assert!(canonical_url(bad).is_err(), "{bad}");
        }

        assert_eq!(classify_result(None, "a"), DedupeOutcome::New);
        assert_eq!(classify_result(Some("a"), "a"), DedupeOutcome::Unchanged);
        assert_eq!(
            classify_result(Some("a"), "b"),
            DedupeOutcome::Superseded {
                previous_digest: "a".into()
            }
        );

        let mut held = std::collections::BTreeMap::new();
        let first = dedupe_batch(
            &mut held,
            &[
                fresh("https://Example.org/a#x", 'a'),
                fresh("https://example.org:443/a", 'a'),
                fresh("https://example.org/b", 'c'),
            ],
        )
        .unwrap();
        assert_eq!(first[0].1, DedupeOutcome::New);
        assert_eq!(first[1].1, DedupeOutcome::Unchanged);
        assert_eq!(first[2].1, DedupeOutcome::New);
        let second = dedupe_batch(
            &mut held,
            &[
                fresh("https://example.org/a", 'a'),
                fresh("https://example.org/b", 'd'),
            ],
        )
        .unwrap();
        assert_eq!(second[0].1, DedupeOutcome::Unchanged);
        assert_eq!(
            second[1].1,
            DedupeOutcome::Superseded {
                previous_digest: "c".repeat(64)
            }
        );
        let before = held.clone();
        let mut bad = fresh("https://example.org/z", 'e');
        bad.content_digest = "E".repeat(64);
        assert!(dedupe_batch(&mut held, &[fresh("https://example.org/y", 'f'), bad]).is_err());
        assert_eq!(held, before, "a refused batch stores nothing");
    }

    /// Verifies MEDULLA-DECISION-ENGINE-R174.9.2: Behavior: the region mark and both budgets.
    // spec: MEDULLA-DECISION-ENGINE-R174.9.2
    #[test]
    fn region_mark_and_both_budgets() {
        assert!(DedupeOutcome::New.marks_region());
        assert!(DedupeOutcome::Superseded {
            previous_digest: "a".into()
        }
        .marks_region());
        assert!(!DedupeOutcome::Unchanged.marks_region());

        let budget = RefreshBudget {
            max_requests: 2,
            max_cost_units: 10,
        };
        let mut spend = RefreshSpend::new(&budget, 1_000, 0);
        spend.charge(4, 0).unwrap();
        spend.charge(4, 0).unwrap();
        assert_eq!(spend.charge(1, 0), Err(BudgetRefusal::Requests));
        assert_eq!(spend.cost_spent(), 8);
        // Half a window refills one request; the cost budget then refuses.
        assert_eq!(spend.charge(3, 500), Err(BudgetRefusal::Cost));
        assert_eq!(spend.cost_spent(), 8, "a refused request changes nothing");
        spend.charge(2, 500).unwrap();
        assert_eq!(spend.cost_spent(), 10);
        // Refill never exceeds the capacity.
        let mut idle = RefreshSpend::new(&budget, 1_000, 0);
        idle.charge(0, 0).unwrap();
        idle.charge(0, 0).unwrap();
        idle.charge(0, 1_000_000).unwrap();
        idle.charge(0, 1_000_000).unwrap();
        assert_eq!(idle.charge(0, 1_000_000), Err(BudgetRefusal::Requests));
        let none = RefreshBudget {
            max_requests: 0,
            max_cost_units: 1,
        };
        assert_eq!(
            RefreshSpend::new(&none, 1_000, 0).charge(0, 9_999),
            Err(BudgetRefusal::Requests)
        );
    }
}
