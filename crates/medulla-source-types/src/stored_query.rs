//! Stored, versioned queries and their materialized results
//! (MEDULLA-FEDERATED-QUERY-R708.1 to R708.8).
//!
//! A stored query is a named, versioned, parameterized query. Every put publishes one
//! immutable version. A call binds typed arguments, runs the highest published version (or
//! the one asked for) and, when the query materializes, keeps the result with the input
//! generation it was computed from. Everything here is a pure function of its arguments;
//! the served handler in `medulla-h-query` owns storage and execution.

use std::collections::BTreeMap;

use medulla_identity_types::identity::{
    owner_or_matching_scope, KnowledgeUnit, ShareScope, UnitPrincipal,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The shortest periodic refresh.
pub const MIN_REFRESH_PERIOD_S: u64 = 60;

pub const STORED_QUERY_INVALID: &str = "STORED_QUERY_INVALID";
pub const STORED_QUERY_PARAM_MISSING: &str = "STORED_QUERY_PARAM_MISSING";
pub const STORED_QUERY_IMMUTABLE: &str = "STORED_QUERY_IMMUTABLE";
pub const STORED_QUERY_NOT_FOUND: &str = "STORED_QUERY_NOT_FOUND";
pub const ACCESS_DENIED: &str = "ACCESS_DENIED";

/// A typed stored-query refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[error("{code}: {detail}")]
pub struct StoredQueryRefusal {
    pub code: String,
    pub detail: String,
}

/// Build a refusal.
pub fn refuse(code: &str, detail: impl Into<String>) -> StoredQueryRefusal {
    StoredQueryRefusal {
        code: code.to_string(),
        detail: detail.into(),
    }
}

/// The language a stored query is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StoredQueryLanguage {
    Uql,
    Sql,
    Sparql,
}

/// The declared type of one parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StoredParamType {
    String,
    Int,
    Float,
    Bool,
}

/// When a materialized result recomputes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum RefreshPolicy {
    /// When the input generation advanced past the one the result was computed from.
    OnChange,
    /// Every `seconds`, at least [`MIN_REFRESH_PERIOD_S`].
    Period { seconds: u64 },
}

fn private_scope() -> ShareScope {
    ShareScope::Private
}

/// A stored query (MEDULLA-FEDERATED-QUERY-R708.1, R708.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredQuery {
    pub name: String,
    pub version: u32,
    pub language: StoredQueryLanguage,
    pub text: String,
    pub params: BTreeMap<String, StoredParamType>,
    pub materialize: bool,
    pub refresh: RefreshPolicy,
    /// The verified principal that published the query. The engine stamps it on a put and
    /// ignores whatever a request carries.
    #[serde(default)]
    pub author: String,
    /// Who besides the author may call it.
    #[serde(default = "private_scope")]
    pub share: ShareScope,
}

impl StoredQuery {
    /// Refuse an empty name or text, version zero and a period under 60 s.
    pub fn validate(&self) -> Result<(), StoredQueryRefusal> {
        let fail = |detail: &str| Err(refuse(STORED_QUERY_INVALID, detail));
        if self.name.trim().is_empty() {
            return fail("name is required");
        }
        if self.text.trim().is_empty() {
            return fail("text is required");
        }
        if self.version == 0 {
            return fail("version starts at 1");
        }
        match self.refresh {
            RefreshPolicy::Period { seconds } if seconds < MIN_REFRESH_PERIOD_S => {
                fail("refresh period is under 60 s")
            }
            _ => Ok(()),
        }
    }

    /// Bind call arguments against the declared parameters. Every declared parameter must be
    /// present (`STORED_QUERY_PARAM_MISSING`), no other name may appear and every value must
    /// have its declared type (`STORED_QUERY_INVALID`). The result holds the values as the
    /// typed JSON the language runner binds, never spliced into the text.
    pub fn bind(
        &self,
        args: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, StoredQueryRefusal> {
        if let Some(extra) = args.keys().find(|name| !self.params.contains_key(*name)) {
            return Err(refuse(
                STORED_QUERY_INVALID,
                format!("`{extra}` is not a parameter of `{}`", self.name),
            ));
        }
        let mut bound = BTreeMap::new();
        for (name, declared) in &self.params {
            let value = args.get(name).ok_or_else(|| {
                refuse(
                    STORED_QUERY_PARAM_MISSING,
                    format!("parameter `{name}` of `{}` is not bound", self.name),
                )
            })?;
            let typed = match (declared, value) {
                (StoredParamType::String, Value::String(_)) => value.clone(),
                (StoredParamType::Bool, Value::Bool(_)) => value.clone(),
                (StoredParamType::Int, Value::Number(n)) if n.is_i64() || n.is_u64() => {
                    value.clone()
                }
                (StoredParamType::Float, Value::Number(_)) => value.clone(),
                _ => {
                    return Err(refuse(
                        STORED_QUERY_INVALID,
                        format!("parameter `{name}` is not a {declared:?} value"),
                    ))
                }
            };
            bound.insert(name.clone(), typed);
        }
        Ok(bound)
    }

    /// The text a language runner executes for one binding. A UQL text is returned
    /// unchanged because the UQL parser binds its own typed parameters. A SQL or SPARQL
    /// text has each `$name` placeholder outside quoted text and comments replaced by one
    /// typed literal rendered from the bound value: a string is quoted and escaped, a
    /// number or boolean is its canonical token. A placeholder that names no declared
    /// parameter refuses `STORED_QUERY_INVALID`.
    pub fn render(&self, bound: &BTreeMap<String, Value>) -> Result<String, StoredQueryRefusal> {
        if self.language == StoredQueryLanguage::Uql {
            return Ok(self.text.clone());
        }
        let sparql = self.language == StoredQueryLanguage::Sparql;
        let chars: Vec<char> = self.text.chars().collect();
        let mut out = String::with_capacity(self.text.len());
        let mut at = 0;
        while at < chars.len() {
            let c = chars[at];
            match c {
                '\'' | '"' => {
                    let end = quoted_end(&chars, at, sparql);
                    out.extend(&chars[at..end]);
                    at = end;
                }
                '-' if !sparql && chars.get(at + 1) == Some(&'-') => {
                    let end = line_end(&chars, at);
                    out.extend(&chars[at..end]);
                    at = end;
                }
                '#' if sparql => {
                    let end = line_end(&chars, at);
                    out.extend(&chars[at..end]);
                    at = end;
                }
                '/' if !sparql && chars.get(at + 1) == Some(&'*') => {
                    let mut end = at + 2;
                    while end < chars.len() && !(chars[end - 1] == '*' && chars[end] == '/') {
                        end += 1;
                    }
                    let end = (end + 1).min(chars.len());
                    out.extend(&chars[at..end]);
                    at = end;
                }
                '$' => {
                    let mut end = at + 1;
                    while end < chars.len()
                        && (chars[end].is_ascii_alphanumeric() || chars[end] == '_')
                    {
                        end += 1;
                    }
                    let name: String = chars[at + 1..end].iter().collect();
                    let value = bound.get(&name).ok_or_else(|| {
                        refuse(
                            STORED_QUERY_INVALID,
                            format!("`${name}` is not a parameter of `{}`", self.name),
                        )
                    })?;
                    out.push_str(&literal(value, sparql)?);
                    at = end;
                }
                _ => {
                    out.push(c);
                    at += 1;
                }
            }
        }
        Ok(out)
    }

    /// Whether `reader` may call this query: its author always, anyone else only inside the
    /// share scope. The scope rule is the one sharing rule of the identity types.
    pub fn may_call(&self, reader: &UnitPrincipal, author: &UnitPrincipal) -> bool {
        let unit = KnowledgeUnit {
            id: format!("stored_query:{}:{}", self.name, self.version),
            owner: self.author.clone(),
            version: u64::from(self.version),
            supersedes: None,
            access_labels: Default::default(),
            share_scope: self.share,
        };
        let owners = BTreeMap::from([(author.id.clone(), author.clone())]);
        owner_or_matching_scope(reader, &unit, &owners)
    }
}

/// The index just past the quoted run that opens at `start` (a doubled quote does not end it, and in SPARQL
/// neither does a backslash escape).
fn quoted_end(chars: &[char], start: usize, backslash_escapes: bool) -> usize {
    let quote = chars[start];
    let mut at = start + 1;
    while at < chars.len() {
        if backslash_escapes && chars[at] == '\\' {
            at += 2;
        } else if chars[at] == quote {
            if chars.get(at + 1) == Some(&quote) {
                at += 2;
            } else {
                return at + 1;
            }
        } else {
            at += 1;
        }
    }
    chars.len()
}

/// The index of the end of the line that holds `start`.
fn line_end(chars: &[char], start: usize) -> usize {
    chars[start..]
        .iter()
        .position(|c| *c == '\n')
        .map_or(chars.len(), |offset| start + offset)
}

/// One bound value as a literal of the target language.
fn literal(value: &Value, sparql: bool) -> Result<String, StoredQueryRefusal> {
    match value {
        Value::String(text) if text.contains('\0') => Err(refuse(
            STORED_QUERY_INVALID,
            "a string argument holds a NUL",
        )),
        Value::String(text) if sparql => {
            let mut out = String::from("\"");
            for c in text.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    other => out.push(other),
                }
            }
            out.push('"');
            Ok(out)
        }
        Value::String(text) => Ok(format!("'{}'", text.replace('\'', "''"))),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Number(number) if number.is_i64() || number.is_u64() => Ok(number.to_string()),
        Value::Number(number) => match number.as_f64().filter(|f| f.is_finite()) {
            Some(float) => Ok(format!("{float:?}")),
            None => Err(refuse(
                STORED_QUERY_INVALID,
                "a number argument is not finite",
            )),
        },
        _ => Err(refuse(STORED_QUERY_INVALID, "an argument is not a scalar")),
    }
}

/// The digest of bound arguments: one materialized result per distinct binding.
pub fn args_digest(bound: &BTreeMap<String, Value>) -> String {
    let canonical = serde_json::to_vec(bound).unwrap_or_default();
    hex::encode(Sha256::digest(canonical))
}

/// The stored materialized result of one (query version, binding) pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredResult {
    /// The result columns, in order.
    pub columns: Vec<String>,
    pub rows: Value,
    pub row_count: u64,
    /// The graph snapshot revision the rows were computed from.
    pub input_generation: u64,
    /// When the rows were computed, in milliseconds.
    pub refreshed_at_ms: u64,
}

/// What one call returns (MEDULLA-FEDERATED-QUERY-R708.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredCall {
    pub name: String,
    pub version: u32,
    pub materialized: bool,
    pub columns: Vec<String>,
    pub rows: Value,
    pub row_count: u64,
    pub input_generation: u64,
    pub refreshed_at_ms: u64,
    /// Whole seconds since `refreshed_at_ms`, computed on read.
    pub staleness_s: u64,
}

/// Whole seconds from a refresh to a read.
pub fn staleness_s(refreshed_at_ms: u64, now_ms: u64) -> u64 {
    now_ms.saturating_sub(refreshed_at_ms) / 1000
}

/// Whether serving needs to evaluate the query: no stored result yet, a period that has
/// elapsed, or `OnChange` (which evaluates through the dependency-keyed result cache).
pub fn needs_run(policy: RefreshPolicy, prior: Option<&StoredResult>, now_ms: u64) -> bool {
    match (policy, prior) {
        (_, None) | (RefreshPolicy::OnChange, _) => true,
        (RefreshPolicy::Period { seconds }, Some(prior)) => {
            now_ms.saturating_sub(prior.refreshed_at_ms) >= seconds.saturating_mul(1000)
        }
    }
}

/// How a materialized read was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// The stored result was current; it is returned as it was.
    Reused,
    /// The result was missing or changed and `run` produced the new rows.
    Recomputed,
}

/// Serve one materialized read.
///
/// `run` evaluates the query and returns the rows (a JSON array) with the input generation
/// they were read at. Under `OnChange` it runs on every read: the served result cache keyed
/// by the dependency set of the plan answers it without work while no input changed, and a
/// result whose generation equals the stored one is reused unchanged, so only a real input
/// change rewrites the record. Under `Period` the stored result is returned untouched until
/// its period has elapsed and `run` is not called. A failed run returns its error and the
/// caller keeps the prior result.
pub fn serve_materialized(
    policy: RefreshPolicy,
    prior: Option<StoredResult>,
    now_ms: u64,
    run: impl FnOnce() -> Result<(Vec<String>, Value, u64), String>,
) -> Result<(StoredResult, Served), String> {
    if !needs_run(policy, prior.as_ref(), now_ms) {
        if let Some(prior) = prior {
            return Ok((prior, Served::Reused));
        }
    }
    let (columns, rows, generation) = run()?;
    if let (RefreshPolicy::OnChange, Some(prior)) = (policy, &prior) {
        if prior.input_generation == generation {
            return Ok((prior.clone(), Served::Reused));
        }
    }
    let row_count = rows.as_array().map_or(0, |rows| rows.len() as u64);
    Ok((
        StoredResult {
            columns,
            rows,
            row_count,
            input_generation: generation,
            refreshed_at_ms: now_ms,
        },
        Served::Recomputed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn query(refresh: RefreshPolicy) -> StoredQuery {
        StoredQuery {
            name: "open_incidents".into(),
            version: 1,
            language: StoredQueryLanguage::Uql,
            text: "MATCH (i:Incident) WHERE i.status = $status RETURN i".into(),
            params: BTreeMap::from([("status".to_string(), StoredParamType::String)]),
            materialize: true,
            refresh,
            author: "usr:alice".into(),
            share: ShareScope::Private,
        }
    }

    fn principal(id: &str, tenant: &str) -> UnitPrincipal {
        UnitPrincipal {
            id: id.into(),
            placement: "g".into(),
            tenant: tenant.into(),
        }
    }

    fn text_query(language: StoredQueryLanguage, text: &str) -> StoredQuery {
        StoredQuery {
            language,
            text: text.into(),
            params: BTreeMap::from([
                ("status".to_string(), StoredParamType::String),
                ("n".to_string(), StoredParamType::Int),
            ]),
            ..query(RefreshPolicy::OnChange)
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R708.10
    #[test]
    fn sql_and_sparql_text_binds_typed_literals() {
        let bound = BTreeMap::from([
            ("status".to_string(), json!("o'pen\" x\\")),
            ("n".to_string(), json!(3)),
        ]);
        let sql = text_query(
            StoredQueryLanguage::Sql,
            "SELECT id FROM nodes WHERE status = $status AND n > $n -- $ignored 'x\n",
        );
        assert_eq!(
            sql.render(&bound).unwrap(),
            "SELECT id FROM nodes WHERE status = 'o''pen\" x\\' AND n > 3 -- $ignored 'x"
        );
        let sparql = text_query(
            StoredQueryLanguage::Sparql,
            "SELECT ?s WHERE { ?s <p> $status . FILTER(?n > $n && \"$status\" != ?s) }",
        );
        assert_eq!(
            sparql.render(&bound).unwrap(),
            "SELECT ?s WHERE { ?s <p> \"o'pen\\\" x\\\\\" . FILTER(?n > 3 && \"$status\" != ?s) }"
        );
        let undeclared = text_query(StoredQueryLanguage::Sql, "SELECT $other");
        assert_eq!(
            undeclared.render(&bound).unwrap_err().code,
            STORED_QUERY_INVALID
        );
        let uql = text_query(StoredQueryLanguage::Uql, "MATCH (n) WHERE s = $status");
        assert_eq!(uql.render(&bound).unwrap(), uql.text);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R708.1
    #[test]
    fn stored_query_round_trips() {
        for refresh in [
            RefreshPolicy::OnChange,
            RefreshPolicy::Period { seconds: 60 },
        ] {
            let q = query(refresh);
            q.validate().unwrap();
            let json = serde_json::to_string(&q).unwrap();
            assert_eq!(serde_json::from_str::<StoredQuery>(&json).unwrap(), q);
        }
        assert!(query(RefreshPolicy::Period { seconds: 59 })
            .validate()
            .is_err());
        assert!(serde_json::from_str::<RefreshPolicy>(r#"{"kind":"hourly"}"#).is_err());
        let mut unnamed = query(RefreshPolicy::OnChange);
        unnamed.name = String::new();
        assert_eq!(unnamed.validate().unwrap_err().code, STORED_QUERY_INVALID);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R708.4
    #[test]
    fn bind_refuses_missing_extra_and_mistyped_arguments() {
        let q = query(RefreshPolicy::OnChange);
        let ok = BTreeMap::from([("status".to_string(), json!("open"))]);
        assert_eq!(q.bind(&ok).unwrap(), ok);
        let missing = q.bind(&BTreeMap::new()).unwrap_err();
        assert_eq!(missing.code, STORED_QUERY_PARAM_MISSING);
        let mistyped = q
            .bind(&BTreeMap::from([("status".to_string(), json!(3))]))
            .unwrap_err();
        assert_eq!(mistyped.code, STORED_QUERY_INVALID);
        let extra = q
            .bind(&BTreeMap::from([
                ("status".to_string(), json!("open")),
                ("other".to_string(), json!(1)),
            ]))
            .unwrap_err();
        assert_eq!(extra.code, STORED_QUERY_INVALID);
        let mut numeric = query(RefreshPolicy::OnChange);
        numeric.params = BTreeMap::from([
            ("n".to_string(), StoredParamType::Int),
            ("f".to_string(), StoredParamType::Float),
        ]);
        assert!(numeric
            .bind(&BTreeMap::from([
                ("n".to_string(), json!(1.5)),
                ("f".to_string(), json!(2))
            ]))
            .is_err());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R708.8
    #[test]
    fn share_scope_limits_who_may_call() {
        let author = principal("usr:alice", "t1");
        let sharee = principal("usr:bob", "t1");
        let outsider = principal("usr:eve", "t2");
        let mut q = query(RefreshPolicy::OnChange);
        assert!(q.may_call(&author, &author));
        assert!(!q.may_call(&sharee, &author));
        q.share = ShareScope::Tenant;
        assert!(q.may_call(&sharee, &author));
        assert!(!q.may_call(&outsider, &author));
    }

    fn stored(generation: u64, refreshed_at_ms: u64) -> StoredResult {
        StoredResult {
            columns: vec![],
            rows: json!([{"id": "a"}]),
            row_count: 1,
            input_generation: generation,
            refreshed_at_ms,
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R708.2
    #[test]
    fn serving_reuses_a_current_result_and_recomputes_a_changed_or_due_one() {
        let run_new = || Ok((vec![], json!([{"id": "a"}, {"id": "b"}]), 8));
        // OnChange: the same generation reuses the stored record, a new one rewrites it.
        let (got, how) = serve_materialized(
            RefreshPolicy::OnChange,
            Some(stored(7, 1_000)),
            9_000,
            || Ok((vec![], json!([{"id": "a"}]), 7)),
        )
        .unwrap();
        assert_eq!((how, got.refreshed_at_ms), (Served::Reused, 1_000));
        let (got, how) = serve_materialized(
            RefreshPolicy::OnChange,
            Some(stored(7, 1_000)),
            9_000,
            run_new,
        )
        .unwrap();
        assert_eq!(
            (
                how,
                got.row_count,
                got.input_generation,
                got.refreshed_at_ms
            ),
            (Served::Recomputed, 2, 8, 9_000)
        );
        // Period: reuses inside the period without running, recomputes at it.
        let period = RefreshPolicy::Period { seconds: 60 };
        let (_, how) =
            serve_materialized(period, Some(stored(7, 1_000)), 60_999, || Err("no".into()))
                .unwrap();
        assert_eq!(how, Served::Reused);
        let (got, how) =
            serve_materialized(period, Some(stored(7, 1_000)), 61_000, run_new).unwrap();
        assert_eq!((how, got.refreshed_at_ms), (Served::Recomputed, 61_000));
        // The first read computes, and a failed run is an error.
        let (_, how) = serve_materialized(period, None, 5, run_new).unwrap();
        assert_eq!(how, Served::Recomputed);
        let failed = serve_materialized(period, Some(stored(7, 1_000)), 61_000, || {
            Err("boom".to_string())
        });
        assert_eq!(failed.unwrap_err(), "boom");
    }

    // spec: MEDULLA-FEDERATED-QUERY-R708.5
    #[test]
    fn staleness_counts_whole_seconds() {
        assert_eq!(staleness_s(1_000, 1_999), 0);
        assert_eq!(staleness_s(1_000, 4_500), 3);
        assert_eq!(staleness_s(5_000, 1_000), 0);
    }

    #[test]
    fn distinct_bindings_have_distinct_digests() {
        let a = BTreeMap::from([("x".to_string(), json!(1))]);
        let b = BTreeMap::from([("x".to_string(), json!(2))]);
        assert_ne!(args_digest(&a), args_digest(&b));
        assert_eq!(args_digest(&a), args_digest(&a.clone()));
    }
}
