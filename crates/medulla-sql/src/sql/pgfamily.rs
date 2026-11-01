//! Postgres-family **extension parity** (wave 19): recognize the SQL syntax the
//! popular Postgres extensions expose so a client that speaks them Just Works over
//! the pgwire surface, lowering each onto the engine's native machinery.
//!
//!   * **pgvector index pushdown** (CONCEPT:MEDULLA-KG.query.real-ann-top-k) — `CREATE INDEX … USING hnsw|ivfflat
//!     (col vector_l2_ops)` registers an ANN index (again textual: `sqlparser` chokes on
//!     the opclass), and a `ORDER BY col <-> $1 LIMIT k` nearest-neighbour query is
//!     recognized so the top-k search can be pushed down to `medulla-ann` instead of the
//!     `MEDULLA-115` brute-force `vector_l2()` full-scan.
//!   * **TimescaleDB** (CONCEPT:MEDULLA-KG.query.continuous-aggregate-lowering) — `create_hypertable('t','ts')` records the
//!     time-partitioning metadata, and `CREATE MATERIALIZED VIEW … WITH
//!     (timescaledb.continuous) AS SELECT time_bucket(…) …` is a continuous aggregate
//!     lowered onto the durable view catalog + `time_bucket` (MEDULLA-067 `Op::Window`).
//!   * **ParadeDB** `col @@@ 'query'` BM25 search + `paradedb.score()`/`snippet()`
//!     (CONCEPT:MEDULLA-KG.query.paradedb-bm25) — recognized so the lexical search can lower onto `medulla-text`'s
//!     BM25 index (the `@@@` operator + `paradedb.*` functions are desugared in
//!     `classify::desugar_vector_ops`).
//!
//! This module is the PURE parse/plan/project layer (no graph, no runtime, no I/O), so
//! it is unit-testable without a socket. The server-side execution facades live in
//! `src/server/wire`.

use datafusion::sql::sqlparser::ast::{
    BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, LimitClause,
    OrderByKind, Query, SelectItem, SetExpr, Statement, TableFactor, UnaryOperator,
    Value as SqlValue,
};
use datafusion::sql::sqlparser::dialect::PostgreSqlDialect;
use datafusion::sql::sqlparser::parser::Parser;
use medulla_table_store::{tables::ann_plan::AnnIndexPlan, tables::ann_plan::HypertablePlan, tables::ann_plan::VectorMetric};

/// The SQL name the `medulla_embed` scalar function registers under.
pub const MEDULLA_EMBED_FN: &str = "medulla_embed";

// ─────────────────────────────────────────────────────────────────────────────
// Plan structs (the decoded shapes classify produces)
// ─────────────────────────────────────────────────────────────────────────────

/// A decoded `CREATE MATERIALIZED VIEW [IF NOT EXISTS] name WITH
/// (timescaledb.continuous) AS <select>` (CONCEPT:MEDULLA-KG.query.continuous-aggregate-lowering) — a continuous aggregate.
/// `select_sql` is the aggregate SELECT (typically a `time_bucket` GROUP BY) lowered
/// onto the durable view catalog; incremental materialized refresh is a follow-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuousAggPlan {
    pub name: String,
    pub select_sql: String,
    pub if_not_exists: bool,
}

/// A recognized pgvector nearest-neighbour query eligible for ANN index pushdown
/// (CONCEPT:MEDULLA-KG.query.real-ann-top-k): `… FROM table [WHERE …] ORDER BY column <op> <query> LIMIT k`.
/// `query` is the query-vector operand as SQL text (a `$N` placeholder or a `'[…]'`
/// pgvector literal) — the server resolves it and calls `medulla-ann::search(query, k)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnSearchPlan {
    pub table: String,
    pub column: String,
    pub metric: VectorMetric,
    pub k: usize,
    pub query: String,
}

/// A recognized ParadeDB BM25 search (CONCEPT:MEDULLA-KG.query.paradedb-bm25): `… FROM table WHERE column @@@
/// 'query' [ORDER BY paradedb.score(...) DESC] [LIMIT k]`. The server lowers it onto
/// `medulla-text`'s BM25 index (`TextIndex::search(query, k)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bm25SearchPlan {
    pub table: String,
    pub column: String,
    pub query: String,
    pub k: Option<usize>,
}

/// Read a dollar-quoted string (`$$ … $$` or `$tag$ … $tag$`) starting at byte
/// `start` (which must be `$`). Returns the body and the byte index just past the
/// closing tag. This mirrors Postgres dollar-quoting for stored-function bodies.
pub(super) fn read_dollar_quoted(s: &str, start: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if *bytes.get(start)? != b'$' {
        return None;
    }
    // Read the opening tag `$tag$`.
    let mut i = start + 1;
    while i < bytes.len() && bytes[i] != b'$' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let tag = &s[start..=i]; // includes both `$`
    let body_start = i + 1;
    let close = s[body_start..].find(tag)?;
    let body = &s[body_start..body_start + close];
    Some((body.to_string(), body_start + close + tag.len()))
}

// ─────────────────────────────────────────────────────────────────────────────
// MEDULLA-116 — textual recognition of CREATE INDEX … USING hnsw|ivfflat
// ─────────────────────────────────────────────────────────────────────────────

// Recognize `CREATE INDEX [CONCURRENTLY] [IF NOT EXISTS] [name] ON table USING
// hnsw|ivfflat (col [opclass])` (CONCEPT:MEDULLA-KG.query.real-ann-top-k) textually — `sqlparser` 0.51 cannot
// parse the opclass inside the column list, nor `IF NOT EXISTS` on an index. Returns
// `None` for a non-ANN `CREATE INDEX` (so `classify` falls through to its ordinary —
// currently-unsupported — path) or any other statement.
// ─────────────────────────────────────────────────────────────────────────────
// MEDULLA-117 — create_hypertable() + continuous aggregate
// ─────────────────────────────────────────────────────────────────────────────

/// Detect `SELECT create_hypertable('t','ts'[, …])` (CONCEPT:MEDULLA-KG.query.continuous-aggregate-lowering) in a parsed query
/// and extract the table + time column. Returns `None` for any other query so the
/// ordinary read path applies.
pub fn detect_create_hypertable(stmt: &Statement) -> Option<HypertablePlan> {
    let Statement::Query(q) = stmt else {
        return None;
    };
    let SetExpr::Select(select) = q.body.as_ref() else {
        return None;
    };
    if !select.from.is_empty() {
        return None;
    }
    let [SelectItem::UnnamedExpr(Expr::Function(f))] = select.projection.as_slice() else {
        return None;
    };
    if !last_fn_ident(f).eq_ignore_ascii_case("create_hypertable") {
        return None;
    }
    let FunctionArguments::List(list) = &f.args else {
        return None;
    };
    let mut strs = list.args.iter().filter_map(arg_as_string);
    let table = strs.next()?;
    let time_column = strs.next()?;
    Some(HypertablePlan { table, time_column })
}

/// Recognize `CREATE MATERIALIZED VIEW [IF NOT EXISTS] name WITH (timescaledb.continuous)
/// AS <select>` (CONCEPT:MEDULLA-KG.query.continuous-aggregate-lowering) textually — `sqlparser` cannot parse the dotted
/// `timescaledb.continuous` option. Returns `None` for a plain materialized view (which
/// the parser then rejects as a documented follow-up).
pub fn parse_continuous_aggregate(sql: &str) -> Option<ContinuousAggPlan> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let lower = trimmed.to_ascii_lowercase();
    let after = lower.strip_prefix("create materialized view")?;
    // Must be a timescaledb continuous aggregate.
    let cont_pos = lower.find("timescaledb.continuous")?;
    let mut rest = after.trim_start();
    let mut if_not_exists = false;
    if let Some(r) = rest.strip_prefix("if not exists") {
        if_not_exists = true;
        rest = r.trim_start();
    }
    // Name is the next token; extract from `trimmed` at the same byte offset (ASCII).
    let name: String = trimmed[trimmed.len() - rest.len()..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return None;
    }
    // The SELECT is everything after the first ` AS ` that follows the WITH clause.
    let as_search_from = cont_pos;
    let rel = lower[as_search_from..].find(" as ")?;
    let select_start = as_search_from + rel + 4;
    let mut select_sql = trimmed[select_start..].trim().to_string();
    // Strip a trailing `WITH [NO] DATA`.
    let sl = select_sql.to_ascii_lowercase();
    if let Some(p) = sl.rfind("with data").or_else(|| sl.rfind("with no data")) {
        // only strip if it is a trailing clause
        if sl[p..].trim_end() == "with data" || sl[p..].trim_end() == "with no data" {
            select_sql = select_sql[..p].trim_end().to_string();
        }
    }
    if select_sql.is_empty() {
        return None;
    }
    Some(ContinuousAggPlan {
        name,
        select_sql,
        if_not_exists,
    })
}

/// The last identifier of a function name (`paradedb.score` → `score`,
/// `create_hypertable` → `create_hypertable`).
fn last_fn_ident(f: &datafusion::sql::sqlparser::ast::Function) -> String {
    f.name
        .0
        .last()
        .and_then(|part| part.as_ident())
        .map(|ident| ident.value.clone())
        .unwrap_or_default()
}

/// A function argument that is a single-quoted string literal ⇒ its contents.
fn arg_as_string(arg: &FunctionArg) -> Option<String> {
    let expr = match arg {
        FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => e,
        FunctionArg::Named {
            arg: FunctionArgExpr::Expr(e),
            ..
        } => e,
        _ => return None,
    };
    match expr {
        Expr::Value(value) => match &value.value {
            SqlValue::SingleQuotedString(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MEDULLA-116 — ANN nearest-neighbour pushdown planner
// ─────────────────────────────────────────────────────────────────────────────

/// Recognize a pgvector nearest-neighbour query eligible for ANN index pushdown
/// (CONCEPT:MEDULLA-KG.query.real-ann-top-k) and return its [`AnnSearchPlan`] IFF a registered `indexes` entry
/// covers the `(table, column, metric)` — i.e. the query "chooses ANN". With no
/// matching index the caller falls back to the MEDULLA-115 brute-force `vector_l2()` scan.
///
/// Recognized shape: a single-table `SELECT … ORDER BY column <op> <query> LIMIT k`,
/// where `<op>` is a pgvector distance operator (`<->`/`<=>`/`<#>`).
pub fn plan_ann_search(sql: &str, indexes: &[AnnIndexPlan]) -> Option<AnnSearchPlan> {
    let stmts = Parser::parse_sql(&PostgreSqlDialect {}, sql).ok()?;
    let [Statement::Query(q)] = stmts.as_slice() else {
        return None;
    };
    // LIMIT k (required — ANN is a top-k search).
    let limit = match q.limit_clause.as_ref()? {
        LimitClause::LimitOffset {
            limit: Some(limit), ..
        }
        | LimitClause::OffsetCommaLimit { limit, .. } => limit,
        LimitClause::LimitOffset { limit: None, .. } => return None,
    };
    let k = match limit {
        Expr::Value(value) => match &value.value {
            SqlValue::Number(n, _) => n.parse::<usize>().ok()?,
            _ => return None,
        },
        _ => return None,
    };
    // ORDER BY column <op> query.
    let order_by = q.order_by.as_ref()?;
    let OrderByKind::Expressions(order_by) = &order_by.kind else {
        return None;
    };
    let [ob] = order_by.as_slice() else {
        return None;
    };
    let (column, metric, query) = vector_order_key(&ob.expr)?;
    // FROM must be a single bare table.
    let SetExpr::Select(select) = q.body.as_ref() else {
        return None;
    };
    let [twj] = select.from.as_slice() else {
        return None;
    };
    let TableFactor::Table { name, .. } = &twj.relation else {
        return None;
    };
    let table = name.0.last()?.as_ident()?.value.clone();
    // "Choose ANN" only when an index covers (table, column, metric).
    let covered = indexes.iter().any(|ix| {
        ix.table.eq_ignore_ascii_case(&table)
            && ix.column.eq_ignore_ascii_case(&column)
            && ix.metric == metric
    });
    if !covered {
        return None;
    }
    Some(AnnSearchPlan {
        table,
        column,
        metric,
        k,
        query,
    })
}

/// The pgvector distance metric a binary operator denotes, else `None`
/// (CONCEPT:MEDULLA-KG.query.real-ann-top-k). Shared by [`vector_order_key`] (which decodes the
/// whole `ORDER BY` key) and [`fold_const_embed_order_key`] (which only needs to know
/// that the key IS a vector-distance key before folding its query operand).
fn vector_metric_for_op(op: &BinaryOperator) -> Option<VectorMetric> {
    match op {
        // GOC-40: sqlparser 0.62.0 promoted `<->` from a generic
        // `Custom("<->")` token to the dedicated `TwoWayArrow`/`LtDashGt`
        // token+operator pair (`PostgreSqlDialect::supports_geometric_types`),
        // so the `Custom` arm below stopped matching it — confirmed by
        // parsing this exact query and inspecting the AST: `op: LtDashGt`.
        // `<#>` (inner-product) has no dedicated variant in this sqlparser
        // version, so it still tokenizes as `Custom("<#>")` and that arm is
        // unaffected; kept for when it too eventually gets its own token.
        BinaryOperator::LtDashGt => Some(VectorMetric::L2),
        BinaryOperator::Custom(s) if s == "<->" => Some(VectorMetric::L2),
        BinaryOperator::Custom(s) if s == "<#>" => Some(VectorMetric::InnerProduct),
        BinaryOperator::Spaceship => Some(VectorMetric::Cosine), // `<=>`
        _ => None,
    }
}

/// Decode an `ORDER BY column <-> query` key: returns `(column, metric, query_text)`
/// for a pgvector distance operator, else `None` (CONCEPT:MEDULLA-KG.query.real-ann-top-k).
fn vector_order_key(expr: &Expr) -> Option<(String, VectorMetric, String)> {
    let Expr::BinaryOp { left, op, right } = expr else {
        return None;
    };
    let metric = vector_metric_for_op(op)?;
    let column = match left.as_ref() {
        Expr::Identifier(id) => id.value.clone(),
        Expr::CompoundIdentifier(ids) => ids.last()?.value.clone(),
        _ => return None,
    };
    Some((column, metric, right.to_string()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Design §9 phase 2 — const-fold `medulla_embed('literal')` for the ANN pushdown probe
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve ONE literal query text to its dense query vector. `None` means "not
/// resolvable here" (no embedder bound, the embedder failed, a non-finite element) —
/// never an error: the caller then simply does not fold, and the query keeps the
/// MEDULLA-115 brute-force path.
pub(crate) type ConstEmbedFn<'a> = &'a dyn Fn(&str) -> Option<Vec<f32>>;

/// Const-fold a `ORDER BY col <op> medulla_embed('literal')` key's QUERY operand to the
/// pgvector text literal it denotes (design §9 phase 2), returning the rewritten SQL —
/// or `None` when there is nothing to fold.
///
/// This exists because [`plan_ann_search`] runs on PRE-desugar SQL and
/// [`vector_order_key`] recognises only a **literal** query vector, so
/// `ORDER BY emb <=> medulla_embed('leaky pump') LIMIT 10` is a function CALL, the ANN
/// pushdown declines, and the query silently falls back to the MEDULLA-115 brute-force exact
/// scan — correct rows, but without the HNSW/IVF speedup that is most of the reason the
/// index exists. Folding first lets `vector_order_key` recognise the literal it ALREADY
/// understands; it is deliberately NOT weakened to accept arbitrary expressions.
///
/// Only an `medulla_embed` call whose single argument is a single-quoted string literal is
/// foldable — `medulla_embed($1)` (an unresolved bind placeholder) and `medulla_embed(col)` (a
/// per-row column argument) are NOT constants, fold to `None`, and correctly keep the
/// brute-force path. `Volatility::Immutable` (see `embed_udf`) is what makes folding a
/// literal legitimate at all.
///
/// The result is for the ANN PROBE only — the SQL that actually executes is left
/// untouched, so no `Display` round-trip of the user's statement can ever change what
/// runs. The executed `medulla_embed` call resolves to the same vector by the immutability
/// contract.
pub(crate) fn fold_const_embed_order_key(sql: &str, embed: ConstEmbedFn<'_>) -> Option<String> {
    let mut stmts = Parser::parse_sql(&PostgreSqlDialect {}, sql).ok()?;
    let [stmt] = stmts.as_mut_slice() else {
        return None;
    };
    let Statement::Query(q) = stmt else {
        return None;
    };
    let key = vector_order_key_query_operand(q)?;
    let text = const_embed_literal_arg(key)?;
    let folded = embed(&text)?;
    *key = Expr::Value(SqlValue::SingleQuotedString(vector_to_pg_text(&folded)).with_empty_span());
    Some(q.to_string())
}

/// The mutable QUERY operand of `q`'s single `ORDER BY column <op> query` vector-distance
/// key — the one expression [`fold_const_embed_order_key`] may rewrite. `None` for any
/// other `ORDER BY` shape, mirroring exactly the shape [`plan_ann_search`] recognises.
fn vector_order_key_query_operand(q: &mut Query) -> Option<&mut Expr> {
    let OrderByKind::Expressions(order_by) = &mut q.order_by.as_mut()?.kind else {
        return None;
    };
    let [ob] = order_by.as_mut_slice() else {
        return None;
    };
    let Expr::BinaryOp { op, right, .. } = &mut ob.expr else {
        return None;
    };
    vector_metric_for_op(op)?;
    Some(right.as_mut())
}

/// The literal text of an `medulla_embed('…')` call — i.e. the one argument shape that is a
/// COMPILE-TIME constant. `None` for anything else (a different function, an arity other
/// than 1, a bind placeholder, a column reference, a nested expression).
fn const_embed_literal_arg(expr: &Expr) -> Option<String> {
    let Expr::Function(func) = expr else {
        return None;
    };
    let name = func.name.0.last()?.as_ident()?;
    let expected = MEDULLA_EMBED_FN;
    if !name.value.eq_ignore_ascii_case(expected) {
        return None;
    }
    let FunctionArguments::List(list) = &func.args else {
        return None;
    };
    let [arg] = list.args.as_slice() else {
        return None;
    };
    arg_as_string(arg)
}

/// Format a dense vector as the pgvector text literal (`[1,2,3]`) that
/// `ann::parse_query_vector` decodes. A non-finite element would render as `inf`/`NaN`,
/// which that decoder rejects — so such a fold degrades to "no pushdown", never to a
/// wrong query vector.
fn vector_to_pg_text(v: &[f32]) -> String {
    let mut out = String::with_capacity(v.len() * 8 + 2);
    out.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&x.to_string());
    }
    out.push(']');
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// MEDULLA-119 — ParadeDB BM25 search planner
// ─────────────────────────────────────────────────────────────────────────────

/// Recognize a ParadeDB BM25 search (CONCEPT:MEDULLA-KG.query.paradedb-bm25): `… FROM table WHERE column @@@
/// 'query' [LIMIT k]`. Returns the [`Bm25SearchPlan`] the server lowers onto `medulla-text`.
/// `@@@` tokenizes in `sqlparser` as `AtAt` with a `PGAbs`-wrapped right operand.
pub fn plan_bm25_search(sql: &str) -> Option<Bm25SearchPlan> {
    let stmts = Parser::parse_sql(&PostgreSqlDialect {}, sql).ok()?;
    let [Statement::Query(q)] = stmts.as_slice() else {
        return None;
    };
    let SetExpr::Select(select) = q.body.as_ref() else {
        return None;
    };
    let [twj] = select.from.as_slice() else {
        return None;
    };
    let TableFactor::Table { name, .. } = &twj.relation else {
        return None;
    };
    let table = name.0.last()?.as_ident()?.value.clone();
    let (column, query) = find_bm25_match(select.selection.as_ref()?)?;
    let limit = match q.limit_clause.as_ref() {
        Some(LimitClause::LimitOffset {
            limit: Some(limit), ..
        })
        | Some(LimitClause::OffsetCommaLimit { limit, .. }) => Some(limit),
        _ => None,
    };
    let k = match limit {
        Some(Expr::Value(value)) => match &value.value {
            SqlValue::Number(n, _) => n.parse::<usize>().ok(),
            _ => None,
        },
        _ => None,
    };
    Some(Bm25SearchPlan {
        table,
        column,
        query,
        k,
    })
}

/// Find a `column @@@ 'query'` match anywhere in a WHERE predicate (CONCEPT:MEDULLA-KG.query.paradedb-bm25),
/// descending through `AND`/`OR`. Returns `(column, query_text)`.
fn find_bm25_match(expr: &Expr) -> Option<(String, String)> {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::AtAt,
            right,
        } => {
            // `@@@` ⇒ right is a `@`(PGAbs)-wrapped string literal.
            let inner = match right.as_ref() {
                Expr::UnaryOp {
                    op: UnaryOperator::PGAbs,
                    expr,
                } => expr.as_ref(),
                other => other,
            };
            let column = match left.as_ref() {
                Expr::Identifier(id) => id.value.clone(),
                Expr::CompoundIdentifier(ids) => ids.last()?.value.clone(),
                _ => return None,
            };
            let query = match inner {
                Expr::Value(value) => match &value.value {
                    SqlValue::SingleQuotedString(s) => s.clone(),
                    _ => inner.to_string(),
                },
                other => other.to_string(),
            };
            Some((column, query))
        }
        Expr::BinaryOp { left, right, .. } => {
            find_bm25_match(left).or_else(|| find_bm25_match(right))
        }
        Expr::Nested(inner) => find_bm25_match(inner),
        _ => None,
    }
}
