//! Typed tool steps and their bindings (MEDULLA-DECISION-ENGINE-R134.111.8 to .11).
//!
//! A call is a caller claim. The engine canonicalizes and statically checks it;
//! it never invokes the tool, never infers an edge from a binding and never
//! checks that a referenced output path exists at run time.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{error, valid_id, TaskPlanTask};

pub const MAX_TOOL_STEPS: usize = 64;
pub const MAX_BINDINGS_PER_CALL: usize = 32;
pub const MAX_CALL_ARGUMENT_BYTES: usize = 16 * 1024;
pub const MAX_BINDING_PATH_SEGMENTS: usize = 16;
pub const MAX_BINDING_PATH_BYTES: usize = 256;
pub const BINDING_KEY: &str = "$ref";

/// A tool step maps over at most this many items (MEDULLA-DECISION-ENGINE-R134.111.19).
pub const MAX_FOREACH_ITEMS: u32 = 256;
/// A plan holds at most this many foreach steps.
pub const MAX_FOREACH_STEPS: usize = 8;
/// A plan expands to at most this many calls: each foreach step counts its `max_items` and
/// every other tool step counts one.
pub const MAX_PLAN_EXPANDED_CALLS: usize = 1024;
/// The reference that names the zero-based index of the current item.
pub const ITEM_INDEX_REFERENCE: &str = "item_index";
/// The prefix of a reference that names a path of the current item.
const ITEM_REFERENCE_PREFIX: &str = "item.";

/// A task has `foreach` and no `call`.
pub const FOREACH_WITHOUT_CALL: &str = "FOREACH_WITHOUT_CALL";
/// The foreach source is not a binding, or not in the direct `depends_on`.
pub const FOREACH_SOURCE_INVALID: &str = "FOREACH_SOURCE_INVALID";
/// `max_items` is outside 1 to 256, or a count of the plan is exceeded.
pub const FOREACH_BOUNDS: &str = "FOREACH_BOUNDS";
/// `item.` or `item_index` appears in a task without `foreach`.
pub const ITEM_REF_OUTSIDE_FOREACH: &str = "ITEM_REF_OUTSIDE_FOREACH";
/// A foreach source reads another foreach step's expanded output.
pub const FOREACH_NESTED: &str = "FOREACH_NESTED";

/// The source of a foreach step: `{"$ref": "steps.<task_id>.output[.<json.path>]"}`, naming a
/// JSON array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ForeachSource {
    #[serde(rename = "$ref")]
    pub reference: String,
}

/// A foreach step: the call of the task runs once per item of the source array, at most
/// `max_items` times. The engine validates the bounds and the bindings and never runs a step;
/// Arbora runs the expansion (MEDULLA-DECISION-ENGINE-R134.111.19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskPlanForeach {
    pub source: ForeachSource,
    /// An integer from 1 to [`MAX_FOREACH_ITEMS`]; any other value refuses `FOREACH_BOUNDS`.
    pub max_items: u32,
}

/// One deterministic tool call. `arguments` is a JSON object whose values are
/// literals or `{"$ref": "steps.<task_id>.output.<json.path>"}` bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskPlanCall {
    pub server: String,
    pub tool: String,
    pub arguments: Value,
}

/// One parsed binding: the source task, its output path and the JSON pointer
/// of the argument that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPlanBinding {
    pub pointer: String,
    pub source_task: String,
    pub reference: String,
}

fn refuse(code: &str, task: &str, pointer: &str) -> String {
    error(code, format!("task={task} pointer={pointer}"))
}

fn pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

fn valid_key_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Normalize one path segment. A digit run is an array index and loses its
/// leading zeros, so `07` and `7` are the same reference.
fn canonical_segment(segment: &str) -> Option<String> {
    if !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()) {
        let trimmed = segment.trim_start_matches('0');
        return Some(if trimmed.is_empty() { "0" } else { trimmed }.to_string());
    }
    valid_key_segment(segment).then(|| segment.to_string())
}

/// Parse `steps.<task_id>.output[.<path>]` into its task id and canonical text.
pub(super) fn parse_reference(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    if text.len() > MAX_BINDING_PATH_BYTES {
        return None;
    }
    let rest = text.strip_prefix("steps.")?;
    let (task, path) = match rest.split_once(".output") {
        Some((task, path)) if valid_id(task) && !task.contains('.') => (task, path),
        _ => return None,
    };
    let mut canonical = format!("steps.{task}.output");
    if !path.is_empty() {
        let path = path.strip_prefix('.')?;
        let segments: Vec<&str> = path.split('.').collect();
        if segments.len() > MAX_BINDING_PATH_SEGMENTS {
            return None;
        }
        for segment in segments {
            canonical.push('.');
            canonical.push_str(&canonical_segment(segment)?);
        }
    }
    Some((task.to_string(), canonical))
}

struct Walk<'a> {
    task: &'a str,
    /// Whether the task is a foreach step, where `item.` references are legal.
    foreach: bool,
    bindings: Vec<TaskPlanBinding>,
}

/// Whether `text` is an item reference: `item_index` or `item.<path>`.
fn is_item_reference(text: &str) -> bool {
    let text = text.trim();
    text == ITEM_INDEX_REFERENCE || text.starts_with(ITEM_REFERENCE_PREFIX)
}

/// Normalize an item reference with the same path grammar and bounds as a step reference.
fn canonical_item_reference(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() > MAX_BINDING_PATH_BYTES {
        return None;
    }
    if text == ITEM_INDEX_REFERENCE {
        return Some(text.to_string());
    }
    let path = text.strip_prefix(ITEM_REFERENCE_PREFIX)?;
    let segments: Vec<&str> = path.split('.').collect();
    if segments.len() > MAX_BINDING_PATH_SEGMENTS {
        return None;
    }
    let mut canonical = String::from("item");
    for segment in segments {
        canonical.push('.');
        canonical.push_str(&canonical_segment(segment)?);
    }
    Some(canonical)
}

impl Walk<'_> {
    fn value(&mut self, value: &Value, pointer: &str) -> Result<Value, String> {
        match value {
            Value::Object(map) if map.contains_key(BINDING_KEY) => self.binding(map, pointer),
            Value::Object(map) => self.object(map, pointer),
            Value::Array(items) => items
                .iter()
                .enumerate()
                .map(|(i, item)| self.value(item, &format!("{pointer}/{i}")))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            literal => Ok(literal.clone()),
        }
    }

    fn object(&mut self, map: &Map<String, Value>, pointer: &str) -> Result<Value, String> {
        let sorted: BTreeMap<&String, &Value> = map.iter().collect();
        let mut out = Map::new();
        for (key, item) in sorted {
            let child = format!("{pointer}/{}", pointer_token(key));
            out.insert(key.clone(), self.value(item, &child)?);
        }
        Ok(Value::Object(out))
    }

    fn binding(&mut self, map: &Map<String, Value>, pointer: &str) -> Result<Value, String> {
        let pointer = if pointer.is_empty() { "/" } else { pointer };
        if let (1, Value::String(text)) = (map.len(), &map[BINDING_KEY]) {
            if is_item_reference(text) {
                // An item reference reads the current item; it is no binding and adds no edge.
                if !self.foreach {
                    return Err(refuse(ITEM_REF_OUTSIDE_FOREACH, self.task, pointer));
                }
                let Some(canonical) = canonical_item_reference(text) else {
                    return Err(refuse("BINDING_BAD_PATH", self.task, pointer));
                };
                let mut out = Map::new();
                out.insert(BINDING_KEY.to_string(), Value::String(canonical));
                return Ok(Value::Object(out));
            }
        }
        let parsed = match (map.len(), &map[BINDING_KEY]) {
            (1, Value::String(text)) => parse_reference(text),
            _ => None,
        };
        let Some((source_task, reference)) = parsed else {
            return Err(refuse("BINDING_BAD_PATH", self.task, pointer));
        };
        self.bindings.push(TaskPlanBinding {
            pointer: pointer.to_string(),
            source_task,
            reference: reference.clone(),
        });
        let mut out = Map::new();
        out.insert(BINDING_KEY.to_string(), Value::String(reference));
        Ok(Value::Object(out))
    }
}

/// Canonicalize one call: sorted keys, no whitespace, one reference form.
/// Returns the canonical call, its bindings and the call's text byte count.
pub(super) fn canonical_call(
    task: &str,
    call: &TaskPlanCall,
    foreach: bool,
) -> Result<(TaskPlanCall, Vec<TaskPlanBinding>, usize), String> {
    let object = call.arguments.as_object();
    if !valid_id(&call.server)
        || !valid_id(&call.tool)
        || object.is_none_or(|map| map.contains_key(BINDING_KEY))
    {
        return Err(refuse("CALL_INVALID", task, ""));
    }
    let mut walk = Walk {
        task,
        foreach,
        bindings: Vec::new(),
    };
    let arguments = walk.value(&call.arguments, "")?;
    let argument_bytes = serde_json::to_vec(&arguments)
        .map_err(|e| error("CALL_INVALID", e))?
        .len();
    if argument_bytes > MAX_CALL_ARGUMENT_BYTES {
        return Err(refuse("CALL_BOUNDS", task, ""));
    }
    if walk.bindings.len() > MAX_BINDINGS_PER_CALL {
        return Err(refuse("CALL_BOUNDS", task, &walk.bindings[0].pointer));
    }
    let bytes = call.server.len() + call.tool.len() + argument_bytes;
    let canonical = TaskPlanCall {
        server: call.server.clone(),
        tool: call.tool.clone(),
        arguments,
    };
    Ok((canonical, walk.bindings, bytes))
}

/// The bindings of a canonical task, in pointer order.
pub fn task_bindings(task: &TaskPlanTask) -> Vec<TaskPlanBinding> {
    let Some(call) = &task.call else {
        return Vec::new();
    };
    canonical_call(&task.task_id, call, task.foreach.is_some())
        .map(|(_, bindings, _)| bindings)
        .unwrap_or_default()
}

/// Refuse a task that has `foreach` and no `call`.
pub(super) fn check_foreach_has_call(task: &TaskPlanTask) -> Result<(), String> {
    if task.foreach.is_some() && task.call.is_none() {
        return Err(refuse(FOREACH_WITHOUT_CALL, &task.task_id, "/foreach"));
    }
    Ok(())
}

/// The canonical foreach of one task: the source reference in its one string form. A
/// `max_items` outside 1 to 256 refuses `FOREACH_BOUNDS`, and a source that is not a
/// `steps.<task>.output` reference refuses `FOREACH_SOURCE_INVALID`.
pub(super) fn canonical_foreach(task: &TaskPlanTask) -> Result<Option<TaskPlanForeach>, String> {
    let Some(foreach) = &task.foreach else {
        return Ok(None);
    };
    let id = task.task_id.as_str();
    let Some((_, reference)) = parse_reference(&foreach.source.reference) else {
        return Err(refuse(FOREACH_SOURCE_INVALID, id, "/foreach/source"));
    };
    if foreach.max_items == 0 || foreach.max_items > MAX_FOREACH_ITEMS {
        return Err(refuse(FOREACH_BOUNDS, id, "/foreach/max_items"));
    }
    Ok(Some(TaskPlanForeach {
        source: ForeachSource { reference },
        max_items: foreach.max_items,
    }))
}

/// Check every foreach step against the acyclic task graph: the source follows the binding
/// rules of R134.111.9 (a direct dependency, never the task itself), reads no other foreach
/// step's expanded output, and the plan counts stay within their bounds.
pub(super) fn check_foreach(tasks: &BTreeMap<String, TaskPlanTask>) -> Result<(), String> {
    let mut steps = 0usize;
    let mut expanded = 0usize;
    for task in tasks.values() {
        let id = task.task_id.as_str();
        let over = |steps: usize, expanded: usize| {
            steps > MAX_FOREACH_STEPS || expanded > MAX_PLAN_EXPANDED_CALLS
        };
        let Some(foreach) = &task.foreach else {
            expanded += usize::from(task.call.is_some());
            if over(steps, expanded) {
                return Err(refuse(FOREACH_BOUNDS, id, "/call"));
            }
            continue;
        };
        let pointer = "/foreach/source";
        let source = parse_reference(&foreach.source.reference)
            .map(|(source, _)| source)
            .ok_or_else(|| refuse(FOREACH_SOURCE_INVALID, id, pointer))?;
        if source == id
            || !tasks.contains_key(&source)
            || !task.depends_on.iter().any(|d| *d == source)
        {
            return Err(refuse(FOREACH_SOURCE_INVALID, id, pointer));
        }
        if tasks[&source].foreach.is_some() {
            return Err(refuse(FOREACH_NESTED, id, pointer));
        }
        steps += 1;
        expanded += foreach.max_items as usize;
        if over(steps, expanded) {
            return Err(refuse(FOREACH_BOUNDS, id, "/foreach"));
        }
    }
    Ok(())
}

pub(super) fn ancestors(tasks: &BTreeMap<String, TaskPlanTask>, id: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![id.to_string()];
    while let Some(next) = pending.pop() {
        for dependency in tasks[&next].depends_on.iter() {
            if seen.insert(dependency.clone()) {
                pending.push(dependency.clone());
            }
        }
    }
    seen
}

/// Check every binding against the acyclic task graph. A binding adds no
/// edge; it may only read a direct dependency's output.
pub(super) fn check_bindings(tasks: &BTreeMap<String, TaskPlanTask>) -> Result<(), String> {
    let tool_steps = tasks.values().filter(|t| t.call.is_some()).count();
    if tool_steps > MAX_TOOL_STEPS {
        return Err(error("CALL_BOUNDS", "too many tool steps"));
    }
    for task in tasks.values() {
        for binding in task_bindings(task) {
            let (id, pointer) = (task.task_id.as_str(), binding.pointer.as_str());
            if !tasks.contains_key(&binding.source_task) {
                return Err(refuse("BINDING_UNKNOWN_TASK", id, pointer));
            }
            if binding.source_task == id || ancestors(tasks, &binding.source_task).contains(id) {
                return Err(refuse("BINDING_CYCLE", id, pointer));
            }
            if !task.depends_on.iter().any(|d| *d == binding.source_task) {
                return Err(refuse("BINDING_NOT_DEPENDENCY", id, pointer));
            }
        }
    }
    Ok(())
}
