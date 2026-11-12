//! Row encoding for `COPY … TO STDOUT` (MEDULLA-ENGINE-PARITY-R006.1): the
//! PostgreSQL text and CSV copy formats, one `CopyData` payload per row.

use medulla_sql_lang::sql::classify::{CopyFormat, CopyPlan};
use serde_json::Value;

/// Encode `rows` (and, when `plan.header` is set, a header line of `names`).
/// Binary is refused at classification, so only text and CSV reach here.
pub(super) fn encode(plan: &CopyPlan, names: &[&str], rows: &[Vec<Value>]) -> Vec<Vec<u8>> {
    let csv = plan.format == CopyFormat::Csv;
    let delimiter = plan.delimiter.unwrap_or(if csv { ',' } else { '\t' });
    let mut lines = Vec::with_capacity(rows.len() + 1);
    if plan.header {
        let header: Vec<Option<String>> = names.iter().map(|n| Some((*n).to_string())).collect();
        lines.push(line(&header, csv, delimiter));
    }
    for row in rows {
        let cells: Vec<Option<String>> = row.iter().map(render).collect();
        lines.push(line(&cells, csv, delimiter));
    }
    lines
}

/// A value's copy text; `None` is SQL NULL.
fn render(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(if *flag { "t" } else { "f" }.to_string()),
        other => Some(other.to_string()),
    }
}

fn line(cells: &[Option<String>], csv: bool, delimiter: char) -> Vec<u8> {
    let mut out = String::new();
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            out.push(delimiter);
        }
        match (cell, csv) {
            (None, true) => {}
            (None, false) => out.push_str("\\N"),
            (Some(text), true) => push_csv(&mut out, text, delimiter),
            (Some(text), false) => push_text(&mut out, text, delimiter),
        }
    }
    out.push('\n');
    out.into_bytes()
}

fn push_text(out: &mut String, text: &str, delimiter: char) {
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == delimiter => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
}

fn push_csv(out: &mut String, text: &str, delimiter: char) {
    let needs_quotes = text.is_empty()
        || text
            .chars()
            .any(|c| c == delimiter || matches!(c, '"' | '\n' | '\r'));
    if !needs_quotes {
        out.push_str(text);
        return;
    }
    out.push('"');
    for ch in text.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(format: CopyFormat, header: bool) -> CopyPlan {
        CopyPlan {
            table: "t".into(),
            columns: Vec::new(),
            format,
            delimiter: None,
            header,
        }
    }

    fn text(lines: Vec<Vec<u8>>) -> Vec<String> {
        lines
            .into_iter()
            .map(|l| String::from_utf8(l).unwrap())
            .collect()
    }

    // spec: MEDULLA-ENGINE-PARITY-R006.1
    #[test]
    fn text_format_escapes_and_marks_null() {
        let rows = vec![vec![json!("a\tb"), json!(null), json!(7), json!(true)]];
        let out = text(encode(
            &plan(CopyFormat::Text, false),
            &["a", "b", "c", "d"],
            &rows,
        ));
        assert_eq!(out, vec!["a\\tb\t\\N\t7\tt\n".to_string()]);
    }

    // spec: MEDULLA-ENGINE-PARITY-R006.1
    #[test]
    fn csv_format_quotes_and_prints_header() {
        let rows = vec![vec![
            json!("x,y"),
            json!(null),
            json!("say \"hi\""),
            json!(""),
        ]];
        let out = text(encode(
            &plan(CopyFormat::Csv, true),
            &["a", "b", "c", "d"],
            &rows,
        ));
        assert_eq!(
            out,
            vec![
                "a,b,c,d\n".to_string(),
                "\"x,y\",,\"say \"\"hi\"\"\",\"\"\n".to_string()
            ]
        );
    }
}
