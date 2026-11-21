//! SQL text that crosses a federation boundary (CONCEPT:MEDULLA-KG.query.query-federation, EH-563).
//!
//! The ONE owner of dialect detection, identifier validation/quoting and literal rendering
//! for SQL that EG sends to an external database: the OBDA virtual-graph source
//! (`SELECT … WHERE …`) and the federation optimizer's pushed key lookups
//! (`… WHERE id IN (…)`). Identifiers are restricted to `[A-Za-z_][A-Za-z0-9_]*` (≤ 63
//! bytes), which is what makes quoting injection-safe; literals are single-quoted with the
//! dialect's escaping, and numeric literals are re-validated as finite numbers.

/// The SQL dialect an external source speaks, inferred from its DSN scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlDialect {
    /// `postgres://` / `postgresql://`.
    Postgres,
    /// `mysql://` / `mariadb://`.
    MySql,
    /// `clickhouse://`: a columnar analytic source. Identifiers quote with backticks, a
    /// literal is single-quoted with backslash escapes, and a rendered statement binds its
    /// values through `param_<name>` query parameters (MEDULLA-UNIFIED-DATA-PLANE-R017.2.2).
    ColumnarAnalytic,
}

impl SqlDialect {
    /// The dialect of a DSN (`postgres://…`, `mysql://…`), or `None` for any other scheme.
    pub fn from_dsn(dsn: &str) -> Option<Self> {
        match dsn.split(':').next().unwrap_or("") {
            "postgres" | "postgresql" => Some(Self::Postgres),
            "mysql" | "mariadb" => Some(Self::MySql),
            "clickhouse" => Some(Self::ColumnarAnalytic),
            _ => None,
        }
    }
}

/// Validate a SQL identifier: `[A-Za-z_][A-Za-z0-9_]*`, at most 63 bytes. Rejecting
/// anything else is what makes [`quote_identifier`] injection-safe.
pub fn validate_identifier(ident: &str) -> Result<(), String> {
    let ok = !ident.is_empty()
        && ident.len() <= 63
        && ident
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(format!("invalid SQL identifier {ident:?}"))
    }
}

/// Validate, then quote an identifier for `dialect`.
pub fn quote_identifier(ident: &str, dialect: SqlDialect) -> Result<String, String> {
    validate_identifier(ident)?;
    Ok(match dialect {
        SqlDialect::Postgres => format!("\"{ident}\""),
        SqlDialect::MySql | SqlDialect::ColumnarAnalytic => format!("`{ident}`"),
    })
}

/// Render a comparison literal: a validated finite numeric token unquoted when `numeric`,
/// otherwise a single-quoted string with the dialect's escaping (Postgres doubles `'`;
/// MySQL, which also treats `\` as an escape, doubles both; the columnar analytic dialect
/// backslash-escapes `\` and `'`). A NUL byte is refused.
pub fn render_literal(value: &str, numeric: bool, dialect: SqlDialect) -> Result<String, String> {
    if value.contains('\0') {
        return Err("SQL literal contains a NUL byte".into());
    }
    if numeric {
        return render_numeric(value);
    }
    let escaped = match dialect {
        SqlDialect::Postgres => value.replace('\'', "''"),
        SqlDialect::MySql => value.replace('\\', "\\\\").replace('\'', "''"),
        SqlDialect::ColumnarAnalytic => value.replace('\\', "\\\\").replace('\'', "\\'"),
    };
    Ok(format!("'{escaped}'"))
}

/// A numeric literal is emitted verbatim only when it parses as a finite number AND is
/// made of number characters alone.
fn render_numeric(value: &str) -> Result<String, String> {
    let n: f64 = value
        .parse()
        .map_err(|_| format!("non-numeric literal {value:?} for a numeric comparison"))?;
    if !n.is_finite() {
        return Err(format!("non-finite numeric literal {value:?}"));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))
    {
        return Err(format!("unsafe numeric literal {value:?}"));
    }
    Ok(value.to_string())
}

/// The bound-parameter form of the shared renderer for [`SqlDialect::ColumnarAnalytic`]
/// (MEDULLA-UNIFIED-DATA-PLANE-R017.2.2): identifiers quote through [`quote_identifier`] and
/// every value binds through an HTTP `param_<name>` query parameter, so no value is ever
/// spliced into the statement text.
pub const CLICKHOUSE_PARAM_PREFIX: &str = "param_";

/// The ClickHouse type a bound value carries in its `{name:Type}` placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickHouseParamKind {
    Text,
    Int64,
    Float64,
}

impl ClickHouseParamKind {
    fn type_name(self) -> &'static str {
        match self {
            Self::Text => "String",
            Self::Int64 => "Int64",
            Self::Float64 => "Float64",
        }
    }
}

/// One `column <op> value` filter of a rendered ClickHouse `SELECT`. `op` is one of
/// `=`, `!=`, `<`, `<=`, `>`, `>=`.
#[derive(Clone, Debug, PartialEq)]
pub struct ClickHouseFilter {
    pub column: String,
    pub op: &'static str,
    pub value: String,
    pub kind: ClickHouseParamKind,
}

/// A rendered ClickHouse statement and its bound parameters, each `(param_<name>, value)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClickHouseStatement {
    pub sql: String,
    pub params: Vec<(String, String)>,
}

/// Render `SELECT <columns> FROM <table> [WHERE ...]` for ClickHouse. A numeric value is
/// re-validated as a finite number before it binds; a NUL byte is refused.
pub fn render_columnar_select(
    table: &str,
    columns: &[String],
    filters: &[ClickHouseFilter],
) -> Result<ClickHouseStatement, String> {
    if columns.is_empty() {
        return Err("a ClickHouse select needs at least one column".into());
    }
    let projection = columns
        .iter()
        .map(|column| quote_identifier(column, SqlDialect::ColumnarAnalytic))
        .collect::<Result<Vec<_>, String>>()?
        .join(", ");
    let mut sql = format!("SELECT {projection} FROM {}", quote_identifier(table, SqlDialect::ColumnarAnalytic)?);
    let mut params = Vec::new();
    let mut predicates = Vec::new();
    for (index, filter) in filters.iter().enumerate() {
        if !matches!(filter.op, "=" | "!=" | "<" | "<=" | ">" | ">=") {
            return Err(format!("unsupported ClickHouse comparison {:?}", filter.op));
        }
        if filter.value.contains('\0') {
            return Err("SQL literal contains a NUL byte".into());
        }
        if filter.kind != ClickHouseParamKind::Text {
            render_numeric(&filter.value)?;
        }
        let name = format!("p{index}");
        predicates.push(format!(
            "{} {} {{{name}:{}}}",
            quote_identifier(&filter.column, SqlDialect::ColumnarAnalytic)?,
            filter.op,
            filter.kind.type_name()
        ));
        params.push((format!("{CLICKHOUSE_PARAM_PREFIX}{name}"), filter.value.clone()));
    }
    if !predicates.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&predicates.join(" AND "));
    }
    Ok(ClickHouseStatement { sql, params })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialect_follows_the_dsn_scheme() {
        assert_eq!(
            SqlDialect::from_dsn("postgresql://h/db"),
            Some(SqlDialect::Postgres)
        );
        assert_eq!(
            SqlDialect::from_dsn("mariadb://h/db"),
            Some(SqlDialect::MySql)
        );
        assert_eq!(SqlDialect::from_dsn("sqlite://x.db"), None);
    }

    #[test]
    fn identifiers_are_validated_before_quoting() {
        assert_eq!(
            quote_identifier("person_id", SqlDialect::Postgres).unwrap(),
            "\"person_id\""
        );
        assert_eq!(quote_identifier("id", SqlDialect::MySql).unwrap(), "`id`");
        let too_long = "x".repeat(64);
        for bad in ["", "1id", "id; DROP TABLE t", "a\"b", too_long.as_str()] {
            assert!(
                quote_identifier(bad, SqlDialect::Postgres).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn literals_escape_per_dialect_and_numbers_are_checked() {
        assert_eq!(
            render_literal("O'Brien", false, SqlDialect::Postgres).unwrap(),
            "'O''Brien'"
        );
        assert_eq!(
            render_literal("a\\'b", false, SqlDialect::MySql).unwrap(),
            "'a\\\\''b'"
        );
        assert_eq!(
            render_literal("-1.5e3", true, SqlDialect::Postgres).unwrap(),
            "-1.5e3"
        );
        assert!(render_literal("1; DROP", true, SqlDialect::Postgres).is_err());
        assert!(render_literal("inf", true, SqlDialect::Postgres).is_err());
        assert!(render_literal("a\0b", false, SqlDialect::Postgres).is_err());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.2
    #[test]
    fn clickhouse_dialect_quotes_and_renders() {
        assert_eq!(
            SqlDialect::from_dsn("clickhouse://h"),
            Some(SqlDialect::ColumnarAnalytic)
        );
        assert_eq!(
            quote_identifier("events", SqlDialect::ColumnarAnalytic).unwrap(),
            "`events`"
        );
        assert_eq!(
            render_literal("it's a\\b", false, SqlDialect::ColumnarAnalytic).unwrap(),
            "'it\\'s a\\\\b'"
        );
        assert!(render_literal("a\0b", false, SqlDialect::ColumnarAnalytic).is_err());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.2
    #[test]
    fn clickhouse_select_quotes_with_backticks_and_binds_values() {
        let statement = render_columnar_select(
            "events",
            &["id".to_string(), "name".to_string()],
            &[ClickHouseFilter {
                column: "id".into(),
                op: ">=",
                value: "7".into(),
                kind: ClickHouseParamKind::Int64,
            }],
        )
        .unwrap();
        assert_eq!(
            statement.sql,
            "SELECT `id`, `name` FROM `events` WHERE `id` >= {p0:Int64}"
        );
        assert_eq!(statement.params, vec![("param_p0".to_string(), "7".to_string())]);
        assert!(render_columnar_select("t; DROP", &["id".to_string()], &[]).is_err());
        assert!(render_columnar_select(
            "t",
            &["id".to_string()],
            &[ClickHouseFilter {
                column: "id".into(),
                op: "=",
                value: "1; DROP".into(),
                kind: ClickHouseParamKind::Int64,
            }]
        )
        .is_err());
    }
}
