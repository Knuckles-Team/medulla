//! Decoder-driven repair of a schema-built sample (MEDULLA-CONTRACT-R001.3).
//!
//! A schema cannot state every constraint a decoder enforces: a non-empty id, 32 bytes of
//! lowercase hex, a canonical decimal, a non-zero denominator. The builder emits the empty
//! string and zero for such leaves. The repair walks those placeholder leaves in document
//! order and raises them up a fixed ladder of replacements. The decoder stops at its first
//! error, so a replacement that clears or moves the error fixed the failing leaf and is kept.
//! The repair is deterministic: fixed ladders, fixed order, no randomness.

use serde_json::Value;

/// Repair `sample` until `decode` accepts it, or return the decoder's last error. Each kept
/// trial steps at least one leaf up its finite ladder, so the repair always terminates. When
/// no step moves the error, the last resort drops one emitted property the decoder rejects.
pub fn repair(
    mut sample: Value,
    decode: impl Fn(&Value) -> Result<(), String>,
) -> Result<Value, String> {
    let mut error = match decode(&sample) {
        Ok(()) => return Ok(sample),
        Err(error) => error,
    };
    loop {
        let step =
            repair_pass(&sample, &error, &decode).or_else(|| prune(&sample, &error, &decode));
        match step {
            Some(Ok(repaired)) => return Ok(repaired),
            Some(Err((repaired, next))) => (sample, error) = (repaired, next),
            None => return Err(error),
        }
    }
}

/// One pass: for each rung, step the placeholder leaves of a growing prefix (in document
/// order) and keep the first trial that clears (`Ok`) or moves (`Err`) `error`. A prefix,
/// not one leaf, because two leaves of the same type report the same error: fixing only the
/// first would look like no progress. A moved trial is then minimised so the leaves that did
/// not matter stay placeholders for a later rung.
fn repair_pass(
    sample: &Value,
    error: &str,
    decode: &impl Fn(&Value) -> Result<(), String>,
) -> Option<Result<Value, (Value, String)>> {
    let leaves = placeholder_leaves(sample);
    for rung in 0..LADDER_RUNGS {
        let mut trial = sample.clone();
        for (index, pointer) in leaves.iter().enumerate() {
            step(&mut trial, pointer, rung);
            match decode(&trial) {
                Ok(()) => return Some(Ok(trial)),
                Err(next) if moved(error, &next, trial.pointer(pointer)) => {
                    let kept = minimise(sample, trial, &leaves[..index], decode);
                    return Some(Err((kept, next)));
                }
                Err(_) => {}
            }
        }
    }
    None
}

/// Revert each earlier stepped leaf whose revert leaves the decoder's answer unchanged.
fn minimise(
    original: &Value,
    mut trial: Value,
    earlier: &[String],
    decode: &impl Fn(&Value) -> Result<(), String>,
) -> Value {
    let answer = decode(&trial);
    for pointer in earlier {
        let mut reverted = trial.clone();
        if let (Some(leaf), Some(before)) =
            (reverted.pointer_mut(pointer), original.pointer(pointer))
        {
            *leaf = before.clone();
        }
        if decode(&reverted) == answer {
            trial = reverted;
        }
    }
    trial
}

/// The last resort: drop the first emitted property whose removal clears or moves the
/// error (an optional field that breaks a cross-field rule). A removal that only reports the
/// field missing is no progress. Each removal shrinks the sample, so this terminates.
fn prune(
    sample: &Value,
    error: &str,
    decode: &impl Fn(&Value) -> Result<(), String>,
) -> Option<Result<Value, (Value, String)>> {
    property_pointers(sample)
        .into_iter()
        .find_map(|(parent, key)| {
            let mut trial = sample.clone();
            trial.pointer_mut(&parent)?.as_object_mut()?.remove(&key);
            match decode(&trial) {
                Ok(()) => Some(Ok(trial)),
                Err(next) if next != error && !next.starts_with("missing field") => {
                    Some(Err((trial, next)))
                }
                Err(_) => None,
            }
        })
}

/// Whether stepping a leaf to `value` moved the error past the failing leaf rather than
/// breaking a leaf that was fine: a serde type error, or an error that quotes the new
/// value, names the stepped leaf itself.
fn moved(error: &str, next: &str, value: Option<&Value>) -> bool {
    let quotes_value = match value {
        Some(Value::String(text)) if text.len() > 1 => next.contains(text.as_str()),
        Some(Value::Number(number)) if number.as_u64() != Some(1) => {
            next.contains(&number.to_string())
        }
        _ => false,
    };
    next != error
        && !next.starts_with("invalid type")
        && !next.starts_with("invalid value")
        && !quotes_value
}

/// How many rungs a placeholder ladder has.
const LADDER_RUNGS: usize = 2;

/// Set the placeholder at `pointer` to its value on `rung`. The empty string becomes `1` (a
/// non-empty id, a canonical decimal), then 32 bytes of lowercase hex; integer zero becomes
/// one, then the Q32 denominator `2^32`.
fn step(sample: &mut Value, pointer: &str, rung: usize) {
    let Some(leaf) = sample.pointer_mut(pointer) else {
        return;
    };
    let next = match leaf {
        Value::String(_) => [Value::from("1"), Value::from("0".repeat(64))][rung].clone(),
        _ => [Value::from(1_u64), Value::from(1_u64 << 32)][rung].clone(),
    };
    *leaf = next;
}

/// A leaf below the top of its ladder: the empty string or `1`, integer zero or one. A step
/// only ever raises a leaf's rung, so the repair terminates.
fn is_placeholder(leaf: &Value) -> bool {
    match leaf {
        Value::String(text) => text.is_empty() || text == "1",
        Value::Number(number) => matches!(number.as_u64(), Some(0 | 1)),
        _ => false,
    }
}

/// `(parent pointer, key)` of every object property, in document order.
fn property_pointers(value: &Value) -> Vec<(String, String)> {
    let mut found = Vec::new();
    collect_properties(value, String::new(), &mut found);
    found
}

fn collect_properties(value: &Value, pointer: String, found: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                found.push((pointer.clone(), key.clone()));
                collect_properties(child, child_pointer(&pointer, key), found);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_properties(child, format!("{pointer}/{index}"), found);
            }
        }
        _ => {}
    }
}

fn child_pointer(pointer: &str, key: &str) -> String {
    format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// JSON pointers of every empty-string and zero-integer leaf, in document order.
fn placeholder_leaves(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect(value, String::new(), &mut found);
    found
}

fn collect(value: &Value, pointer: String, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                collect(child, child_pointer(&pointer, key), found);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect(child, format!("{pointer}/{index}"), found);
            }
        }
        leaf if is_placeholder(leaf) => found.push(pointer),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::repair;
    use serde_json::{json, Value};

    /// A decoder that wants a non-empty id, a 64-char hex digest and a non-zero denominator,
    /// and reports the first failure only.
    fn decode(value: &Value) -> Result<(), String> {
        if value["id"] == "" {
            return Err("id must be non-empty".into());
        }
        if value["digest"].as_str().map(str::len) != Some(64) {
            return Err("digest must be 32 bytes of hex".into());
        }
        if value["other_digest"].as_str().map(str::len) != Some(64) {
            return Err("digest must be 32 bytes of hex".into());
        }
        if value["ratio"]["denominator"] == 0 {
            return Err("denominator must be non-zero".into());
        }
        Ok(())
    }

    // spec: MEDULLA-CONTRACT-R001.3
    #[test]
    fn repair_satisfies_decoder_constraints_the_schema_cannot_state() {
        let built = json!({
            "id": "", "digest": "", "other_digest": "", "note": "",
            "ratio": {"numerator": 0, "denominator": 0}
        });
        let repaired = repair(built, decode).unwrap();
        assert_eq!(repaired["id"], "1");
        assert_eq!(repaired["digest"], "0".repeat(64));
        assert_eq!(repaired["ratio"]["denominator"], 1);
        let cross_field = |value: &Value| match (value.get("current"), value.get("previous")) {
            (None, _) => Err("missing field `current`".to_string()),
            (Some(current), Some(previous)) if current == previous => Err("must differ".into()),
            _ => Ok(()),
        };
        assert_eq!(
            repair(json!({"current": "a", "previous": "a"}), cross_field),
            Ok(json!({"current": "a"}))
        );
        assert_eq!(
            repair(json!({"id": "x", "digest": true}), decode),
            Err("digest must be 32 bytes of hex".into())
        );
    }
}
