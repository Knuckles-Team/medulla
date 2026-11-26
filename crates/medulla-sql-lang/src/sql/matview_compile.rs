//! The incremental-view compiler (MEDULLA-INCREMENTAL-VIEWS-R009.2).
//!
//! [`compile_incremental`] turns the defining query of a materialized view into an
//! [`IncrementalViewProgram`] when the view is one of the incremental forms (a filter and a
//! scalar projection over one table, or a grouped `count`, `sum` or `avg` over exact numbers),
//! and otherwise names the first construct that keeps it a recompute view
//! ([`NotIncremental`], code `MATVIEW_NOT_INCREMENTAL`).
//!
//! The refusals are checked in this order, and the first one found is the one named:
//! (1) a second table, a join or a subquery; (2) a window function, `DISTINCT`, `ORDER BY`,
//! `LIMIT` or `HAVING` (plain `DISTINCT` is maintained); (3) a volatile function; (5) a column or a literal
//! the exact arithmetic cannot hold (a float column, or `sum` / `avg` over one). Then every
//! remaining shape the program has no node for is refused by name.
//!
//! Typing: the caller feeds the declared type of each source column. A `Numeric` column
//! compiles to `RowExpr::Decimal(Column)` wherever it appears, so sums and averages never pass
//! through `f64`; `Int`, `BigInt`, `Text`, `Bool` and `Timestamp` columns compile to
//! `RowExpr::Column`.
//!
//! Output shape: a `Rows` view projects scalar expressions; a view with an aggregate or a
//! `GROUP BY` is a `Groups` view whose projection lists the group keys (each a `GROUP BY`
//! expression, every `GROUP BY` expression exactly once) and then the aggregates. A
//! `SELECT *` has no column list here and is refused.

use datafusion::sql::sqlparser::ast::{
    BinaryOperator, Distinct, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, GroupByExpr, Query,
    SelectItem, SetExpr, TableFactor, UnaryOperator, Value as SqlValue,
};
use datafusion::sql::sqlparser::dialect::PostgreSqlDialect;
use datafusion::sql::sqlparser::parser::Parser;
use medulla_table_store::tables::matview_incremental::{
    ExactAgg, IncrementalViewProgram, RowExpr, ViewOutput,
};
use medulla_tables::schema::ColumnType;
use medulla_types::contract::ServerErrorCode;
use serde_json::Value;

/// The first construct of a view's defining query that keeps it from being maintained
/// incrementally. Maps to `MATVIEW_NOT_INCREMENTAL`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotIncremental {
    /// What the query contains, such as `join` or `window function`.
    pub construct: String,
}

impl NotIncremental {
    fn new(construct: impl Into<String>) -> Self {
        Self {
            construct: construct.into(),
        }
    }

    /// The typed refusal code.
    pub fn code(&self) -> ServerErrorCode {
        ServerErrorCode::MatviewNotIncremental
    }
}

impl std::fmt::Display for NotIncremental {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code().as_str(), self.construct)
    }
}

type Refusal = NotIncremental;

/// Function names whose value changes between calls or between rows.
const VOLATILE_FUNCTIONS: &[&str] = &[
    "now",
    "random",
    "random_normal",
    "setseed",
    "clock_timestamp",
    "statement_timestamp",
    "transaction_timestamp",
    "timeofday",
    "current_timestamp",
    "current_date",
    "current_time",
    "localtime",
    "localtimestamp",
    "gen_random_uuid",
    "uuid_generate_v4",
    "nextval",
    "currval",
    "setval",
];

/// Parse `select_sql` and compile it. A text that is not one plain query is refused as
/// `statement`.
pub fn compile_incremental_sql(
    name: &str,
    select_sql: &str,
    column_type: &dyn Fn(&str, &str) -> Option<ColumnType>,
) -> Result<IncrementalViewProgram, NotIncremental> {
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, select_sql)
        .map_err(|error| NotIncremental::new(format!("unparsable query: {error}")))?;
    let [datafusion::sql::sqlparser::ast::Statement::Query(query)] = statements.as_slice() else {
        return Err(NotIncremental::new("statement that is not one query"));
    };
    compile_incremental(name, query, column_type)
}

/// Compile the defining `query` of the view `name`. `column_type(table, column)` answers the
/// declared type of a column of the one source table, `None` for an unknown column.
pub fn compile_incremental(
    name: &str,
    query: &Query,
    column_type: &dyn Fn(&str, &str) -> Option<ColumnType>,
) -> Result<IncrementalViewProgram, NotIncremental> {
    // (1) A second table, a join or a subquery.
    if query.with.is_some() {
        return Err(Refusal::new("subquery"));
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return Err(Refusal::new(match query.body.as_ref() {
            SetExpr::SetOperation { .. } => "set operation",
            SetExpr::Values(_) => "VALUES",
            _ => "subquery",
        }));
    };
    let table = source_table(select)?;
    let exprs = all_exprs(select);
    for expr in &exprs {
        if find(expr, &|e| {
            matches!(
                e,
                Expr::Subquery(_) | Expr::Exists { .. } | Expr::InSubquery { .. }
            )
        })
        .is_some()
        {
            return Err(Refusal::new("subquery"));
        }
    }
    // (2) A window function, DISTINCT, ORDER BY, LIMIT or HAVING.
    for expr in &exprs {
        if find(expr, &|e| matches!(e, Expr::Function(f) if f.over.is_some())).is_some() {
            return Err(Refusal::new("window function"));
        }
    }
    // Plain `DISTINCT` is maintained (R012.3); `DISTINCT ON` is not.
    if matches!(&select.distinct, Some(distinct) if !matches!(distinct, Distinct::Distinct)) {
        return Err(Refusal::new("DISTINCT ON"));
    }
    if query.order_by.is_some() {
        return Err(Refusal::new("ORDER BY"));
    }
    if query.limit_clause.is_some() || query.fetch.is_some() {
        return Err(Refusal::new("LIMIT"));
    }
    if select.having.is_some() {
        return Err(Refusal::new("HAVING"));
    }
    // (3) A volatile function.
    for expr in &exprs {
        if let Some(Expr::Function(f)) = find(expr, &|e| {
            matches!(e, Expr::Function(f) if VOLATILE_FUNCTIONS.contains(&function_name(f).as_str()))
        }) {
            return Err(Refusal::new(format!("volatile function {}", function_name(f))));
        }
    }
    // (5) The shapes the program has no node for, and float columns.
    let ctx = Ctx {
        table: &table,
        column_type,
    };
    let filter = select
        .selection
        .as_ref()
        .map(|expr| ctx.expr(expr))
        .transpose()?;
    let mut output = ctx.output(select)?;
    if select.distinct.is_some() {
        output = match output {
            ViewOutput::Rows(columns) => ViewOutput::Distinct(columns),
            _ => return Err(Refusal::new("DISTINCT with an aggregate or GROUP BY")),
        };
    }
    Ok(IncrementalViewProgram {
        view: name.to_string(),
        table,
        join: None,
        filter,
        output,
    })
}

/// The one source table: a single bare table, no join.
fn source_table(
    select: &datafusion::sql::sqlparser::ast::Select,
) -> Result<String, Refusal> {
    let [from] = select.from.as_slice() else {
        return Err(Refusal::new(if select.from.is_empty() {
            "query without a table"
        } else {
            "second table"
        }));
    };
    if !from.joins.is_empty() {
        return Err(Refusal::new("join"));
    }
    match &from.relation {
        TableFactor::Table { name, .. } => name
            .0
            .last()
            .and_then(|part| part.as_ident())
            .map(|ident| ident.value.clone())
            .ok_or_else(|| Refusal::new("table name")),
        TableFactor::Derived { .. } => Err(Refusal::new("subquery")),
        _ => Err(Refusal::new("table function")),
    }
}

/// Every expression of the select that a pass scans, in walk order: filter, group by,
/// projection, having.
fn all_exprs(select: &datafusion::sql::sqlparser::ast::Select) -> Vec<&Expr> {
    let mut out = Vec::new();
    out.extend(select.selection.iter());
    if let GroupByExpr::Expressions(keys, _) = &select.group_by {
        out.extend(keys.iter());
    }
    for item in &select.projection {
        match item {
            SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => out.push(expr),
            _ => {}
        }
    }
    out.extend(select.having.iter());
    out
}

fn function_name(function: &datafusion::sql::sqlparser::ast::Function) -> String {
    function
        .name
        .0
        .last()
        .and_then(|part| part.as_ident())
        .map(|ident| ident.value.to_ascii_lowercase())
        .unwrap_or_default()
}

/// The arguments of a plain call: `None` when the call has clauses, a subquery argument or
/// named arguments.
fn call_args(
    function: &datafusion::sql::sqlparser::ast::Function,
) -> Option<Vec<&FunctionArgExpr>> {
    match &function.args {
        FunctionArguments::None => Some(Vec::new()),
        FunctionArguments::List(list) if list.clauses.is_empty() => list
            .args
            .iter()
            .map(|arg| match arg {
                FunctionArg::Unnamed(inner) => Some(inner),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// The first subexpression of `expr` (itself included) that `hit` accepts.
fn find<'a>(expr: &'a Expr, hit: &dyn Fn(&Expr) -> bool) -> Option<&'a Expr> {
    if hit(expr) {
        return Some(expr);
    }
    let children: Vec<&Expr> = match expr {
        Expr::BinaryOp { left, right, .. } => vec![left.as_ref(), right.as_ref()],
        Expr::UnaryOp { expr, .. }
        | Expr::Nested(expr)
        | Expr::IsNull(expr)
        | Expr::IsNotNull(expr)
        | Expr::Cast { expr, .. } => vec![expr.as_ref()],
        Expr::InList { expr, list, .. } => {
            std::iter::once(expr.as_ref()).chain(list.iter()).collect()
        }
        Expr::Between {
            expr, low, high, ..
        } => vec![expr.as_ref(), low.as_ref(), high.as_ref()],
        Expr::InSubquery { expr, .. } => vec![expr.as_ref()],
        Expr::Function(function) => match &function.args {
            FunctionArguments::List(list) => list
                .args
                .iter()
                .filter_map(|arg| match arg {
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(inner))
                    | FunctionArg::Named {
                        arg: FunctionArgExpr::Expr(inner),
                        ..
                    } => Some(inner),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    children.into_iter().find_map(|child| find(child, hit))
}

struct Ctx<'a> {
    table: &'a str,
    column_type: &'a dyn Fn(&str, &str) -> Option<ColumnType>,
}

fn boxed(expr: RowExpr) -> Box<RowExpr> {
    Box::new(expr)
}

impl Ctx<'_> {
    /// A source column as a program expression, typed by its declared type.
    fn column(&self, column: &str, aggregate: Option<&str>) -> Result<RowExpr, Refusal> {
        let Some(ty) = (self.column_type)(self.table, column) else {
            return Err(Refusal::new(format!(
                "unknown column {}.{column}",
                self.table
            )));
        };
        match ty {
            ColumnType::Numeric(_) => Ok(RowExpr::Decimal(boxed(RowExpr::Column(
                column.to_string(),
            )))),
            ColumnType::Int
            | ColumnType::BigInt
            | ColumnType::Text
            | ColumnType::Bool
            | ColumnType::Timestamp => Ok(RowExpr::Column(column.to_string())),
            ColumnType::Float | ColumnType::Double => Err(Refusal::new(match aggregate {
                Some(function) => format!("{function} over a float column"),
                None => format!("float column {}.{column}", self.table),
            })),
            _ => Err(Refusal::new(format!(
                "column {}.{column} of a type the program cannot hold",
                self.table
            ))),
        }
    }

    fn expr(&self, expr: &Expr) -> Result<RowExpr, Refusal> {
        self.expr_in(expr, None)
    }

    /// `aggregate` names the aggregate whose argument this is, for the float refusal.
    fn expr_in(&self, expr: &Expr, aggregate: Option<&str>) -> Result<RowExpr, Refusal> {
        let sub = |inner: &Expr| self.expr_in(inner, aggregate);
        match expr {
            Expr::Identifier(ident) => self.column(&ident.value, aggregate),
            Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                [table, column] if table.value == self.table => {
                    self.column(&column.value, aggregate)
                }
                [table, column] => Err(Refusal::new(format!(
                    "unknown column {}.{}",
                    table.value, column.value
                ))),
                _ => Err(Refusal::new(format!("expression {expr}"))),
            },
            Expr::Value(value) => literal(&value.value),
            Expr::Nested(inner) => sub(inner),
            Expr::UnaryOp { op, expr: inner } => match (op, inner.as_ref()) {
                (UnaryOperator::Not, inner) => Ok(RowExpr::Not(boxed(sub(inner)?))),
                (UnaryOperator::Plus, inner) => sub(inner),
                (UnaryOperator::Minus, Expr::Value(value)) => negative(&value.value),
                (UnaryOperator::Minus, inner) => Ok(RowExpr::Sub(
                    boxed(RowExpr::Literal(Value::from(0))),
                    boxed(sub(inner)?),
                )),
                _ => Err(Refusal::new(format!("operator {op}"))),
            },
            Expr::IsNull(inner) => Ok(RowExpr::IsNull(boxed(sub(inner)?))),
            Expr::IsNotNull(inner) => Ok(RowExpr::IsNotNull(boxed(sub(inner)?))),
            Expr::BinaryOp { left, op, right } => {
                let (l, r) = (boxed(sub(left)?), boxed(sub(right)?));
                Ok(match op {
                    BinaryOperator::Plus => RowExpr::Add(l, r),
                    BinaryOperator::Minus => RowExpr::Sub(l, r),
                    BinaryOperator::Multiply => RowExpr::Mul(l, r),
                    BinaryOperator::Eq => RowExpr::Eq(l, r),
                    BinaryOperator::NotEq => RowExpr::Ne(l, r),
                    BinaryOperator::Lt => RowExpr::Lt(l, r),
                    BinaryOperator::LtEq => RowExpr::Le(l, r),
                    BinaryOperator::Gt => RowExpr::Gt(l, r),
                    BinaryOperator::GtEq => RowExpr::Ge(l, r),
                    BinaryOperator::And => RowExpr::And(l, r),
                    BinaryOperator::Or => RowExpr::Or(l, r),
                    other => return Err(Refusal::new(format!("operator {other}"))),
                })
            }
            Expr::Function(function) => Err(Refusal::new(format!(
                "function {}",
                function_name(function)
            ))),
            other => Err(Refusal::new(format!("expression {other}"))),
        }
    }

    /// The view's output: `Rows`, or `Groups` when there is an aggregate or a `GROUP BY`.
    fn output(&self, select: &datafusion::sql::sqlparser::ast::Select) -> Result<ViewOutput, Refusal> {
        let group_keys: &[Expr] = match &select.group_by {
            GroupByExpr::Expressions(keys, modifiers) if modifiers.is_empty() => keys,
            GroupByExpr::Expressions(..) | GroupByExpr::All(_) => {
                return Err(Refusal::new("GROUP BY modifier"));
            }
        };
        let mut items = Vec::new();
        for item in &select.projection {
            match item {
                SelectItem::UnnamedExpr(expr) => items.push((expr, None)),
                SelectItem::ExprWithAlias { expr, alias } => {
                    items.push((expr, Some(alias.value.clone())));
                }
                _ => return Err(Refusal::new("wildcard projection")),
            }
        }
        let is_aggregate = |expr: &Expr| {
            matches!(expr, Expr::Function(f) if matches!(function_name(f).as_str(), "count" | "sum" | "avg" | "min" | "max"))
        };
        let grouped = !group_keys.is_empty() || items.iter().any(|(expr, _)| is_aggregate(expr));
        if !grouped {
            let mut columns = Vec::new();
            for (expr, alias) in items {
                let name = alias.unwrap_or_else(|| output_name(expr));
                columns.push((name, self.expr(expr)?));
            }
            return Ok(ViewOutput::Rows(columns));
        }
        let mut keys = Vec::new();
        let mut aggs = Vec::new();
        for (expr, alias) in items {
            if let Expr::Function(function) = expr {
                if is_aggregate(expr) {
                    let name = alias.unwrap_or_else(|| function_name(function));
                    aggs.push((name, self.aggregate(function)?));
                    continue;
                }
            }
            if !aggs.is_empty() {
                return Err(Refusal::new("group key after an aggregate"));
            }
            if !group_keys.iter().any(|key| key.to_string() == expr.to_string()) {
                return Err(Refusal::new(format!(
                    "projection {expr} that is neither an aggregate nor a GROUP BY expression"
                )));
            }
            let name = alias.unwrap_or_else(|| output_name(expr));
            keys.push((name, self.expr(expr)?));
        }
        // Every GROUP BY expression is projected exactly once.
        for key in group_keys {
            let projected = select
                .projection
                .iter()
                .filter(|item| match item {
                    SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
                        expr.to_string() == key.to_string()
                    }
                    _ => false,
                })
                .count();
            if projected != 1 {
                return Err(Refusal::new(format!(
                    "GROUP BY expression {key} projected {projected} times"
                )));
            }
        }
        Ok(ViewOutput::Groups { keys, aggs })
    }

    /// The bare column `arg` names when it is a float or double column of the source table.
    fn float_column(&self, arg: &Expr) -> Option<RowExpr> {
        let name = match arg {
            Expr::Identifier(ident) => ident.value.clone(),
            Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                [table, column] if table.value == self.table => column.value.clone(),
                _ => return None,
            },
            _ => return None,
        };
        matches!(
            (self.column_type)(self.table, &name),
            Some(ColumnType::Float | ColumnType::Double)
        )
        .then(|| RowExpr::Column(name))
    }

    fn aggregate(
        &self,
        function: &datafusion::sql::sqlparser::ast::Function,
    ) -> Result<ExactAgg, Refusal> {
        let name = function_name(function);
        if function.filter.is_some() {
            return Err(Refusal::new(format!("{name} with a FILTER clause")));
        }
        if let FunctionArguments::List(list) = &function.args {
            if list.duplicate_treatment.is_some() {
                return Err(Refusal::new(format!("{name} with DISTINCT or ALL")));
            }
        }
        let Some(args) = call_args(function) else {
            return Err(Refusal::new(format!("{name} with clauses")));
        };
        match (name.as_str(), args.as_slice()) {
            ("count", [FunctionArgExpr::Wildcard]) => Ok(ExactAgg::Count),
            ("count", [FunctionArgExpr::Expr(Expr::Value(_))]) => Ok(ExactAgg::Count),
            ("count", _) => Err(Refusal::new("count over an expression")),
            ("min" | "max", [FunctionArgExpr::Expr(arg)]) => {
                // An extreme needs no subtraction, so a float column is accepted.
                let arg = match self.float_column(arg) {
                    Some(column) => column,
                    None => self.expr_in(arg, Some(name.as_str()))?,
                };
                Ok(if name == "min" {
                    ExactAgg::Min(arg)
                } else {
                    ExactAgg::Max(arg)
                })
            }
            ("sum" | "avg", [FunctionArgExpr::Expr(arg)]) => {
                let arg = self.expr_in(arg, Some(name.as_str()))?;
                Ok(if name == "sum" {
                    ExactAgg::Sum(arg)
                } else {
                    ExactAgg::Avg(arg)
                })
            }
            _ => Err(Refusal::new(format!("{name} with {} arguments", args.len()))),
        }
    }
}

/// The name a projected expression gets when it has no alias.
fn output_name(expr: &Expr) -> String {
    match expr {
        Expr::Identifier(ident) => ident.value.clone(),
        Expr::CompoundIdentifier(parts) => parts
            .last()
            .map(|part| part.value.clone())
            .unwrap_or_default(),
        Expr::Nested(inner) => output_name(inner),
        other => other.to_string(),
    }
}

/// A literal of the query as a program literal. A decimal literal is read as an exact
/// decimal, so it compares and adds with a numeric column.
fn literal(value: &SqlValue) -> Result<RowExpr, Refusal> {
    match value {
        SqlValue::Number(text, _) => number(text, false),
        SqlValue::SingleQuotedString(text) => Ok(RowExpr::Literal(Value::String(text.clone()))),
        SqlValue::Boolean(flag) => Ok(RowExpr::Literal(Value::Bool(*flag))),
        SqlValue::Null => Ok(RowExpr::Literal(Value::Null)),
        other => Err(Refusal::new(format!("literal {other}"))),
    }
}

fn negative(value: &SqlValue) -> Result<RowExpr, Refusal> {
    match value {
        SqlValue::Number(text, _) => number(text, true),
        other => Err(Refusal::new(format!("literal -{other}"))),
    }
}

fn number(text: &str, negate: bool) -> Result<RowExpr, Refusal> {
    if text.contains(['e', 'E']) {
        return Err(Refusal::new(format!("float literal {text}")));
    }
    let signed = if negate { format!("-{text}") } else { text.to_string() };
    if text.contains('.') {
        return Ok(RowExpr::Decimal(boxed(RowExpr::Literal(Value::String(signed)))));
    }
    signed
        .parse::<i64>()
        .map(|integer| RowExpr::Literal(Value::from(integer)))
        .map_err(|_| Refusal::new(format!("integer literal {text} out of range")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(table: &str, column: &str) -> Option<ColumnType> {
        if table != "orders" {
            return None;
        }
        Some(match column {
            "id" | "qty" => ColumnType::Int,
            "region" => ColumnType::Text,
            "amount" => ColumnType::Numeric(Some((10, 2))),
            "ratio" => ColumnType::Double,
            _ => return None,
        })
    }

    fn compile(sql: &str) -> Result<IncrementalViewProgram, String> {
        compile_incremental_sql("v", sql, &types).map_err(|refusal| refusal.construct)
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R009.2
    #[test]
    fn unsupported_constructs_name_the_first() {
        let refused = |sql: &str, construct: &str| {
            assert_eq!(compile(sql).unwrap_err(), construct, "{sql}");
        };
        refused("SELECT id FROM orders, other", "second table");
        refused(
            "SELECT o.id FROM orders o JOIN other p ON o.id = p.id",
            "join",
        );
        refused("SELECT id FROM (SELECT id FROM orders) t", "subquery");
        refused(
            "SELECT id FROM orders WHERE id IN (SELECT id FROM orders)",
            "subquery",
        );
        refused(
            "SELECT id, row_number() OVER (ORDER BY id) FROM orders",
            "window function",
        );
        refused("SELECT DISTINCT ON (id) id FROM orders", "DISTINCT ON");
        refused("SELECT id FROM orders ORDER BY id", "ORDER BY");
        refused("SELECT id FROM orders LIMIT 5", "LIMIT");
        refused(
            "SELECT region, count(*) FROM orders GROUP BY region HAVING count(*) > 1",
            "HAVING",
        );
        refused("SELECT id FROM orders WHERE now() > now()", "volatile function now");
        refused(
            "SELECT region, sum(ratio) FROM orders GROUP BY region",
            "sum over a float column",
        );
        refused("SELECT ratio FROM orders", "float column orders.ratio");
        refused("SELECT nope FROM orders", "unknown column orders.nope");
        refused("SELECT * FROM orders", "wildcard projection");
        // Two refusals in one query: the earlier category wins.
        refused(
            "SELECT DISTINCT id FROM orders, other ORDER BY id",
            "second table",
        );
        refused(
            "SELECT DISTINCT region, max(qty) FROM orders GROUP BY region",
            "DISTINCT with an aggregate or GROUP BY",
        );
        // The code and text of the refusal.
        let error = compile_incremental_sql("v", "SELECT id FROM orders LIMIT 1", &types).unwrap_err();
        assert_eq!(error.code(), ServerErrorCode::MatviewNotIncremental);
        assert_eq!(error.to_string(), "MATVIEW_NOT_INCREMENTAL: LIMIT");
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R009.2
    #[test]
    fn supported_shapes_compile_with_typed_columns() {
        let program = compile("SELECT id, amount * 2 AS twice FROM orders WHERE qty > 1 AND region = 'eu'").unwrap();
        assert_eq!(program.table, "orders");
        let Some(RowExpr::And(..)) = program.filter else {
            panic!("the filter is an AND: {:?}", program.filter);
        };
        let ViewOutput::Rows(columns) = &program.output else {
            panic!("a projection is a Rows view");
        };
        assert_eq!(columns[0].0, "id");
        assert_eq!(columns[0].1, RowExpr::Column("id".into()));
        assert_eq!(columns[1].0, "twice");
        assert_eq!(
            columns[1].1,
            RowExpr::Mul(
                boxed(RowExpr::Decimal(boxed(RowExpr::Column("amount".into())))),
                boxed(RowExpr::Literal(Value::from(2))),
            )
        );

        let grouped = compile(
            "SELECT region, count(*) AS n, sum(amount) AS total, avg(amount) AS mean FROM orders GROUP BY region",
        )
        .unwrap();
        let ViewOutput::Groups { keys, aggs } = &grouped.output else {
            panic!("an aggregate is a Groups view");
        };
        assert_eq!(keys, &[("region".to_string(), RowExpr::Column("region".into()))]);
        let amount = || RowExpr::Decimal(boxed(RowExpr::Column("amount".into())));
        assert_eq!(
            aggs,
            &[
                ("n".to_string(), ExactAgg::Count),
                ("total".to_string(), ExactAgg::Sum(amount())),
                ("mean".to_string(), ExactAgg::Avg(amount())),
            ]
        );
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R012.3
    #[test]
    fn distinct_min_max_compile() {
        let distinct = compile("SELECT DISTINCT region FROM orders").unwrap();
        assert_eq!(
            distinct.output,
            ViewOutput::Distinct(vec![("region".to_string(), RowExpr::Column("region".into()))])
        );
        let extremes = compile(
            "SELECT region, min(amount) AS lo, max(ratio) AS hi FROM orders GROUP BY region",
        )
        .unwrap();
        let ViewOutput::Groups { aggs, .. } = &extremes.output else {
            panic!("min and max are a Groups view");
        };
        assert_eq!(
            aggs,
            &[
                (
                    "lo".to_string(),
                    ExactAgg::Min(RowExpr::Decimal(boxed(RowExpr::Column("amount".into()))))
                ),
                ("hi".to_string(), ExactAgg::Max(RowExpr::Column("ratio".into()))),
            ]
        );
    }
}
