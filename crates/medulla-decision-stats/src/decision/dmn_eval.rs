//! Decision table evaluation (MEDULLA-DECISION-ENGINE-R180.2).
//!
//! Pure and deterministic: no clock, no store, no I/O. [`evaluate_table`]
//! matches the rules of a stored DMN table against typed inputs in declared
//! order and applies the table's hit policy.
//!
//! An input entry is `-` (or empty) for any value, or a list of tests joined by
//! commas, or `not(` list `)`. A test is a literal (equality), a comparison
//! (`<`, `<=`, `>`, `>=` and a literal) or a range (`[a..b]`, `(a..b)`,
//! `[a..b)`, `(a..b]`; `]` and `[` also open and close an exclusive end).
//! A literal is a quoted string, an integer, a fixed-point decimal, `true`,
//! `false` or `date("YYYY-MM-DD")`. An entry outside this subset is refused
//! with its text, never read leniently. A value of another kind than the
//! literal never matches.
//!
//! PRIORITY and OUTPUT ORDER order outputs by the priority list of each output
//! clause (earlier is higher; an unlisted value is lowest), compared output
//! by output; PRIORITY keeps the first of equal priority in rule order.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use medulla_decision_types::decision::process_decision::{DmnValue, FixedDecimal, TableVerdict};

/// One output clause: its name and the value priority list (highest first).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DmnOutput {
    pub name: String,
    pub priority: Vec<DmnValue>,
}

/// One rule; entries keep their expression text verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DmnRule {
    pub id: String,
    pub input_entries: Vec<String>,
    pub output_entries: Vec<String>,
}

/// One decision table: the input names its clauses read, its outputs, the hit
/// policy (`UNIQUE`, `FIRST`, `PRIORITY`, `ANY`, `COLLECT`, `RULE ORDER` or
/// `OUTPUT ORDER`), the optional COLLECT aggregation and its rules.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DmnTable {
    pub hit_policy: String,
    pub aggregation: Option<String>,
    pub input_names: Vec<String>,
    pub outputs: Vec<DmnOutput>,
    pub rules: Vec<DmnRule>,
}

/// Why a table cannot be evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmnError {
    /// UNIQUE, ANY or FIRST matched no rule.
    NoRuleMatched,
    /// UNIQUE matched more than one rule.
    UniqueViolated { rule_ids: Vec<String> },
    /// ANY matched rules with different outputs.
    AnyConflict { rule_ids: Vec<String> },
    /// An entry or a hit policy outside the supported subset.
    ExpressionUnsupported { entry: String },
    /// A table input the request did not supply.
    InputMissing { name: String },
    /// A rule whose entry count differs from the table's clause count.
    MalformedTable { rule_id: String },
    /// An aggregation that does not fit 64-bit fixed point.
    Overflow,
}

#[derive(Debug, Clone)]
enum Alternative {
    Equals(DmnValue),
    Compare(Ordering, bool, DmnValue),
    Range {
        low: DmnValue,
        low_inclusive: bool,
        high: DmnValue,
        high_inclusive: bool,
    },
}

#[derive(Debug, Clone)]
enum Test {
    Any,
    Alternatives(Vec<Alternative>),
    Not(Vec<Alternative>),
}

fn unsupported(entry: &str) -> DmnError {
    DmnError::ExpressionUnsupported {
        entry: entry.to_string(),
    }
}

fn parse_date(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
    shaped.then(|| text.to_string())
}

fn parse_literal(text: &str) -> Option<DmnValue> {
    let text = text.trim();
    if text == "true" {
        return Some(DmnValue::Boolean(true));
    }
    if text == "false" {
        return Some(DmnValue::Boolean(false));
    }
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        let inner = &text[1..text.len() - 1];
        return (!inner.contains('"')).then(|| DmnValue::String(inner.to_string()));
    }
    if let Some(inner) = text.strip_prefix("date(\"").and_then(|t| t.strip_suffix("\")")) {
        return parse_date(inner).map(DmnValue::Date);
    }
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() {
        return None;
    }
    match digits.split_once('.') {
        None if digits.bytes().all(|b| b.is_ascii_digit()) => {
            text.parse::<i64>().ok().map(DmnValue::Integer)
        }
        Some((whole, fraction))
            if !whole.is_empty()
                && !fraction.is_empty()
                && fraction.len() <= 18
                && whole.bytes().all(|b| b.is_ascii_digit())
                && fraction.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let negative = text.starts_with('-');
            let units: i64 = format!("{whole}{fraction}").parse().ok()?;
            Some(DmnValue::Decimal(FixedDecimal {
                units: if negative { -units } else { units },
                scale: fraction.len() as u8,
            }))
        }
        _ => None,
    }
}

/// Split on top-level commas: not inside quotes or parentheses.
fn split_list(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quoted = false;
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn parse_alternative(text: &str) -> Option<Alternative> {
    let text = text.trim();
    for (prefix, ordering, inclusive) in [
        ("<=", Ordering::Less, true),
        (">=", Ordering::Greater, true),
        ("<", Ordering::Less, false),
        (">", Ordering::Greater, false),
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return Some(Alternative::Compare(ordering, inclusive, parse_literal(rest)?));
        }
    }
    let first = text.chars().next()?;
    let last = text.chars().last()?;
    if matches!(first, '[' | '(' | ']') && matches!(last, ']' | ')' | '[') && text.len() >= 4 {
        let inner = &text[1..text.len() - 1];
        let (low, high) = inner.split_once("..")?;
        return Some(Alternative::Range {
            low: parse_literal(low)?,
            low_inclusive: first == '[',
            high: parse_literal(high)?,
            high_inclusive: last == ']',
        });
    }
    parse_literal(text).map(Alternative::Equals)
}

fn parse_alternatives(text: &str) -> Option<Vec<Alternative>> {
    split_list(text).into_iter().map(parse_alternative).collect()
}

fn parse_entry(entry: &str) -> Result<Test, DmnError> {
    let text = entry.trim();
    if text.is_empty() || text == "-" {
        return Ok(Test::Any);
    }
    let parsed = match text.strip_prefix("not(").and_then(|t| t.strip_suffix(')')) {
        Some(inner) => parse_alternatives(inner).map(Test::Not),
        None => parse_alternatives(text).map(Test::Alternatives),
    };
    parsed.ok_or_else(|| unsupported(entry))
}

fn scaled(value: &FixedDecimal, scale: u8) -> i128 {
    i128::from(value.units) * 10i128.pow(u32::from(scale - value.scale))
}

fn compare(left: &DmnValue, right: &DmnValue) -> Option<Ordering> {
    match (left, right) {
        (DmnValue::String(a), DmnValue::String(b)) => Some(a.cmp(b)),
        (DmnValue::Date(a), DmnValue::Date(b)) => Some(a.cmp(b)),
        (DmnValue::Boolean(a), DmnValue::Boolean(b)) => Some(a.cmp(b)),
        (DmnValue::Integer(a), DmnValue::Integer(b)) => Some(a.cmp(b)),
        (DmnValue::Integer(a), DmnValue::Decimal(b)) => {
            let a = FixedDecimal { units: *a, scale: 0 };
            let scale = b.scale;
            Some(scaled(&a, scale).cmp(&scaled(b, scale)))
        }
        (DmnValue::Decimal(a), DmnValue::Integer(b)) => {
            let b = FixedDecimal { units: *b, scale: 0 };
            let scale = a.scale;
            Some(scaled(a, scale).cmp(&scaled(&b, scale)))
        }
        (DmnValue::Decimal(a), DmnValue::Decimal(b)) => {
            let scale = a.scale.max(b.scale);
            Some(scaled(a, scale).cmp(&scaled(b, scale)))
        }
        _ => None,
    }
}

fn alternative_holds(alternative: &Alternative, value: &DmnValue) -> bool {
    match alternative {
        Alternative::Equals(literal) => compare(value, literal) == Some(Ordering::Equal),
        Alternative::Compare(wanted, inclusive, literal) => match compare(value, literal) {
            Some(Ordering::Equal) => *inclusive,
            Some(found) => found == *wanted,
            None => false,
        },
        Alternative::Range {
            low,
            low_inclusive,
            high,
            high_inclusive,
        } => {
            let above = match compare(value, low) {
                Some(Ordering::Greater) => true,
                Some(Ordering::Equal) => *low_inclusive,
                _ => false,
            };
            let below = match compare(value, high) {
                Some(Ordering::Less) => true,
                Some(Ordering::Equal) => *high_inclusive,
                _ => false,
            };
            above && below
        }
    }
}

fn parse_output(entry: &str) -> Result<DmnValue, DmnError> {
    parse_literal(entry).ok_or_else(|| unsupported(entry))
}

fn priority_rank(output: &DmnOutput, value: &DmnValue) -> usize {
    output
        .priority
        .iter()
        .position(|candidate| candidate == value)
        .unwrap_or(usize::MAX)
}

fn row_rank(table: &DmnTable, row: &[DmnValue]) -> Vec<usize> {
    table
        .outputs
        .iter()
        .zip(row)
        .map(|(output, value)| priority_rank(output, value))
        .collect()
}

fn named(table: &DmnTable, row: Vec<DmnValue>) -> BTreeMap<String, DmnValue> {
    table
        .outputs
        .iter()
        .map(|output| output.name.clone())
        .zip(row)
        .collect()
}

fn aggregate(
    table: &DmnTable,
    kind: &str,
    rows: &[Vec<DmnValue>],
) -> Result<Vec<BTreeMap<String, DmnValue>>, DmnError> {
    if kind == "COUNT" {
        let name = table.outputs.first().map(|o| o.name.clone()).unwrap_or_default();
        let count = i64::try_from(rows.len()).map_err(|_| DmnError::Overflow)?;
        return Ok(vec![BTreeMap::from([(name, DmnValue::Integer(count))])]);
    }
    if !matches!(kind, "SUM" | "MIN" | "MAX") || table.outputs.len() != 1 {
        return Err(unsupported(kind));
    }
    let mut numbers: Vec<FixedDecimal> = Vec::new();
    for row in rows {
        match &row[0] {
            DmnValue::Integer(units) => numbers.push(FixedDecimal {
                units: *units,
                scale: 0,
            }),
            DmnValue::Decimal(value) => numbers.push(*value),
            _ => return Err(unsupported(kind)),
        }
    }
    let Some(scale) = numbers.iter().map(|n| n.scale).max() else {
        return Ok(Vec::new());
    };
    let all_integer = scale == 0;
    let values: Vec<i128> = numbers.iter().map(|n| scaled(n, scale)).collect();
    let result = match kind {
        "SUM" => values.iter().sum::<i128>(),
        "MIN" => values.iter().copied().min().unwrap_or_default(),
        _ => values.iter().copied().max().unwrap_or_default(),
    };
    let units = i64::try_from(result).map_err(|_| DmnError::Overflow)?;
    let value = if all_integer {
        DmnValue::Integer(units)
    } else {
        DmnValue::Decimal(FixedDecimal { units, scale })
    };
    Ok(vec![BTreeMap::from([(table.outputs[0].name.clone(), value)])])
}

/// Evaluate `table` over `inputs`. Equal inputs give equal outputs.
pub fn evaluate_table(
    table: &DmnTable,
    inputs: &BTreeMap<String, DmnValue>,
) -> Result<TableVerdict, DmnError> {
    let policy = table.hit_policy.trim().to_uppercase().replace('_', " ");
    if !matches!(
        policy.as_str(),
        "UNIQUE" | "FIRST" | "PRIORITY" | "ANY" | "COLLECT" | "RULE ORDER" | "OUTPUT ORDER"
    ) {
        return Err(unsupported(&table.hit_policy));
    }
    let mut matched: Vec<(&DmnRule, Vec<DmnValue>)> = Vec::new();
    for rule in &table.rules {
        if rule.input_entries.len() != table.input_names.len()
            || rule.output_entries.len() != table.outputs.len()
        {
            return Err(DmnError::MalformedTable {
                rule_id: rule.id.clone(),
            });
        }
        let mut holds = true;
        for (name, entry) in table.input_names.iter().zip(&rule.input_entries) {
            let test = parse_entry(entry)?;
            if matches!(test, Test::Any) {
                continue;
            }
            let value = inputs.get(name.trim()).ok_or_else(|| DmnError::InputMissing {
                name: name.trim().to_string(),
            })?;
            let hit = |alternatives: &[Alternative]| {
                alternatives.iter().any(|a| alternative_holds(a, value))
            };
            let passes = match &test {
                Test::Any => true,
                Test::Alternatives(alternatives) => hit(alternatives),
                Test::Not(alternatives) => !hit(alternatives),
            };
            if !passes {
                holds = false;
                break;
            }
        }
        if holds {
            let row = rule
                .output_entries
                .iter()
                .map(|entry| parse_output(entry))
                .collect::<Result<Vec<_>, _>>()?;
            matched.push((rule, row));
        }
    }
    let ids = |rules: &[(&DmnRule, Vec<DmnValue>)]| -> Vec<String> {
        rules.iter().map(|(rule, _)| rule.id.clone()).collect()
    };
    let verdict = |rules: Vec<(&DmnRule, Vec<DmnValue>)>| TableVerdict {
        matched_rule_ids: ids(&rules),
        outputs: rules.into_iter().map(|(_, row)| named(table, row)).collect(),
    };
    match policy.as_str() {
        "UNIQUE" => match matched.len() {
            0 => Err(DmnError::NoRuleMatched),
            1 => Ok(verdict(matched)),
            _ => Err(DmnError::UniqueViolated {
                rule_ids: ids(&matched),
            }),
        },
        "FIRST" => {
            matched.truncate(1);
            if matched.is_empty() {
                return Err(DmnError::NoRuleMatched);
            }
            Ok(verdict(matched))
        }
        "ANY" => {
            let Some((_, first)) = matched.first() else {
                return Err(DmnError::NoRuleMatched);
            };
            if matched.iter().any(|(_, row)| row != first) {
                return Err(DmnError::AnyConflict {
                    rule_ids: ids(&matched),
                });
            }
            Ok(verdict(matched))
        }
        "PRIORITY" => {
            let mut best: Option<usize> = None;
            for (index, (_, row)) in matched.iter().enumerate() {
                let better = match best {
                    None => true,
                    Some(held) => row_rank(table, row) < row_rank(table, &matched[held].1),
                };
                if better {
                    best = Some(index);
                }
            }
            match best {
                None => Err(DmnError::NoRuleMatched),
                Some(index) => Ok(verdict(vec![matched.swap_remove(index)])),
            }
        }
        "OUTPUT ORDER" => {
            matched.sort_by_key(|(_, row)| row_rank(table, row));
            Ok(verdict(matched))
        }
        "COLLECT" => match table.aggregation.as_deref().map(str::trim) {
            None | Some("") => Ok(verdict(matched)),
            Some(kind) => {
                let rows: Vec<Vec<DmnValue>> = matched.iter().map(|(_, row)| row.clone()).collect();
                Ok(TableVerdict {
                    matched_rule_ids: ids(&matched),
                    outputs: aggregate(table, &kind.to_uppercase(), &rows)?,
                })
            }
        },
        _ => Ok(verdict(matched)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, inputs: &[&str], outputs: &[&str]) -> DmnRule {
        DmnRule {
            id: id.into(),
            input_entries: inputs.iter().map(|s| (*s).into()).collect(),
            output_entries: outputs.iter().map(|s| (*s).into()).collect(),
        }
    }

    fn table(policy: &str, aggregation: Option<&str>, priority: &[&str]) -> DmnTable {
        DmnTable {
            hit_policy: policy.into(),
            aggregation: aggregation.map(str::to_string),
            input_names: vec!["age".into(), "tier".into()],
            outputs: vec![DmnOutput {
                name: "discount".into(),
                priority: priority.iter().map(|p| parse_literal(p).unwrap()).collect(),
            }],
            rules: vec![
                rule("r1", &["[18..65]", "\"gold\", \"silver\""], &["10"]),
                rule("r2", &[">=65", "-"], &["20"]),
                rule("r3", &["<18", "not(\"gold\")"], &["5"]),
                rule("r4", &["-", "\"gold\""], &["10"]),
            ],
        }
    }

    fn inputs(age: i64, tier: &str) -> BTreeMap<String, DmnValue> {
        BTreeMap::from([
            ("age".to_string(), DmnValue::Integer(age)),
            ("tier".to_string(), DmnValue::String(tier.into())),
        ])
    }

    fn discounts(verdict: &TableVerdict) -> Vec<DmnValue> {
        verdict.outputs.iter().map(|row| row["discount"].clone()).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R180.2
    #[test]
    fn every_hit_policy_matches_its_fixture() {
        // UNIQUE: one match, none, and a violation naming both rules.
        let unique = table("UNIQUE", None, &[]);
        let one = evaluate_table(&unique, &inputs(70, "bronze")).unwrap();
        assert_eq!(one.matched_rule_ids, vec!["r2"]);
        assert_eq!(discounts(&one), vec![DmnValue::Integer(20)]);
        assert_eq!(
            evaluate_table(&unique, &inputs(30, "bronze")),
            Err(DmnError::NoRuleMatched)
        );
        assert_eq!(
            evaluate_table(&unique, &inputs(30, "gold")),
            Err(DmnError::UniqueViolated {
                rule_ids: vec!["r1".into(), "r4".into()]
            })
        );

        // FIRST takes the first match in declared order.
        let first = evaluate_table(&table("FIRST", None, &[]), &inputs(30, "gold")).unwrap();
        assert_eq!(first.matched_rule_ids, vec!["r1"]);

        // PRIORITY takes the output listed first.
        let priority = table("PRIORITY", None, &["20", "10", "5"]);
        let verdict = evaluate_table(&priority, &inputs(70, "gold")).unwrap();
        assert_eq!(discounts(&verdict), vec![DmnValue::Integer(20)]);
        assert_eq!(verdict.matched_rule_ids, vec!["r2"]);

        // ANY accepts equal outputs and refuses different ones.
        let any = table("ANY", None, &[]);
        let agreed = evaluate_table(&any, &inputs(30, "gold")).unwrap();
        assert_eq!(agreed.matched_rule_ids, vec!["r1", "r4"]);
        assert_eq!(
            evaluate_table(&any, &inputs(70, "gold")),
            Err(DmnError::AnyConflict {
                rule_ids: vec!["r2".into(), "r4".into()]
            })
        );

        // COLLECT returns all matches, or an aggregation of them.
        let collect = evaluate_table(&table("COLLECT", None, &[]), &inputs(70, "gold")).unwrap();
        assert_eq!(
            discounts(&collect),
            vec![DmnValue::Integer(20), DmnValue::Integer(10)]
        );
        for (kind, expected) in [("SUM", 30), ("MIN", 10), ("MAX", 20), ("COUNT", 2)] {
            let verdict =
                evaluate_table(&table("COLLECT", Some(kind), &[]), &inputs(70, "gold")).unwrap();
            assert_eq!(discounts(&verdict), vec![DmnValue::Integer(expected)], "{kind}");
        }

        // RULE ORDER keeps declared order; OUTPUT ORDER sorts by priority.
        let rule_order = evaluate_table(&table("RULE ORDER", None, &[]), &inputs(70, "gold")).unwrap();
        assert_eq!(rule_order.matched_rule_ids, vec!["r2", "r4"]);
        let output_order =
            evaluate_table(&table("OUTPUT ORDER", None, &["10", "20"]), &inputs(70, "gold")).unwrap();
        assert_eq!(
            discounts(&output_order),
            vec![DmnValue::Integer(10), DmnValue::Integer(20)]
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R180.2
    #[test]
    fn entries_outside_the_subset_and_missing_inputs_are_refused() {
        let mut bad = table("FIRST", None, &[]);
        bad.rules[0].input_entries[0] = "age + 1".into();
        assert_eq!(
            evaluate_table(&bad, &inputs(30, "gold")),
            Err(DmnError::ExpressionUnsupported {
                entry: "age + 1".into()
            })
        );
        let missing = BTreeMap::from([("age".to_string(), DmnValue::Integer(30))]);
        assert_eq!(
            evaluate_table(&table("FIRST", None, &[]), &missing),
            Err(DmnError::InputMissing {
                name: "tier".into()
            })
        );
    }
}
