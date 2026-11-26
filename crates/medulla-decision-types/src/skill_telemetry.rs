//! Skill telemetry for the Learn stage (MEDULLA-DECISION-ENGINE-R713.1, R713.4).
//!
//! One [`SkillInvocation`] records one use of a skill: who chose it, how long it ran, how it
//! ended and what it cost. The type holds no free text: every field is an id, a number, an enum
//! or a closed error code of at most 64 bytes. [`SkillMeasures`] sums the invocations of one
//! skill over a window into exactly twelve numbers. [`SkillTelemetryOp`] is the body of
//! `Method::SkillTelemetry`.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;
use medulla_types::refusal_code::RefusalCode;

/// The graph class of an invocation node (vocabulary R817).
pub const SKILL_INVOCATION_CLASS: &str = "SkillInvocation";
/// The stored property the read index files an invocation under by skill revision.
pub const INDEX_KEY_SKILL: &str = "skill_ref";
/// The stored property the read index files an invocation under by principal pseudonym.
pub const INDEX_KEY_PRINCIPAL: &str = "principal_pseudonym";
/// The stored property the read index files an invocation under by day of `started_ms`.
pub const INDEX_KEY_DAY: &str = "started_day";
/// Milliseconds in one day bucket of the read index.
pub const INDEX_DAY_MS: u64 = 86_400_000;
/// The most day buckets a windowed read looks up; a longer window reads by skill or principal alone.
pub const INDEX_DAYS_MAX: u64 = 62;

/// The day bucket `ms` is filed under in the read index.
pub fn started_day(ms: u64) -> u64 {
    ms / INDEX_DAY_MS
}

/// The day buckets a window covers, or `None` when the window is empty or longer than
/// [`INDEX_DAYS_MAX`] days.
pub fn window_days(window: Window) -> Option<std::ops::RangeInclusive<u64>> {
    if window.end_ms <= window.start_ms {
        return None;
    }
    let (first, last) = (started_day(window.start_ms), started_day(window.end_ms - 1));
    (last - first < INDEX_DAYS_MAX).then_some(first..=last)
}
/// Most events one `Ingest` carries.
pub const INGEST_BATCH_MAX: usize = 500;
/// Most bytes of an error code.
pub const ERROR_CODE_MAX_BYTES: usize = 64;
/// Most traces one `TracesByEval` page returns.
pub const TRACES_PAGE_MAX: usize = 200;
/// The scope that lets a service ingest invocations.
pub const SCOPE_WRITE: &str = "telemetry:write";
/// The scope that lets a caller read measures, ranks, peers and traces.
pub const SCOPE_READ: &str = "telemetry:read";
/// The length of the retention windows of `retained_users_ppm`, in milliseconds (14 days).
pub const RETENTION_WINDOW_MS: u64 = 14 * 24 * 60 * 60 * 1000;

/// `TELEMETRY_FIELD_UNKNOWN`: an event holds a field outside the type.
pub const TELEMETRY_FIELD_UNKNOWN: RefusalCode = RefusalCode::new("TELEMETRY_FIELD_UNKNOWN");
/// `TELEMETRY_SESSION_UNKNOWN`: a person's span names a run that is not one of its own
/// sessions; an absent run and another principal's run answer the same.
pub const TELEMETRY_SESSION_UNKNOWN: RefusalCode = RefusalCode::new("TELEMETRY_SESSION_UNKNOWN");
/// `TELEMETRY_SKILL_UNKNOWN`: an event names a skill revision the graph does not hold.
pub const TELEMETRY_SKILL_UNKNOWN: RefusalCode = RefusalCode::new("TELEMETRY_SKILL_UNKNOWN");
/// `TELEMETRY_BATCH_TOO_LARGE`: more than 500 events in one batch.
pub const TELEMETRY_BATCH_TOO_LARGE: RefusalCode = RefusalCode::new("TELEMETRY_BATCH_TOO_LARGE");
/// `TELEMETRY_CLOCK_INVALID`: an event ends before it starts.
pub const TELEMETRY_CLOCK_INVALID: RefusalCode = RefusalCode::new("TELEMETRY_CLOCK_INVALID");
/// `TELEMETRY_SCOPE_REQUIRED`: the caller lacks the scope of the op.
pub const TELEMETRY_SCOPE_REQUIRED: RefusalCode = RefusalCode::new("TELEMETRY_SCOPE_REQUIRED");
/// `TELEMETRY_CODE_INVALID`: an error code outside the closed form.
pub const TELEMETRY_CODE_INVALID: RefusalCode = RefusalCode::new("TELEMETRY_CODE_INVALID");
/// `PEER_COHORT_TOO_SMALL`: the cohort holds fewer principals than `skill.peers.min_peers`.
pub const PEER_COHORT_TOO_SMALL: RefusalCode = RefusalCode::new("PEER_COHORT_TOO_SMALL");

/// The `skill_telemetry` table of registered refusal codes.
pub const REFUSALS: &[RefusalCode] = &[
    TELEMETRY_FIELD_UNKNOWN,
    TELEMETRY_SKILL_UNKNOWN,
    TELEMETRY_BATCH_TOO_LARGE,
    TELEMETRY_CLOCK_INVALID,
    TELEMETRY_SCOPE_REQUIRED,
    TELEMETRY_CODE_INVALID,
    PEER_COHORT_TOO_SMALL,
];

/// Who chose the skill for the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChosenBy {
    /// The agent picked the skill.
    Agent,
    /// The user asked for the skill.
    User,
}

impl ChosenBy {
    /// The stored text.
    pub fn stored(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::User => "user",
        }
    }
}

/// How an invocation ended.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InvocationOutcome {
    /// The skill ran to its end.
    Ok,
    /// The skill ended with a closed error code.
    Error {
        /// The code: upper-case ASCII letters, digits and underscores, at most 64 bytes.
        code: String,
    },
}

impl InvocationOutcome {
    /// Whether the code is inside the closed form.
    pub fn code_valid(&self) -> bool {
        match self {
            Self::Ok => true,
            Self::Error { code } => {
                !code.is_empty()
                    && code.len() <= ERROR_CODE_MAX_BYTES
                    && code
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            }
        }
    }
}

/// One use of one skill. No field holds free text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillInvocation {
    /// The invocation id; a repeat of one id is stored once.
    pub invocation_id: String,
    /// The tenant: the engine overwrites it with the verified tenant.
    pub tenant_id: String,
    /// The skill revision: `skill:<name>@<revision>`.
    pub skill_ref: String,
    /// The version of the skill that ran.
    pub skill_version: String,
    /// Who chose the skill.
    pub chosen_by: ChosenBy,
    /// `sha256(tenant_id, principal, salt)` as hex: see [`principal_pseudonym`].
    pub principal_pseudonym: String,
    /// The run the skill ran in.
    pub run_id: String,
    /// When it started, Unix milliseconds.
    pub started_ms: u64,
    /// When it ended, Unix milliseconds.
    pub ended_ms: u64,
    /// How it ended.
    pub outcome: InvocationOutcome,
    /// What the run cost beyond the baseline, in micro USD; absent when unknown.
    #[serde(default)]
    pub cost_delta_micro_usd: Option<i64>,
    /// Input tokens.
    pub tokens_in: u64,
    /// Output tokens.
    pub tokens_out: u64,
}

/// The pseudonym of one principal: the hex SHA-256 over the tenant, the principal and the salt of
/// knob `skill.pseudonym_salt`. The same principal always gets the same pseudonym within a tenant,
/// and no principal can be read back from it.
pub fn principal_pseudonym(tenant_id: &str, principal: &str, salt: u64) -> String {
    let salt = salt.to_string();
    Digest256::framed(
        b"medulla.skill_telemetry.pseudonym.v1",
        &[tenant_id.as_bytes(), principal.as_bytes(), salt.as_bytes()],
    )
    .expect("a pseudonym frames three short fields")
    .to_hex()
}

/// The twelve measures of one skill over one window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillMeasures {
    /// How many invocations the window holds.
    pub invocations: u64,
    /// How many distinct pseudonyms used the skill.
    pub distinct_users: u64,
    /// The share of users of the first 14 days who are active again in the last 14, in ppm.
    pub retained_users_ppm: u64,
    /// The share of invocations the agent chose, in ppm.
    pub agent_chosen_ppm: u64,
    /// The share of invocations the user chose, in ppm.
    pub user_chosen_ppm: u64,
    /// The share of invocations that ended in an error, in ppm.
    pub error_rate_ppm: u64,
    /// The median latency; absent when the window holds no invocation.
    #[serde(default)]
    pub latency_p50_ms: Option<u64>,
    /// The 95th percentile latency; absent when the window holds no invocation.
    #[serde(default)]
    pub latency_p95_ms: Option<u64>,
    /// The mean cost delta over invocations that report one; absent when none does.
    #[serde(default)]
    pub cost_mean_micro_usd: Option<i64>,
    /// The 95th percentile cost delta; absent when no invocation reports one.
    #[serde(default)]
    pub cost_p95_micro_usd: Option<i64>,
    /// The pass rate of the skill's latest evaluation suite, in ppm.
    pub eval_pass_rate_ppm: u64,
    /// The cases that suite ran.
    pub eval_runs: u64,
}

/// A half-open time window `[start_ms, end_ms)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Window {
    /// The first millisecond the window holds.
    pub start_ms: u64,
    /// The first millisecond after the window.
    pub end_ms: u64,
}

impl Window {
    /// Whether `ms` lies inside the window.
    pub fn contains(self, ms: u64) -> bool {
        (self.start_ms..self.end_ms).contains(&ms)
    }
}

/// What a `TracesByEval` read filters on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EvalOutcome {
    /// The run's score passed.
    Pass,
    /// The run's score failed.
    Fail,
    /// The run has no score.
    Unscored,
}

/// The body of `Method::SkillTelemetry`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "op", deny_unknown_fields)]
pub enum SkillTelemetryOp {
    /// Store a batch of invocations. Scope `telemetry:write`. Each event is a JSON object of
    /// [`SkillInvocation`]; a field outside the type refuses the whole batch.
    Ingest {
        /// The graph that holds the skills, the run traces and the invocations.
        graph: String,
        /// The events.
        events: Vec<serde_json::Value>,
    },
    /// The twelve measures of one skill. Scope `telemetry:read`.
    Measures {
        /// The graph.
        graph: String,
        /// The skill revision.
        skill_ref: String,
        /// The window.
        window: Window,
    },
    /// Skills ordered by quality. Scope `telemetry:read`.
    Rank {
        /// The graph.
        graph: String,
        /// Only these skills; every skill with an invocation when empty.
        #[serde(default)]
        skill_refs: Vec<String>,
        /// The window.
        window: Window,
        /// Most skills returned.
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Skills the caller's cohort used and the caller did not. The engine computes the cohort
    /// itself: every principal of the caller's tenant with telemetry of the same skill class in
    /// the window. Scope `telemetry:read`.
    Peers {
        /// The graph.
        graph: String,
        /// The window.
        window: Window,
    },
    /// Run trace ids of one skill by eval outcome. Scope `telemetry:read`.
    TracesByEval {
        /// The graph.
        graph: String,
        /// The skill revision.
        skill_ref: String,
        /// Pass, fail or unscored.
        outcome: EvalOutcome,
        /// Most ids returned, at most 200.
        #[serde(default)]
        limit: Option<u32>,
        /// The cursor an earlier page returned.
        #[serde(default)]
        cursor: Option<String>,
    },
}

impl SkillTelemetryOp {
    /// Whether the op writes: only `Ingest` does.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Ingest { .. })
    }

    /// The graph the op reads and writes.
    pub fn graph(&self) -> &str {
        match self {
            Self::Ingest { graph, .. }
            | Self::Measures { graph, .. }
            | Self::Rank { graph, .. }
            | Self::Peers { graph, .. }
            | Self::TracesByEval { graph, .. } => graph,
        }
    }
}

/// What `Ingest` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct IngestReceipt {
    /// How many events were new.
    pub stored: u64,
    /// How many repeated an id that was already stored.
    pub repeated: u64,
}

/// One skill of a `Rank` answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RankedSkill {
    /// The skill revision.
    pub skill_ref: String,
    /// `eval_pass_rate_ppm * eval_runs / (eval_runs + prior_runs)`.
    pub quality_ppm: u64,
    /// The cases of its latest suite.
    pub eval_runs: u64,
    /// Its invocations in the window.
    pub invocations: u64,
}

/// What `Rank` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RankAnswer {
    /// The skills, best first.
    pub skills: Vec<RankedSkill>,
}

/// One skill of a `Peers` answer. No peer is named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PeerSkill {
    /// The skill revision.
    pub skill_ref: String,
    /// How many principals of the cohort used it.
    pub cohort_users: u64,
}

/// What `Peers` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PeersAnswer {
    /// The skills, most used first.
    pub skills: Vec<PeerSkill>,
}

/// What `TracesByEval` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TracesAnswer {
    /// The run trace ids, newest end first.
    pub trace_ids: Vec<String>,
    /// The cursor of the next page, when there is one.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation() -> serde_json::Value {
        serde_json::json!({
            "invocation_id": "i1", "tenant_id": "t", "skill_ref": "skill:a@1",
            "skill_version": "1", "chosen_by": "agent", "principal_pseudonym": "ab",
            "run_id": "r", "started_ms": 1, "ended_ms": 2,
            "outcome": {"error": {"code": "TOOL_TIMEOUT"}},
            "cost_delta_micro_usd": -5, "tokens_in": 10, "tokens_out": 20
        })
    }

    // spec: MEDULLA-DECISION-ENGINE-R713.1
    #[test]
    fn invocation_has_no_free_text_and_measures_has_twelve_fields() {
        let parsed: SkillInvocation = serde_json::from_value(invocation()).expect("decodes");
        assert_eq!(serde_json::to_value(&parsed).unwrap(), invocation());
        // A field outside the type is refused, so no free text can ride along.
        let mut extra = invocation();
        extra["prompt"] = "please summarise my notes".into();
        assert!(serde_json::from_value::<SkillInvocation>(extra).is_err());
        // The error code is a closed form of at most 64 bytes.
        assert!(parsed.outcome.code_valid());
        for bad in ["", "lower case", &"X".repeat(65)] {
            assert!(!InvocationOutcome::Error { code: bad.to_string() }.code_valid());
        }
        // The pseudonym is stable, tenant-bound and salted.
        let one = principal_pseudonym("t", "alice", 1);
        assert_eq!(one, principal_pseudonym("t", "alice", 1));
        assert_ne!(one, principal_pseudonym("t", "alice", 2));
        assert_ne!(one, principal_pseudonym("u", "alice", 1));
        assert_eq!(one.len(), 64);
        // Exactly twelve measures.
        let measures = serde_json::to_value(SkillMeasures::default()).unwrap();
        let count = measures.as_object().unwrap().len();
        assert_eq!(count, 12);
    }

    // spec: MEDULLA-DECISION-ENGINE-R713.8
    #[test]
    fn a_window_names_its_day_buckets_and_a_long_one_names_none() {
        let day = INDEX_DAY_MS;
        assert_eq!(started_day(day - 1), 0);
        assert_eq!(started_day(day), 1);
        assert_eq!(window_days(Window { start_ms: 0, end_ms: 3 * day }), Some(0..=2));
        assert_eq!(window_days(Window { start_ms: day / 2, end_ms: day + 1 }), Some(0..=1));
        assert_eq!(window_days(Window { start_ms: 5, end_ms: 5 }), None);
        assert!(window_days(Window { start_ms: 0, end_ms: INDEX_DAYS_MAX * day }).is_some());
        assert!(window_days(Window { start_ms: 0, end_ms: (INDEX_DAYS_MAX + 1) * day }).is_none());
    }
}
