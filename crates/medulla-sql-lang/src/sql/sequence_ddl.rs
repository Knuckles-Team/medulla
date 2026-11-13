//! The textual `CREATE SEQUENCE` and `DROP SEQUENCE` parsers
//! (MEDULLA-ENGINE-PARITY-R013.2).
//!
//! `CREATE SEQUENCE [IF NOT EXISTS] name [START [WITH] n] [INCREMENT [BY] n]
//! [CACHE n]` and `DROP SEQUENCE [IF EXISTS] name`. The options may come in any
//! order, each at most once. A malformed statement is a precise error, never a
//! silent fall-through to the generic parser.

use medulla_tables::sequence::Sequence;

fn tokens(sql: &str) -> Vec<String> {
    sql.trim()
        .trim_end_matches(';')
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn is_kw(toks: &[String], i: usize, kw: &str) -> bool {
    toks.get(i).is_some_and(|t| t.eq_ignore_ascii_case(kw))
}

fn is_sequence_statement(toks: &[String], verb: &str) -> bool {
    is_kw(toks, 0, verb) && is_kw(toks, 1, "sequence")
}

fn name_of(token: Option<&String>) -> Result<String, String> {
    let name = token
        .map(|t| t.trim_matches('"').to_string())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "SEQUENCE_INVALID: a sequence name is required".to_string())?;
    Ok(name)
}

fn number(toks: &[String], i: usize, option: &str) -> Result<i64, String> {
    toks.get(i)
        .and_then(|t| t.parse::<i64>().ok())
        .ok_or_else(|| format!("SEQUENCE_INVALID: {option} needs an integer value"))
}

/// `CREATE SEQUENCE …`: `None` when the statement is not one, otherwise the
/// definition and the `IF NOT EXISTS` flag.
pub fn parse_create_sequence(sql: &str) -> Option<Result<(Sequence, bool), String>> {
    let toks = tokens(sql);
    if !is_sequence_statement(&toks, "create") {
        return None;
    }
    Some(create_sequence_of(&toks))
}

fn create_sequence_of(toks: &[String]) -> Result<(Sequence, bool), String> {
    let mut i = 2;
    let mut if_not_exists = false;
    if is_kw(toks, i, "if") {
        if !(is_kw(toks, i + 1, "not") && is_kw(toks, i + 2, "exists")) {
            return Err("SEQUENCE_INVALID: expected IF NOT EXISTS".to_string());
        }
        if_not_exists = true;
        i += 3;
    }
    let mut def = Sequence::new(name_of(toks.get(i))?);
    i += 1;
    let (mut start, mut increment, mut cache) = (false, false, false);
    while i < toks.len() {
        if is_kw(toks, i, "start") && !start {
            i += 1;
            if is_kw(toks, i, "with") {
                i += 1;
            }
            def.start = number(toks, i, "START")?;
            start = true;
        } else if is_kw(toks, i, "increment") && !increment {
            i += 1;
            if is_kw(toks, i, "by") {
                i += 1;
            }
            def.increment = number(toks, i, "INCREMENT")?;
            increment = true;
        } else if is_kw(toks, i, "cache") && !cache {
            i += 1;
            let value = number(toks, i, "CACHE")?;
            def.cache = u64::try_from(value)
                .map_err(|_| "SEQUENCE_INVALID: CACHE must not be negative".to_string())?;
            cache = true;
        } else {
            return Err(format!(
                "SEQUENCE_INVALID: unexpected `{}` in CREATE SEQUENCE",
                toks[i]
            ));
        }
        i += 1;
    }
    Ok((def, if_not_exists))
}

/// `DROP SEQUENCE [IF EXISTS] name`: `None` when the statement is not one,
/// otherwise the name and the `IF EXISTS` flag.
pub fn parse_drop_sequence(sql: &str) -> Option<Result<(String, bool), String>> {
    let toks = tokens(sql);
    if !is_sequence_statement(&toks, "drop") {
        return None;
    }
    let mut i = 2;
    let mut if_exists = false;
    if is_kw(&toks, i, "if") {
        if !is_kw(&toks, i + 1, "exists") {
            return Some(Err("SEQUENCE_INVALID: expected IF EXISTS".to_string()));
        }
        if_exists = true;
        i += 2;
    }
    let name = match name_of(toks.get(i)) {
        Ok(name) => name,
        Err(error) => return Some(Err(error)),
    };
    if toks.len() > i + 1 {
        return Some(Err(
            "SEQUENCE_INVALID: DROP SEQUENCE takes one name".to_string()
        ));
    }
    Some(Ok((name, if_exists)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ENGINE-PARITY-R013.2
    #[test]
    fn create_sequence_options_parse_in_any_order() {
        let (def, if_not_exists) = parse_create_sequence(
            "CREATE SEQUENCE IF NOT EXISTS s CACHE 4 START WITH 10 INCREMENT BY 5;",
        )
        .unwrap()
        .unwrap();
        assert!(if_not_exists);
        assert_eq!(
            (def.name.as_str(), def.start, def.increment, def.cache),
            ("s", 10, 5, 4)
        );
        assert!(parse_create_sequence("CREATE TABLE t (a int)").is_none());
        assert!(parse_create_sequence("CREATE SEQUENCE s START 1 START 2")
            .unwrap()
            .is_err());
    }

    // spec: MEDULLA-ENGINE-PARITY-R013.2
    #[test]
    fn drop_sequence_parses_if_exists_and_one_name() {
        assert_eq!(
            parse_drop_sequence("drop sequence if exists s")
                .unwrap()
                .unwrap(),
            ("s".to_string(), true)
        );
        assert!(parse_drop_sequence("DROP SEQUENCE a b").unwrap().is_err());
        assert!(parse_drop_sequence("DROP TABLE t").is_none());
    }
}
