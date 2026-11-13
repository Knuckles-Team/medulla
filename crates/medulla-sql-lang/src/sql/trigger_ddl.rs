//! The textual `CREATE TRIGGER` and `DROP TRIGGER` parsers
//! (MEDULLA-ENGINE-PARITY-R012.2).
//!
//! `CREATE TRIGGER name {BEFORE|AFTER} {INSERT|UPDATE|DELETE} [OR …] ON table
//! EXECUTE MACHINE machine_id` and `DROP TRIGGER name ON table`. The trigger runs
//! the stored statechart `machine_id`; the PostgreSQL `EXECUTE FUNCTION` form is
//! not this engine's trigger and keeps falling through to the refused-form path.
//! A malformed statement is a precise error, never a silent fall-through.

use medulla_tables::trigger::{TriggerEvent, TriggerTiming};

/// A parsed `CREATE TRIGGER`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTriggerPlan {
    /// Trigger name, unique within its table.
    pub name: String,
    /// The table the trigger belongs to.
    pub table: String,
    /// Before or after the row write.
    pub timing: TriggerTiming,
    /// The row events it reacts to, in statement order, without repeats.
    pub events: Vec<TriggerEvent>,
    /// The stored statechart definition the trigger runs.
    pub machine_id: String,
}

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

fn ident(token: Option<&String>, what: &str) -> Result<String, String> {
    token
        .map(|t| t.trim_matches('"').to_string())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| format!("TRIGGER_INVALID: a {what} is required"))
}

fn event_of(token: &str) -> Option<TriggerEvent> {
    [
        TriggerEvent::Insert,
        TriggerEvent::Update,
        TriggerEvent::Delete,
    ]
    .into_iter()
    .find(|event| token.eq_ignore_ascii_case(event.as_str()))
}

/// `CREATE TRIGGER …` in the machine form: `None` when the statement is not one
/// (including the `EXECUTE FUNCTION` form), otherwise the plan or a precise error.
pub fn parse_create_trigger(sql: &str) -> Option<Result<CreateTriggerPlan, String>> {
    let toks = tokens(sql);
    if !(is_kw(&toks, 0, "create") && is_kw(&toks, 1, "trigger")) {
        return None;
    }
    if !toks.iter().any(|t| t.eq_ignore_ascii_case("machine")) {
        return None;
    }
    Some(create_trigger_of(&toks))
}

fn create_trigger_of(toks: &[String]) -> Result<CreateTriggerPlan, String> {
    let name = ident(toks.get(2), "trigger name")?;
    let timing = if is_kw(toks, 3, "before") {
        TriggerTiming::Before
    } else if is_kw(toks, 3, "after") {
        TriggerTiming::After
    } else {
        return Err("TRIGGER_INVALID: expected BEFORE or AFTER".to_string());
    };
    let mut i = 4;
    let mut events = Vec::new();
    loop {
        let event = toks
            .get(i)
            .and_then(|t| event_of(t))
            .ok_or_else(|| "TRIGGER_INVALID: expected INSERT, UPDATE or DELETE".to_string())?;
        if events.contains(&event) {
            return Err(format!(
                "TRIGGER_INVALID: event {} named twice",
                event.as_str()
            ));
        }
        events.push(event);
        i += 1;
        if is_kw(toks, i, "or") {
            i += 1;
        } else {
            break;
        }
    }
    if !is_kw(toks, i, "on") {
        return Err("TRIGGER_INVALID: expected ON <table>".to_string());
    }
    let table = ident(toks.get(i + 1), "table name")?;
    i += 2;
    if !(is_kw(toks, i, "execute") && is_kw(toks, i + 1, "machine")) {
        return Err("TRIGGER_INVALID: expected EXECUTE MACHINE <machine_id>".to_string());
    }
    let machine_id = ident(toks.get(i + 2), "machine id")?;
    if toks.len() > i + 3 {
        return Err(format!(
            "TRIGGER_INVALID: unexpected `{}` in CREATE TRIGGER",
            toks[i + 3]
        ));
    }
    Ok(CreateTriggerPlan {
        name,
        table,
        timing,
        events,
        machine_id,
    })
}

/// `DROP TRIGGER name ON table`: `None` when the statement is not one, otherwise
/// the trigger name and its table.
pub fn parse_drop_trigger(sql: &str) -> Option<Result<(String, String), String>> {
    let toks = tokens(sql);
    if !(is_kw(&toks, 0, "drop") && is_kw(&toks, 1, "trigger")) {
        return None;
    }
    Some(drop_trigger_of(&toks))
}

fn drop_trigger_of(toks: &[String]) -> Result<(String, String), String> {
    let name = ident(toks.get(2), "trigger name")?;
    if !is_kw(toks, 3, "on") {
        return Err("TRIGGER_INVALID: expected ON <table>".to_string());
    }
    let table = ident(toks.get(4), "table name")?;
    if toks.len() > 5 {
        return Err("TRIGGER_INVALID: DROP TRIGGER takes one name and one table".to_string());
    }
    Ok((name, table))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ENGINE-PARITY-R012.2
    #[test]
    fn create_trigger_parses_timing_events_table_and_machine() {
        let plan = parse_create_trigger(
            "CREATE TRIGGER audit AFTER INSERT OR UPDATE ON orders EXECUTE MACHINE abc123;",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (
                plan.name.as_str(),
                plan.table.as_str(),
                plan.machine_id.as_str()
            ),
            ("audit", "orders", "abc123")
        );
        assert_eq!(plan.timing, TriggerTiming::After);
        assert_eq!(
            plan.events,
            vec![TriggerEvent::Insert, TriggerEvent::Update]
        );
        assert!(parse_create_trigger("CREATE TABLE t (a int)").is_none());
        assert!(parse_create_trigger(
            "CREATE TRIGGER t BEFORE INSERT ON a FOR EACH ROW EXECUTE FUNCTION f()"
        )
        .is_none());
        assert!(parse_create_trigger(
            "CREATE TRIGGER t BEFORE INSERT OR INSERT ON a EXECUTE MACHINE m"
        )
        .unwrap()
        .is_err());
        assert!(
            parse_create_trigger("CREATE TRIGGER t SOMETIME INSERT ON a EXECUTE MACHINE m")
                .unwrap()
                .is_err()
        );
    }

    // spec: MEDULLA-ENGINE-PARITY-R012.2
    #[test]
    fn drop_trigger_parses_name_and_table() {
        assert_eq!(
            parse_drop_trigger("DROP TRIGGER audit ON orders;")
                .unwrap()
                .unwrap(),
            ("audit".to_string(), "orders".to_string())
        );
        assert!(parse_drop_trigger("DROP TRIGGER audit").unwrap().is_err());
        assert!(parse_drop_trigger("DROP TABLE t").is_none());
    }
}
