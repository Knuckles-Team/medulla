//! `ORDER BY NEAREST(column, 'text')`: the recognized form of a column vector read and the
//! scalar function the executor ranks with (MEDULLA-FEDERATED-QUERY-R077.5).
//!
//! The text is embedded and probed by the served caller, never here: this module only parses
//! the statement and ranks the rows the authorized projection kept. A `NEAREST` statement
//! that reaches the executor with no ranking refuses `METHOD_NOT_YET_SERVED`.

use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Float64Array, RecordBatch, StringArray};
use arrow::datatypes::DataType;
use datafusion::error::{DataFusionError, Result as DfResult};
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use datafusion::sql::sqlparser::ast::{
    Expr, FunctionArg, FunctionArgExpr, FunctionArguments, LimitClause, OrderByKind, SetExpr,
    Statement, TableFactor, Value as SqlValue,
};
use datafusion::sql::sqlparser::dialect::PostgreSqlDialect;
use datafusion::sql::sqlparser::parser::Parser;
use medulla_table_store::tables::nearest::NearestRanking;

use super::read_only::explained_statement;

/// The SQL name of the ranking function.
pub const NEAREST_FN: &str = "nearest";

/// The recognized shape of a column vector read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearestForm {
    /// The single `FROM` table, as the statement spells it.
    pub table: String,
    /// The bare column of that table, as the statement spells it.
    pub column: String,
    /// The query text, the second argument.
    pub text: String,
    /// `k + o`: the nearest rows the probe must return.
    pub fetch: usize,
    /// Whether the statement runs under `EXPLAIN`.
    pub explain: bool,
}

fn invalid(reason: &str) -> String {
    format!("INVALID_ARGUMENT: {reason}")
}

/// How many times `sql` calls `NEAREST(`, ignoring quoted text and quoted identifiers.
pub fn nearest_calls(sql: &str) -> usize {
    let bytes: Vec<char> = sql.chars().collect();
    let mut count = 0;
    let mut quote: Option<char> = None;
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index];
        if let Some(open) = quote {
            if c == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            index += 1;
            continue;
        }
        let word_start = c.is_alphabetic() || c == '_';
        if word_start && (index == 0 || !(bytes[index - 1].is_alphanumeric() || bytes[index - 1] == '_'))
        {
            let end = bytes[index..]
                .iter()
                .position(|ch| !(ch.is_alphanumeric() || *ch == '_'))
                .map_or(bytes.len(), |offset| index + offset);
            let word: String = bytes[index..end].iter().collect();
            if word.eq_ignore_ascii_case(NEAREST_FN) {
                let next = bytes[end..].iter().find(|ch| !ch.is_whitespace());
                if next == Some(&'(') {
                    count += 1;
                }
            }
            index = end;
            continue;
        }
        index += 1;
    }
    count
}

fn count_literal(expr: &Expr, what: &str) -> Result<usize, String> {
    match expr {
        Expr::Value(value) => match &value.value {
            SqlValue::Number(number, _) => number
                .parse::<usize>()
                .map_err(|_| invalid(&format!("{what} must be a non-negative integer"))),
            _ => Err(invalid(&format!("{what} must be an integer literal"))),
        },
        _ => Err(invalid(&format!("{what} must be an integer literal"))),
    }
}

fn function_args(expr: &Expr) -> Option<Vec<&Expr>> {
    let Expr::Function(function) = expr else {
        return None;
    };
    let name = function.name.0.last()?.as_ident()?;
    if !name.value.eq_ignore_ascii_case(NEAREST_FN) {
        return None;
    }
    let FunctionArguments::List(list) = &function.args else {
        return None;
    };
    list.args
        .iter()
        .map(|arg| match arg {
            FunctionArg::Unnamed(FunctionArgExpr::Expr(expr)) => Some(expr),
            _ => None,
        })
        .collect()
}

/// Recognize `SELECT ... FROM t [WHERE ...] ORDER BY NEAREST(col, 'text') [, ...] LIMIT k
/// [OFFSET o]`, optionally under `EXPLAIN`. A statement with no `NEAREST` call is `Ok(None)`.
/// Two calls, a call anywhere but the first `ORDER BY` key, a first argument that is not a
/// bare column of the single `FROM` table, a second argument that is not a string literal, a
/// join or subquery in `FROM`, and no `LIMIT` refuse `INVALID_ARGUMENT`.
pub fn nearest_form(sql: &str) -> Result<Option<NearestForm>, String> {
    let (inner, explain) = match explained_statement(sql) {
        Some(inner) => (inner, true),
        None => (sql.to_string(), false),
    };
    match nearest_calls(&inner) {
        0 => return Ok(None),
        1 => {}
        _ => return Err(invalid("NEAREST may be called once")),
    }
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, &inner)
        .map_err(|error| invalid(&format!("NEAREST statement does not parse: {error}")))?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return Err(invalid("NEAREST needs exactly one SELECT statement"));
    };
    let SetExpr::Select(select) = query.body.as_ref() else {
        return Err(invalid("NEAREST needs a plain SELECT"));
    };
    let [from] = select.from.as_slice() else {
        return Err(invalid("NEAREST needs a single FROM table"));
    };
    if !from.joins.is_empty() {
        return Err(invalid("NEAREST does not accept a join"));
    }
    let TableFactor::Table { name, alias, .. } = &from.relation else {
        return Err(invalid("NEAREST needs a table in FROM, not a subquery"));
    };
    let table = name
        .0
        .last()
        .and_then(|part| part.as_ident())
        .map(|ident| ident.value.clone())
        .ok_or_else(|| invalid("NEAREST needs a named FROM table"))?;
    let alias = alias.as_ref().map(|alias| alias.name.value.clone());

    let Some(order_by) = query.order_by.as_ref() else {
        return Err(invalid("NEAREST must be the first ORDER BY key"));
    };
    let OrderByKind::Expressions(keys) = &order_by.kind else {
        return Err(invalid("NEAREST must be the first ORDER BY key"));
    };
    let Some(arguments) = keys.first().and_then(|key| function_args(&key.expr)) else {
        return Err(invalid("NEAREST must be the first ORDER BY key"));
    };
    let [column, text] = arguments.as_slice() else {
        return Err(invalid("NEAREST takes a column and a text"));
    };
    let column = match column {
        Expr::Identifier(ident) => ident.value.clone(),
        Expr::CompoundIdentifier(parts) => match parts.as_slice() {
            [qualifier, column]
                if qualifier.value.eq_ignore_ascii_case(&table)
                    || alias
                        .as_ref()
                        .is_some_and(|alias| qualifier.value.eq_ignore_ascii_case(alias)) =>
            {
                column.value.clone()
            }
            _ => return Err(invalid("NEAREST needs a bare column of the FROM table")),
        },
        _ => return Err(invalid("NEAREST needs a bare column of the FROM table")),
    };
    let text = match text {
        Expr::Value(value) => match &value.value {
            SqlValue::SingleQuotedString(text) => text.clone(),
            _ => return Err(invalid("NEAREST needs a string literal text")),
        },
        _ => return Err(invalid("NEAREST needs a string literal text")),
    };

    let fetch = match query.limit_clause.as_ref() {
        Some(LimitClause::LimitOffset {
            limit: Some(limit),
            offset,
            ..
        }) => {
            let k = count_literal(limit, "LIMIT")?;
            let o = match offset {
                Some(offset) => count_literal(&offset.value, "OFFSET")?,
                None => 0,
            };
            k.checked_add(o)
        }
        Some(LimitClause::OffsetCommaLimit { offset, limit }) => {
            count_literal(limit, "LIMIT")?.checked_add(count_literal(offset, "OFFSET")?)
        }
        _ => return Err(invalid("NEAREST needs a LIMIT")),
    }
    .ok_or_else(|| invalid("LIMIT plus OFFSET is too large"))?;
    Ok(Some(NearestForm {
        table,
        column,
        text,
        fetch,
        explain,
    }))
}

/// `nearest(column, text) -> Float64`: the hit distance of the column text the projection
/// kept, `NULL` for a text that is not in the ranking.
pub fn nearest_udf(ranking: Arc<NearestRanking>) -> ScalarUDF {
    ScalarUDF::new_from_impl(NearestUdf {
        signature: Signature::any(2, Volatility::Immutable),
        ranking,
    })
}

struct NearestUdf {
    signature: Signature,
    ranking: Arc<NearestRanking>,
}

impl std::fmt::Debug for NearestUdf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NearestUdf").field("name", &NEAREST_FN).finish()
    }
}

impl PartialEq for NearestUdf {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature && Arc::ptr_eq(&self.ranking, &other.ranking)
    }
}

impl Eq for NearestUdf {}

impl std::hash::Hash for NearestUdf {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&NEAREST_FN, state);
        std::hash::Hash::hash(&self.signature, state);
    }
}

impl ScalarUDFImpl for NearestUdf {
    fn name(&self) -> &str {
        NEAREST_FN
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> DfResult<DataType> {
        Ok(DataType::Float64)
    }

    fn invoke_with_args(&self, call: ScalarFunctionArgs) -> DfResult<ColumnarValue> {
        let Some(first) = call.args.first() else {
            return Err(DataFusionError::Execution(format!(
                "{NEAREST_FN} takes a column and a text"
            )));
        };
        let array = first.to_array(call.number_rows)?;
        let utf8 = arrow::compute::cast(&array, &DataType::Utf8).map_err(|error| {
            DataFusionError::Execution(format!("{NEAREST_FN}: the column must be text ({error})"))
        })?;
        let texts = utf8.as_any().downcast_ref::<StringArray>().ok_or_else(|| {
            DataFusionError::Execution(format!("{NEAREST_FN}: text decode failed"))
        })?;
        let distances: Float64Array = (0..texts.len())
            .map(|row| {
                if texts.is_null(row) {
                    None
                } else {
                    self.ranking.distances.get(texts.value(row)).copied()
                }
            })
            .collect();
        Ok(ColumnarValue::Array(Arc::new(distances) as ArrayRef))
    }
}

/// Append the `vector_scan` row to the plan relation an `EXPLAIN` produced: one row,
/// `plan_type = 'vector_scan'` and the ranking's plan line.
pub fn append_vector_scan(
    batches: &mut Vec<RecordBatch>,
    ranking: &NearestRanking,
) -> Result<(), String> {
    let Some(first) = batches.first() else {
        return Ok(());
    };
    let schema = first.schema();
    let values = ["vector_scan", ranking.explain.as_str()];
    if schema.fields().len() != values.len() {
        return Err("EXPLAIN did not produce a plan_type and plan relation".to_string());
    }
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(values.len());
    for (field, value) in schema.fields().iter().zip(values) {
        let text: ArrayRef = Arc::new(StringArray::from(vec![value]));
        columns.push(
            arrow::compute::cast(&text, field.data_type())
                .map_err(|error| format!("vector_scan plan row: {error}"))?,
        );
    }
    let row = RecordBatch::try_new(schema, columns)
        .map_err(|error| format!("vector_scan plan row: {error}"))?;
    batches.push(row);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_table_store::tables::TableStore;
    use medulla_tables::schema::{Column, ColumnType, TableSchema};
    use serde_json::json;

    fn store_with_docs() -> TableStore {
        let (store, _path) = TableStore::open_temp().unwrap();
        let schema = TableSchema::new(
            "docs",
            vec![
                Column::new("id", ColumnType::Text, false, true),
                Column::new("body", ColumnType::Text, true, false),
            ],
        );
        store.create_table(&schema, true).unwrap();
        let columns = ["id".to_string(), "body".to_string()];
        let rows: Vec<Vec<serde_json::Value>> = ["alpha", "beta", "gamma"]
            .iter()
            .enumerate()
            .map(|(index, text)| vec![json!(format!("d{index}")), json!(text)])
            .collect();
        store.insert_rows("docs", &columns, &rows).unwrap();
        store
    }

    // spec: MEDULLA-FEDERATED-QUERY-R077.5
    #[test]
    fn the_ranking_orders_rows_nearest_first_and_explain_names_the_scan() {
        let view = medulla_graph_core::graph::GraphView::default();
        let store = store_with_docs();
        let sql = "SELECT id FROM docs ORDER BY NEAREST(body, 'q') LIMIT 2";
        // No ranking: the statement is not served here.
        let refused = crate::sql::exec_sql_typed_with_tables(&view, &store, sql).unwrap_err();
        assert!(refused.starts_with("METHOD_NOT_YET_SERVED"), "{refused}");

        let mut ranking = NearestRanking {
            explain: "VectorScan binding=binding:docs.body space=m generation=3".into(),
            ..NearestRanking::default()
        };
        ranking.record("alpha".into(), 0.9);
        ranking.record("beta".into(), 0.1);
        ranking.record("gamma".into(), 0.5);
        let ranked = store.with_nearest(Arc::new(ranking));
        let typed = crate::sql::exec_sql_typed_with_tables(&view, &ranked, sql).unwrap();
        let ids: Vec<String> = typed
            .rows
            .iter()
            .map(|row| row[0].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(ids, ["d1", "d2"]);

        let explained = crate::sql::exec_sql_typed_with_tables(
            &view,
            &ranked,
            &format!("EXPLAIN {sql}"),
        )
        .unwrap();
        assert!(
            explained.rows.iter().any(|row| {
                row[0].as_str() == Some("vector_scan")
                    && row[1].as_str().is_some_and(|plan| plan.contains("binding=binding:docs.body"))
            }),
            "{:?}",
            explained.rows
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R077.5
    #[test]
    fn the_form_is_recognized_and_malformed_forms_refuse() {
        let form = nearest_form(
            "SELECT id, body FROM docs WHERE id > 1 ORDER BY NEAREST(body, 'leaky pump') LIMIT 3 OFFSET 2",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            form,
            NearestForm {
                table: "docs".into(),
                column: "body".into(),
                text: "leaky pump".into(),
                fetch: 5,
                explain: false,
            }
        );
        let explained = nearest_form("EXPLAIN SELECT body FROM docs ORDER BY nearest(body, 'x') LIMIT 1")
            .unwrap()
            .unwrap();
        assert!(explained.explain);
        assert_eq!(nearest_form("SELECT * FROM docs LIMIT 2").unwrap(), None);
        assert_eq!(
            nearest_form("SELECT * FROM docs WHERE body = 'nearest(x)' LIMIT 2").unwrap(),
            None
        );
        for refused in [
            "SELECT body FROM docs ORDER BY NEAREST(body, 'a'), NEAREST(body, 'b') LIMIT 1",
            "SELECT body FROM docs ORDER BY id, NEAREST(body, 'a') LIMIT 1",
            "SELECT body FROM docs WHERE NEAREST(body, 'a') < 1 ORDER BY id LIMIT 1",
            "SELECT body FROM docs ORDER BY NEAREST(lower(body), 'a') LIMIT 1",
            "SELECT body FROM docs ORDER BY NEAREST(body, other) LIMIT 1",
            "SELECT a.body FROM docs a JOIN more b ON a.id = b.id ORDER BY NEAREST(a.body, 'a') LIMIT 1",
            "SELECT body FROM (SELECT body FROM docs) s ORDER BY NEAREST(body, 'a') LIMIT 1",
            "SELECT body FROM docs ORDER BY NEAREST(body, 'a')",
        ] {
            let error = nearest_form(refused).unwrap_err();
            assert!(error.starts_with("INVALID_ARGUMENT"), "{refused}: {error}");
        }
    }
}
