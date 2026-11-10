//! Column masking and row-filter policies (MEDULLA-FEDERATED-QUERY-R706).
//!
//! ONE policy set, ONE rewrite, every query language: a policy binds to a
//! classification tag (MEDULLA-TYPED-PACKS-R652 markings / R803 detections).
//! [`apply_policies`] rewrites the shared [`Plan`] IR before pushdown — the
//! same rewrite serves UQL, UnifiedQuery, SQL and SPARQL (the served SQL and
//! SPARQL paths reach it through the projection adapters [`sql_read_rewrite`]
//! and [`sparql_read_rewrite`]; `execute` runs it at its top,
//! MEDULLA-FEDERATED-QUERY-R706.3). The rewrite is pure and writes nothing:
//! it mutates only the owned pre-pushdown plan copy and returns the applied
//! policies as evidence records (no masked value ever rides on the record).

use sha2::{Digest, Sha256};

use medulla_types::wire::{CmpOp, Op, Plan, Pred, PredLiteral};

use serde::{Deserialize, Serialize};

/// How a masked column value is rewritten. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskMethod {
    Null,
    Hash,
    Partial,
    Redact,
}

/// A masking policy on every column carrying `tag` (MEDULLA-FEDERATED-QUERY-R706.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaskPolicy {
    pub tag: String,
    #[serde(default)]
    pub principals_exempt: Vec<String>,
    pub method: MaskMethod,
}

/// A row filter on every table carrying `tag` (MEDULLA-FEDERATED-QUERY-R706.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowFilterPolicy {
    pub tag: String,
    /// The predicate rows must satisfy, in the shared predicate syntax.
    pub predicate: String,
}

/// The tag-based policies bound to ONE query run (MEDULLA-FEDERATED-QUERY-R706.2).
/// Loaded per tenant (MEDULLA-FEDERATED-QUERY-R706.7); `Default` is the no-policy
/// set, so a run with no loaded policies rewrites nothing — byte-for-byte the
/// unmasked fold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicySet {
    #[serde(default)]
    pub masks: Vec<MaskPolicy>,
    #[serde(default)]
    pub row_filters: Vec<RowFilterPolicy>,
}

impl PolicySet {
    /// True when no policy is bound — the rewrite is a no-op.
    pub fn is_empty(&self) -> bool {
        self.masks.is_empty() && self.row_filters.is_empty()
    }

    /// The mask policy bound to `tag`, if any (one policy per tag). A projected
    /// column is tagged when the plan/projection names it by the classification
    /// tag the loader (R706.7) resolves off the R652 column claims.
    pub fn mask_for(&self, tag: &str) -> Option<&MaskPolicy> {
        self.masks.iter().find(|m| m.tag == tag)
    }

    /// The row-filter policy bound to `tag`, if any.
    pub fn row_filter_for(&self, tag: &str) -> Option<&RowFilterPolicy> {
        self.row_filters.iter().find(|f| f.tag == tag)
    }
}

/// The verified principal a query runs as, matched against
/// [`MaskPolicy::principals_exempt`] (MEDULLA-FEDERATED-QUERY-R706.2). The loader
/// (R706.7) binds the caller's verified principal id; exemption is an EXACT string
/// match — an unbound principal (the empty id) matches no exemption entry, so a
/// bound policy set without a bound principal masks everything (fail closed,
/// never a wildcard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrincipalRef<'a>(pub &'a str);

impl<'a> From<&'a str> for PrincipalRef<'a> {
    fn from(id: &'a str) -> Self {
        Self(id)
    }
}

/// Which kind of policy an [`AppliedPolicy`] record names
/// (MEDULLA-FEDERATED-QUERY-R706.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    /// A column mask ([`MaskPolicy`]).
    Mask,
    /// A row filter ([`RowFilterPolicy`]).
    RowFilter,
}

/// One applied policy — the EXPLAIN evidence record (MEDULLA-FEDERATED-QUERY-R706.5).
/// Carries tag/kind/column and NO masked value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedPolicy {
    pub tag: String,
    pub kind: PolicyKind,
    /// The column the mask covers (Mask) or the column the predicate guards
    /// (RowFilter).
    pub column: String,
}

/// The value-level mask (MEDULLA-FEDERATED-QUERY-R706.2): `null` gives `NULL`;
/// `hash` gives lowercase SHA-256 hex; `partial` keeps the last 4 characters and
/// replaces the rest with `*` (a value of 4 characters or fewer is FULLY masked —
/// a masked value is never returned raw); `redact` gives `[REDACTED]`.
pub fn mask_expr(method: MaskMethod, value: &str) -> String {
    match method {
        MaskMethod::Null => "NULL".to_string(),
        MaskMethod::Hash => sha256_hex(value.as_bytes()),
        MaskMethod::Partial => {
            let n = value.len();
            if n <= 4 {
                "*".repeat(n)
            } else {
                format!("{}{}", "*".repeat(n - 4), &value[n - 4..])
            }
        }
        MaskMethod::Redact => "[REDACTED]".to_string(),
    }
}

/// Lowercase SHA-256 hex — the same digest the medulla-kernel audit chain hashes
/// with (the kernel's `sha2` `Sha256::digest` helper).
fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// The mask expression the PUSHED source query selects instead of the raw column
/// (MEDULLA-FEDERATED-QUERY-R706.3 — the pushed query carries the masked
/// projection, in source SQL form). `NULL` and `'[REDACTED]'` are source literals;
/// `SHA256(col)` / `MASK_PARTIAL(col)` are the pushed-projection function contract
/// the source side of the served row implements.
pub fn mask_column_expr(method: MaskMethod, column: &str) -> String {
    match method {
        MaskMethod::Null => "NULL".to_string(),
        MaskMethod::Hash => format!("SHA256({column})"),
        MaskMethod::Partial => format!("MASK_PARTIAL({column})"),
        MaskMethod::Redact => "'[REDACTED]'".to_string(),
    }
}

/// The projection adapter (MEDULLA-FEDERATED-QUERY-R706.3): one query language's
/// projected column list (a SQL SELECT list, a SPARQL SELECT variable list, the
/// UQL `RETURN` channels) adapted to the shared [`Plan`] IR, so the ONE rewrite
/// [`apply_policies`] serves every language.
pub fn projection_plan(columns: Vec<String>) -> Plan {
    Plan {
        ops: vec![Op::Project { channels: columns }],
    }
}

/// The shared predicate syntax → plan algebra (the R078 tenant predicates join the
/// row filters through this SAME builder, MEDULLA-FEDERATED-QUERY-R706.2). The
/// operator-writable subset: `prop = 'value'`, `prop = "value"`, `prop = number`
/// and `prop <op> number` (`>`, `>=`, `<`, `<=`).
pub fn predicate_from_syntax(text: &str) -> Option<Pred> {
    let text = text.trim();
    for (op_tok, op) in [
        (">=", CmpOp::Ge),
        (">", CmpOp::Gt),
        ("<=", CmpOp::Le),
        ("<", CmpOp::Lt),
    ] {
        if let Some((prop, rest)) = text.split_once(op_tok) {
            if let Ok(n) = rest.trim().parse::<f64>() {
                return Some(Pred::Cmp {
                    prop: prop.trim().to_string(),
                    op,
                    value: PredLiteral::Num(n),
                });
            }
        }
    }
    if let Some((prop, rest)) = text.split_once('=') {
        let (prop, rest) = (prop.trim(), rest.trim());
        let unquoted = rest
            .strip_prefix('\'')
            .and_then(|r| r.strip_suffix('\''))
            .or_else(|| rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')));
        if let Some(value) = unquoted {
            return Some(Pred::Eq {
                prop: prop.to_string(),
                value: value.to_string(),
            });
        }
        if let Ok(n) = rest.parse::<f64>() {
            return Some(Pred::Cmp {
                prop: prop.to_string(),
                op: CmpOp::Eq,
                value: PredLiteral::Num(n),
            });
        }
    }
    None
}

/// The plan rewrite before pushdown (MEDULLA-FEDERATED-QUERY-R706.2).
///
/// Pure and write-free: mutates only `plan` (the owned pre-pushdown copy) and
/// returns the applied policies as evidence records.
/// - Each projected column whose name carries a [`MaskPolicy`] tag takes the mask
///   expression (`Project` channel rewrite), unless `principals_exempt` lists the
///   principal.
/// - Each [`RowFilterPolicy::predicate`] joins by AND — appended to EVERY
///   `Filter` op's predicate list (a row must satisfy all of them), or inserted as
///   its own `Filter` directly after the plan's first (source) op when the plan
///   filters nothing yet. A row filter guards every row the plan touches; the
///   table-scoped narrowing arrives with the R706.7 loader.
///
/// A predicate the rewrite cannot express fails closed: it is recorded as an
/// empty membership list, which matches no row.
pub fn apply_policies(
    plan: &mut Plan,
    set: &PolicySet,
    principal: &PrincipalRef,
) -> Vec<AppliedPolicy> {
    let mut applied = Vec::new();

    for op in plan.ops.iter_mut() {
        if let Op::Project { channels } = op {
            for channel in channels.iter_mut() {
                let Some(mask) = set.mask_for(channel) else {
                    continue;
                };
                if mask
                    .principals_exempt
                    .iter()
                    .any(|exempt| *exempt == principal.0)
                {
                    // Exempt: the raw projection stands.
                    continue;
                }
                applied.push(AppliedPolicy {
                    tag: mask.tag.clone(),
                    kind: PolicyKind::Mask,
                    column: channel.clone(),
                });
                *channel = mask_column_expr(mask.method, channel);
            }
        }
    }

    for filter in &set.row_filters {
        let (pred, column) = match predicate_from_syntax(&filter.predicate) {
            Some(pred) => {
                let column = match &pred {
                    Pred::Eq { prop, .. } | Pred::Cmp { prop, .. } | Pred::In { prop, .. } => {
                        prop.clone()
                    }
                    _ => filter.predicate.clone(),
                };
                (pred, column)
            }
            // Fail closed: a predicate the rewrite cannot express admits no rows —
            // an empty membership list matches nothing.
            None => (
                Pred::In {
                    prop: filter.tag.clone(),
                    values: Vec::new(),
                },
                filter.tag.clone(),
            ),
        };
        join_row_filter(plan, pred);
        applied.push(AppliedPolicy {
            tag: filter.tag.clone(),
            kind: PolicyKind::RowFilter,
            column,
        });
    }

    applied
}

/// AND a row-filter predicate into the plan: every existing `Filter` gains it; a
/// plan with no `Filter` gains one directly after its first op (the source op of
/// every canonical plan — a non-source head filters an empty row set, fail
/// closed).
fn join_row_filter(plan: &mut Plan, pred: Pred) {
    if plan.ops.iter().any(|op| matches!(op, Op::Filter { .. })) {
        for op in plan.ops.iter_mut() {
            if let Op::Filter { preds } = op {
                preds.push(pred.clone());
            }
        }
    } else if !plan.ops.is_empty() {
        plan.ops.insert(1, Op::Filter { preds: vec![pred] });
    }
}

/// One SQL SELECT-list entry → (column, optional alias): `col AS alias`, `col
/// alias`, a quoted `"col"` (the tag-named form) or a bare `col`.
fn parse_select_entry(entry: &str) -> (String, Option<String>) {
    let entry = entry.trim();
    let upper = entry.to_ascii_uppercase();
    if let Some(at) = upper.rfind(" AS ") {
        let column = entry[..at].trim();
        let column = column
            .strip_prefix('"')
            .and_then(|c| c.strip_suffix('"'))
            .unwrap_or(column);
        return (column.to_string(), Some(entry[at + 4..].trim().to_string()));
    }
    if entry.len() >= 2 && entry.starts_with('"') && entry.ends_with('"') {
        return (entry[1..entry.len() - 1].to_string(), None);
    }
    let parts: Vec<&str> = entry.split_whitespace().collect();
    if parts.len() == 2 {
        return (parts[0].to_string(), Some(parts[1].to_string()));
    }
    (entry.to_string(), None)
}

/// The SQL projection adapter (MEDULLA-FEDERATED-QUERY-R706.3): a read SQL
/// statement's top-level SELECT list, run through the ONE rewrite [`apply_policies`].
/// Returns the pushed query — the masked projection in the SELECT list, the
/// row-filter predicates AND-joined into the WHERE (inserted before the `FROM`
/// when none is present) — plus the applied policies as evidence. Refuses (the
/// error is a refusal string) when a bound row filter cannot be spliced into the
/// shape: fail closed — a read with an inexpressible row filter never runs raw.
pub fn sql_read_rewrite(
    query: &str,
    set: &PolicySet,
    principal: &PrincipalRef,
) -> Result<(String, Vec<AppliedPolicy>), String> {
    if set.is_empty() {
        return Ok((query.to_string(), Vec::new()));
    }
    let upper = query.to_ascii_uppercase();
    let Some(select_at) = upper.find("SELECT ") else {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: no top-level SELECT projection to mask".to_string(),
        );
    };
    let list_start = select_at + "SELECT ".len();
    let Some(from_at) = upper[list_start..].find(" FROM ").map(|i| list_start + i) else {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: no top-level SELECT … FROM projection to mask"
                .to_string(),
        );
    };
    let select_list = &query[list_start..from_at];
    if select_list.contains('(') || select_list.contains('*') {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: the SELECT list is not a plain column list"
                .to_string(),
        );
    }
    let entries: Vec<String> = select_list
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    let columns: Vec<String> = entries.iter().map(|e| parse_select_entry(e).0).collect();
    let mut plan = projection_plan(columns);
    let applied = apply_policies(&mut plan, set, principal);
    let channels = match &plan.ops[0] {
        Op::Project { channels } => channels.clone(),
        _ => unreachable!("projection_plan emits exactly one Project"),
    };
    // Splice the (possibly masked) entries back, keeping each entry's alias.
    let new_list: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let rewritten = &channels[i];
            if rewritten == entry {
                entry.clone()
            } else {
                let alias = parse_select_entry(entry).1;
                match alias {
                    Some(alias) => format!("{rewritten} AS {alias}"),
                    None => rewritten.clone(),
                }
            }
        })
        .collect();
    // The row filters AND-join the existing WHERE clause (inserted right after
    // the `FROM …` table reference when the statement has none).
    let where_splice: Option<(usize, String)> = if set.row_filters.is_empty() {
        None
    } else {
        let clause = set
            .row_filters
            .iter()
            .map(|f| format!("({})", f.predicate.trim()))
            .collect::<Vec<_>>()
            .join(" AND ");
        let tail = &upper[from_at..];
        let where_pos = tail.find(" WHERE ").map(|i| from_at + i);
        let clause_end = [" GROUP BY ", " HAVING ", " ORDER BY ", " LIMIT "]
            .iter()
            .filter_map(|k| tail.find(k))
            .min()
            .map(|i| from_at + i)
            .unwrap_or(query.len());
        let mut at = clause_end;
        while at > from_at {
            match query.as_bytes().get(at - 1) {
                Some(b' ' | b'\t' | b'\n' | b'\r' | b';') => at -= 1,
                _ => break,
            }
        }
        let text = match where_pos {
            Some(_) => format!(" AND {clause}"),
            None => format!(" WHERE {clause}"),
        };
        Some((at, text))
    };
    let mut out = String::with_capacity(query.len());
    out.push_str(&query[..list_start]);
    out.push_str(&new_list.join(", "));
    match where_splice {
        Some((at, text)) => {
            out.push_str(&query[from_at..at]);
            out.push_str(&text);
            out.push_str(&query[at..]);
        }
        None => out.push_str(&query[from_at..]),
    }
    Ok((out, applied))
}

/// The SPARQL projection adapter (MEDULLA-FEDERATED-QUERY-R706.3): a SPARQL
/// `SELECT … WHERE …` statement's projection, run through the ONE rewrite
/// [`apply_policies`]. A row filter splices in as a `FILTER (…)` (the shared
/// predicate syntax is SPARQL-compatible once single quotes become double). A
/// COLUMN MASK cannot be expressed inline in SPARQL 1.1, so a tagged projection
/// is refused — fail closed, until the value-level boundary of the served row
/// lands. An empty policy set returns the query byte-for-byte.
pub fn sparql_read_rewrite(
    query: &str,
    set: &PolicySet,
    principal: &PrincipalRef,
) -> Result<(String, Vec<AppliedPolicy>), String> {
    if set.is_empty() {
        return Ok((query.to_string(), Vec::new()));
    }
    let upper = query.to_ascii_uppercase();
    let Some(select_at) = upper.find("SELECT ") else {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: no SPARQL SELECT projection to mask".to_string(),
        );
    };
    let list_start = select_at + "SELECT ".len();
    let Some(where_rel) = upper[list_start..].find(" WHERE ") else {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: no SPARQL SELECT … WHERE projection to mask"
                .to_string(),
        );
    };
    let vars: Vec<String> = query[list_start..list_start + where_rel]
        .split_whitespace()
        .map(|v| v.to_string())
        .collect();
    let mut plan = projection_plan(vars.clone());
    let applied = apply_policies(&mut plan, set, principal);
    // A SPARQL variable names a masked column when it equals a mask tag or the
    // tag's first path segment (?pii ← pii.email): the query text cannot resolve
    // the variable → column binding, so the served boundary fails closed instead
    // of projecting the raw value.
    let var_masked = set.masks.iter().any(|m| {
        m.principals_exempt
            .iter()
            .all(|exempt| principal.0 != exempt)
            && vars.iter().any(|v| {
                let name = v.strip_prefix('?').unwrap_or(v.as_str());
                m.tag == name || m.tag.starts_with(&format!("{name}."))
            })
    });
    if applied.iter().any(|a| a.kind == PolicyKind::Mask) || var_masked {
        return Err(
            "POLICY_SPARQL_MASK_UNSUPPORTED: SPARQL 1.1 cannot express the masked \
             projection inline; a tagged projection is refused until the value-level \
             boundary of MEDULLA-FEDERATED-QUERY-R706.3 lands"
                .to_string(),
        );
    }
    let filters: Vec<String> = set
        .row_filters
        .iter()
        .map(|f| format!("FILTER ({})", f.predicate.trim().replace('\'', "\"")))
        .collect();
    if filters.is_empty() {
        return Ok((query.to_string(), applied));
    }
    let insert_at = list_start + where_rel + " WHERE ".len();
    let mut out = query.to_string();
    out.insert_str(insert_at, &format!("{} ", filters.join(" ")));
    Ok((out, applied))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R706.1
    #[test]
    fn policies_round_trip() {
        for method in [
            MaskMethod::Null,
            MaskMethod::Hash,
            MaskMethod::Partial,
            MaskMethod::Redact,
        ] {
            let mask = MaskPolicy {
                tag: "pii.email".into(),
                principals_exempt: vec!["role:dpo".into()],
                method,
            };
            let json = serde_json::to_string(&mask).unwrap();
            assert_eq!(serde_json::from_str::<MaskPolicy>(&json).unwrap(), mask);
        }
        assert!(serde_json::from_str::<MaskMethod>("\"encrypt\"").is_err());
        let filter = RowFilterPolicy {
            tag: "region.eu".into(),
            predicate: "region = 'eu'".into(),
        };
        let json = serde_json::to_string(&filter).unwrap();
        assert_eq!(
            serde_json::from_str::<RowFilterPolicy>(&json).unwrap(),
            filter
        );
        assert!(serde_json::from_str::<RowFilterPolicy>(r#"{"tag":"t"}"#).is_err());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R706.2
    #[test]
    fn mask_expr_covers_the_closed_set() {
        assert_eq!(mask_expr(MaskMethod::Null, "john@example.com"), "NULL");
        assert_eq!(
            mask_expr(MaskMethod::Redact, "john@example.com"),
            "[REDACTED]"
        );
        assert_eq!(
            mask_expr(MaskMethod::Partial, "password-123"),
            "********-123"
        );
        assert_eq!(mask_expr(MaskMethod::Partial, "ab"), "**");
        assert_eq!(mask_expr(MaskMethod::Partial, "abcd"), "****");
        // lowercase SHA-256 hex of "john@example.com"
        assert_eq!(
            mask_expr(MaskMethod::Hash, "john@example.com"),
            "855f96e983f1f8e8be944692b6f719fd54329826cb62e98015efee8e2e071dd4"
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R706.2
    #[test]
    fn apply_policies_masks_projection_and_and_joins_row_filters() {
        let mut plan = Plan {
            ops: vec![
                Op::Scan {
                    label: "Person".into(),
                },
                Op::Filter {
                    preds: vec![Pred::Eq {
                        prop: "tenant".into(),
                        value: "acme".into(),
                    }],
                },
                Op::Project {
                    channels: vec![
                        "pii.email".into(),
                        "pii.ssn".into(),
                        "pii.notes".into(),
                        "plain".into(),
                    ],
                },
            ],
        };
        let set = PolicySet {
            masks: vec![
                MaskPolicy {
                    tag: "pii.email".into(),
                    principals_exempt: Vec::new(),
                    method: MaskMethod::Null,
                },
                MaskPolicy {
                    tag: "pii.ssn".into(),
                    principals_exempt: Vec::new(),
                    method: MaskMethod::Hash,
                },
                MaskPolicy {
                    tag: "pii.notes".into(),
                    principals_exempt: vec!["role:dpo".into()],
                    method: MaskMethod::Redact,
                },
            ],
            row_filters: vec![RowFilterPolicy {
                tag: "region.eu".into(),
                predicate: "region = 'eu'".into(),
            }],
        };
        let principal = PrincipalRef::from("role:dpo");
        let applied = apply_policies(&mut plan, &set, &principal);

        let masks: Vec<&AppliedPolicy> = applied
            .iter()
            .filter(|a| a.kind == PolicyKind::Mask)
            .collect();
        assert_eq!(masks.len(), 2, "the exempt principal is not masked");
        assert!(applied
            .iter()
            .any(|a| a.kind == PolicyKind::RowFilter && a.tag == "region.eu"));

        let channels = match &plan.ops[2] {
            Op::Project { channels } => channels.clone(),
            _ => panic!("projection lost"),
        };
        assert_eq!(
            channels,
            vec!["NULL", "SHA256(pii.ssn)", "pii.notes", "plain"]
        );
        let preds = match &plan.ops[1] {
            Op::Filter { preds } => preds.clone(),
            _ => panic!("filter lost"),
        };
        assert_eq!(
            preds.len(),
            2,
            "the row filter AND-joins the tenant predicate"
        );
        assert!(matches!(&preds[1], Pred::Eq { prop, value } if prop == "region" && value == "eu"));
    }
}
