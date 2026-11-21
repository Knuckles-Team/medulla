//! Deterministic smallest-valid sample builder over JSON Schema (MEDULLA-CONTRACT-R001.3).
//!
//! The builder is a pure function of the schema: it reads no clock, no random source and no
//! environment. It emits every property it can build, required and optional (a decoder may
//! require a field its schema leaves optional; an optional property that cannot be built is
//! omitted), the schema `minimum` (or 0) for numbers, the shortest string that satisfies
//! `minLength` and `pattern`, `false` for booleans,
//! `minItems` elements for arrays, an empty map, the first listed alternative of an `enum`,
//! `oneOf` or `anyOf`, and the `const` value for `const`.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

/// Deepest chain of resolved `$ref` hops on one path before the builder gives up.
pub const MAX_SAMPLE_DEPTH: usize = 8;

/// Why a sample could not be built. Every variant names the method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SampleBuildError {
    /// The reference chain on one path is deeper than [`MAX_SAMPLE_DEPTH`], or cycles.
    DepthExceeded { method: String },
    /// A `$ref` names no definition in the root schema.
    UnresolvedRef { method: String, reference: String },
    /// A constraint no value can satisfy; `path` is the JSON pointer of the node.
    Unsatisfiable { method: String, path: String },
}

impl std::fmt::Display for SampleBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DepthExceeded { method } => write!(f, "{method}: sample depth exceeded"),
            Self::UnresolvedRef { method, reference } => {
                write!(f, "{method}: unresolved reference {reference}")
            }
            Self::Unsatisfiable { method, path } => {
                write!(f, "{method}: unsatisfiable constraint at {path:?}")
            }
        }
    }
}

impl std::error::Error for SampleBuildError {}

/// Build the smallest valid value of the whole `schema`, resolving `$ref` against its root.
pub fn build_sample(schema: &schemars::Schema, method: &str) -> Result<Value, SampleBuildError> {
    let root = schema.as_value();
    build_node(root, root, method)
}

/// Build the smallest valid value of `node`, resolving `$ref` against `root`. Used for one
/// variant of an enum schema, where the root is the whole enum.
pub fn build_node(root: &Value, node: &Value, method: &str) -> Result<Value, SampleBuildError> {
    build_node_with(root, node, method, &|_| None)
}

/// A per-type sample hook: the sample of the `$ref` it names, built by the type's own
/// constructor, for a type whose decoder checks a rule no schema can state (a digest over
/// its own fields). `None` builds the type from its schema.
pub type SampleHook<'a> = &'a dyn Fn(&str) -> Option<Value>;

/// [`build_node`], with `hook` consulted at every `$ref` before the schema.
pub fn build_node_with(
    root: &Value,
    node: &Value,
    method: &str,
    hook: SampleHook<'_>,
) -> Result<Value, SampleBuildError> {
    let mut walk = Walk {
        root,
        method,
        hook,
        refs: Vec::new(),
    };
    walk.node(node, "")
}

struct Walk<'a> {
    root: &'a Value,
    method: &'a str,
    hook: SampleHook<'a>,
    /// The `$ref` targets on the current path, outermost first.
    refs: Vec<String>,
}

impl Walk<'_> {
    fn unsatisfiable(&self, path: &str) -> SampleBuildError {
        SampleBuildError::Unsatisfiable {
            method: self.method.to_string(),
            path: path.to_string(),
        }
    }

    fn node(&mut self, node: &Value, path: &str) -> Result<Value, SampleBuildError> {
        let object = match node {
            Value::Bool(true) => return Ok(Value::Null),
            Value::Bool(false) => return Err(self.unsatisfiable(path)),
            Value::Object(object) => object,
            _ => return Err(self.unsatisfiable(path)),
        };
        if let Some(composite) = self.composite(object, path) {
            return self.with_siblings(composite?, object, path);
        }
        if let Some(value) = object.get("const") {
            return Ok(value.clone());
        }
        if let Some(first) = object
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
        {
            return Ok(first.clone());
        }
        match schema_type(object) {
            Some("null") | None if !looks_structured(object) => Ok(Value::Null),
            Some("boolean") => Ok(Value::Bool(false)),
            Some("integer") => Ok(self.number(object, true)),
            Some("number") => Ok(self.number(object, false)),
            Some("string") => self.string(object, path),
            Some("array") => self.array(object, path),
            Some("object") => self.object(object, path),
            _ if object.contains_key("items") || object.contains_key("prefixItems") => {
                self.array(object, path)
            }
            _ => self.object(object, path),
        }
    }

    /// A `$ref`, `oneOf`, `anyOf` or `allOf` node, or `None` when the node has none.
    fn composite(
        &mut self,
        object: &Map<String, Value>,
        path: &str,
    ) -> Option<Result<Value, SampleBuildError>> {
        if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
            return Some(self.reference(reference, path));
        }
        for keyword in ["oneOf", "anyOf"] {
            if let Some(alternatives) = object.get(keyword).and_then(Value::as_array) {
                return Some(self.first_alternative(alternatives, path, keyword));
            }
        }
        let parts = object.get("allOf").and_then(Value::as_array)?;
        Some(self.all_of(parts, path))
    }

    /// Merge the sibling `properties` of a composite node (an internally tagged enum puts
    /// its tag beside the variant's `$ref` or `oneOf`) into the composite's object value.
    fn with_siblings(
        &mut self,
        composite: Value,
        object: &Map<String, Value>,
        path: &str,
    ) -> Result<Value, SampleBuildError> {
        let Value::Object(mut merged) = composite else {
            return Ok(composite);
        };
        if !object.contains_key("properties") && !object.contains_key("required") {
            return Ok(Value::Object(merged));
        }
        if let Value::Object(siblings) = self.object(object, path)? {
            merged.extend(siblings);
        }
        Ok(Value::Object(merged))
    }

    fn reference(&mut self, reference: &str, path: &str) -> Result<Value, SampleBuildError> {
        if let Some(sample) = (self.hook)(reference) {
            return Ok(sample);
        }
        let Some(pointer) = reference.strip_prefix('#') else {
            return Err(SampleBuildError::UnresolvedRef {
                method: self.method.to_string(),
                reference: reference.to_string(),
            });
        };
        let Some(target) = self.root.pointer(pointer) else {
            return Err(SampleBuildError::UnresolvedRef {
                method: self.method.to_string(),
                reference: reference.to_string(),
            });
        };
        if self.refs.len() >= MAX_SAMPLE_DEPTH || self.refs.iter().any(|seen| seen == reference) {
            return Err(SampleBuildError::DepthExceeded {
                method: self.method.to_string(),
            });
        }
        self.refs.push(reference.to_string());
        let built = self.node(target, path);
        self.refs.pop();
        built
    }

    /// The first listed alternative that builds; a recursive first alternative falls
    /// through to the next one so a recursive type still has a finite sample.
    fn first_alternative(
        &mut self,
        alternatives: &[Value],
        path: &str,
        keyword: &str,
    ) -> Result<Value, SampleBuildError> {
        let mut last = self.unsatisfiable(path);
        for (index, alternative) in alternatives.iter().enumerate() {
            match self.node(alternative, &format!("{path}/{keyword}/{index}")) {
                Ok(value) => return Ok(value),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    fn all_of(&mut self, parts: &[Value], path: &str) -> Result<Value, SampleBuildError> {
        let mut merged: Option<Value> = None;
        for (index, part) in parts.iter().enumerate() {
            let built = self.node(part, &format!("{path}/allOf/{index}"))?;
            merged = Some(match (merged, built) {
                (Some(Value::Object(mut left)), Value::Object(right)) => {
                    left.extend(right);
                    Value::Object(left)
                }
                (None, built) => built,
                (Some(first), _) => first,
            });
        }
        merged.ok_or_else(|| self.unsatisfiable(path))
    }

    fn number(&self, object: &Map<String, Value>, integer: bool) -> Value {
        if let Some(minimum) = object.get("minimum").filter(|v| v.is_number()) {
            return minimum.clone();
        }
        if let Some(exclusive) = object.get("exclusiveMinimum").and_then(Value::as_f64) {
            return if integer {
                json!(exclusive.floor() as i64 + 1)
            } else {
                json!(exclusive + 1.0)
            };
        }
        if integer {
            json!(0)
        } else {
            json!(0.0)
        }
    }

    fn string(&self, object: &Map<String, Value>, path: &str) -> Result<Value, SampleBuildError> {
        let min = object.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
        // A string declared `x-medulla-nonblank` needs at least one character; the
        // decoder's runtime check is no schema constraint, so the builder states it.
        let min = if object
            .get("x-medulla-nonblank")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            min.max(1)
        } else {
            min
        };
        if let Some(max) = object.get("maxLength").and_then(Value::as_u64) {
            if (min as u64) > max {
                return Err(self.unsatisfiable(path));
            }
        }
        let Some(pattern) = object.get("pattern").and_then(Value::as_str) else {
            return Ok(Value::String("a".repeat(min)));
        };
        let regex = regex::Regex::new(pattern).map_err(|_| self.unsatisfiable(path))?;
        let max = object
            .get("maxLength")
            .and_then(Value::as_u64)
            .map_or(usize::MAX, |max| max as usize);
        // Shortest first, over a fixed alphabet of fillers: deterministic and finite.
        for length in min..=min.saturating_add(8).min(max) {
            for filler in ["a", "0", "A", "_", "-", "."] {
                let candidate = filler.repeat(length);
                if regex.is_match(&candidate) {
                    return Ok(Value::String(candidate));
                }
            }
        }
        if let Some(candidate) = super::pattern_sample::synthesize(pattern) {
            let length = candidate.chars().count();
            if length >= min && length <= max && regex.is_match(&candidate) {
                return Ok(Value::String(candidate));
            }
        }
        for candidate in [
            "a0", "a_b", "a.b", "a-b", "a/b", "a:b", "a@b", "0.0.0", "1.0",
        ] {
            let length = candidate.chars().count();
            if length >= min && length <= max && regex.is_match(candidate) {
                return Ok(Value::String(candidate.to_string()));
            }
        }
        Err(self.unsatisfiable(path))
    }

    fn array(
        &mut self,
        object: &Map<String, Value>,
        path: &str,
    ) -> Result<Value, SampleBuildError> {
        let min = object.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
        if let Some(max) = object.get("maxItems").and_then(Value::as_u64) {
            if (min as u64) > max {
                return Err(self.unsatisfiable(path));
            }
        }
        let prefix = object
            .get("prefixItems")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut items = Vec::with_capacity(min);
        for index in 0..min {
            let element = match prefix.get(index).or_else(|| object.get("items")) {
                Some(schema) => self.node(schema, &format!("{path}/items/{index}"))?,
                None => Value::Null,
            };
            items.push(element);
        }
        Ok(Value::Array(items))
    }

    fn object(
        &mut self,
        object: &Map<String, Value>,
        path: &str,
    ) -> Result<Value, SampleBuildError> {
        let required: BTreeSet<&str> = object
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut built = Map::new();
        for name in &required {
            let property = object
                .get("properties")
                .and_then(|properties| properties.get(*name))
                .or_else(|| object.get("additionalProperties").filter(|v| v.is_object()));
            let value = match property {
                Some(schema) => self.node(schema, &format!("{path}/properties/{name}"))?,
                None => Value::Null,
            };
            built.insert(name.to_string(), value);
        }
        self.optional_properties(object, &required, path, &mut built);
        Ok(Value::Object(built))
    }

    /// Every optional property that builds; one that cannot be built is left out.
    fn optional_properties(
        &mut self,
        object: &Map<String, Value>,
        required: &BTreeSet<&str>,
        path: &str,
        built: &mut Map<String, Value>,
    ) {
        let Some(properties) = object.get("properties").and_then(Value::as_object) else {
            return;
        };
        for (name, schema) in properties {
            if required.contains(name.as_str()) {
                continue;
            }
            if let Ok(value) = self.node(schema, &format!("{path}/properties/{name}")) {
                built.insert(name.clone(), value);
            }
        }
    }
}

/// The first listed type of a `type` keyword, which is a string or an array of strings.
fn schema_type(object: &Map<String, Value>) -> Option<&str> {
    match object.get("type")? {
        Value::String(name) => Some(name),
        Value::Array(names) => names.iter().filter_map(Value::as_str).next(),
        _ => None,
    }
}

/// An untyped schema that still constrains an object or an array.
fn looks_structured(object: &Map<String, Value>) -> bool {
    ["properties", "required", "items", "prefixItems"]
        .iter()
        .any(|keyword| object.contains_key(*keyword))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(value: Value) -> schemars::Schema {
        schemars::Schema::try_from(value).expect("a JSON object is a valid schema")
    }

    // spec: MEDULLA-CONTRACT-R001.3
    #[test]
    fn builder_emits_minimal_values_and_refuses_unsatisfiable_schemas() {
        let root = schema(json!({
            "type": "object",
            "required": ["n", "x", "s", "p", "b", "v", "m", "tag", "e", "r"],
            "properties": {
                "n": {"type": "integer", "minimum": 3},
                "x": {"type": "number"},
                "s": {"type": "string", "minLength": 2},
                "p": {"type": "string", "pattern": "^[0-9]+$"},
                "b": {"type": "boolean"},
                "v": {"type": "array", "minItems": 2, "items": {"type": "integer"}},
                "m": {"type": "object", "additionalProperties": {"type": "string"}},
                "tag": {"const": "fixed"},
                "e": {"oneOf": [{"enum": ["first", "second"]}, {"type": "null"}]},
                "r": {"$ref": "#/$defs/Leaf"},
                "unused": {"type": "string"}
            },
            "$defs": {"Leaf": {"type": "object", "required": ["a"], "properties": {"a": {"type": "boolean"}}}}
        }));
        let sample = build_sample(&root, "Demo").unwrap();
        assert_eq!(
            sample,
            json!({
                "n": 3, "x": 0.0, "s": "aa", "p": "0", "b": false, "v": [0, 0], "m": {},
                "tag": "fixed", "e": "first", "r": {"a": false}, "unused": ""
            })
        );
        assert_eq!(build_sample(&root, "Demo").unwrap(), sample);

        let cyclic = schema(json!({
            "$ref": "#/$defs/Node",
            "$defs": {"Node": {"type": "object", "required": ["next"], "properties": {"next": {"$ref": "#/$defs/Node"}}}}
        }));
        assert_eq!(
            build_sample(&cyclic, "Demo"),
            Err(SampleBuildError::DepthExceeded {
                method: "Demo".into()
            })
        );
        let missing = schema(json!({"$ref": "#/$defs/Nowhere"}));
        assert_eq!(
            build_sample(&missing, "Demo"),
            Err(SampleBuildError::UnresolvedRef {
                method: "Demo".into(),
                reference: "#/$defs/Nowhere".into()
            })
        );
        let prefixed = schema(json!({"type": "string", "pattern": "^v1:[0-9a-f]{64}$"}));
        assert_eq!(
            build_sample(&prefixed, "Demo").unwrap(),
            json!(format!("v1:{}", "0".repeat(64)))
        );
        let tagged = schema(json!({
            "$ref": "#/$defs/Leaf",
            "required": ["family"],
            "properties": {"family": {"const": "user"}},
            "$defs": {"Leaf": {"type": "object", "required": ["a"], "properties": {"a": {"type": "boolean"}}}}
        }));
        assert_eq!(
            build_sample(&tagged, "Demo").unwrap(),
            json!({"a": false, "family": "user"})
        );
        let impossible = schema(json!({"type": "string", "minLength": 4, "maxLength": 2}));
        assert!(matches!(
            build_sample(&impossible, "Demo"),
            Err(SampleBuildError::Unsatisfiable { .. })
        ));
    }
}
