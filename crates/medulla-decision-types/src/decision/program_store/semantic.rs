//! Program phrasings, the match index and the bound match
//! (MEDULLA-DECISION-ENGINE-R135.16.1).
//!
//! Types only: a compiled program may carry up to [`MAX_PHRASINGS`] phrasings
//! of the request it answers, `Decide` with `QuestionKind::ProgramLookup`
//! finds the nearest program by request text, and a [`ProgramMatch`] is the
//! program with its parameters bound to values. Nothing here reads a store.

use super::super::run_governance::{StepKind, TraceScalar};
use medulla_types::contract::BoundedVec;
use serde::{Deserialize, Serialize};

/// Current wire schema version of [`ProgramMatchIndex`] and [`ProgramMatch`].
pub const MATCH_SCHEMA_VERSION: u32 = 1;
/// Most phrasings one program carries.
pub const MAX_PHRASINGS: usize = 16;
/// Most bytes of one phrasing.
pub const MAX_PHRASING_BYTES: usize = 512;
/// Most bytes of the request text of a lookup.
pub const MAX_REQUEST_TEXT_BYTES: usize = 4096;
/// The ontology class of one phrasing node.
pub const PHRASING_NODE_CLASS: &str = "ProgramPhrasing";
/// The edge from a phrasing node to its program node.
pub const PHRASING_OF: &str = "phrasing_of";
/// The lookup parameter that carries the request text.
pub const PARAM_REQUEST_TEXT: &str = "request_text";
/// The lookup parameter that names the graph the program nodes live in.
pub const PARAM_PROGRAM_GRAPH: &str = "program_graph";
/// The lookup parameter that carries the candidate hits.
pub const PARAM_PROGRAM_CANDIDATES: &str = "program_candidates";
/// The prefix of the answer a program match is carried under.
pub const MATCH_ANSWER_PREFIX: &str = "program_match:";

/// One phrasing of the request a program answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProgramPhrasing {
    pub text: String,
}

/// One argument of a step template: the parameter it is bound from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TemplateArgument {
    pub name: String,
    pub parameter: String,
}

/// One step of a program with its arguments left as parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepTemplate {
    pub index: u32,
    pub kind: StepKind,
    pub server: String,
    pub tool: String,
    pub arguments: BoundedVec<TemplateArgument, 32>,
}

/// A parameter the program fixes to one value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FixedValue {
    pub parameter: String,
    pub value: TraceScalar,
}

/// What the match index holds for one stored program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProgramMatchIndex {
    pub schema_version: u32,
    pub task_signature: String,
    pub program_digest: String,
    pub threshold_micros: Option<i64>,
    pub phrasings: BoundedVec<ProgramPhrasing, 16>,
    pub slots: BoundedVec<String, 64>,
    pub fixed: BoundedVec<FixedValue, 64>,
    pub steps: BoundedVec<StepTemplate, 40>,
    pub executable: bool,
    pub space_digest: Option<String>,
    pub embedded_nodes: BoundedVec<String, 17>,
}

/// How a request matched a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MatchKind {
    Exact,
    Semantic,
}

/// Where a bound value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BindingSource {
    Slot,
    Fixed,
}

/// One parameter bound to a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Binding {
    pub parameter: String,
    pub source: BindingSource,
    pub value: TraceScalar,
}

/// One argument of a bound step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BoundArgument {
    pub name: String,
    pub value: TraceScalar,
}

/// One step of a matched program with its arguments bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BoundStep {
    pub index: u32,
    pub server: String,
    pub tool: String,
    pub arguments: BoundedVec<BoundArgument, 32>,
}

/// A matched program with its parameters bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProgramMatch {
    pub schema_version: u32,
    pub task_signature: String,
    pub program_digest: String,
    pub match_kind: MatchKind,
    pub score_micros: Option<i64>,
    pub threshold_micros: i64,
    pub matched_node: Option<String>,
    pub bindings: BoundedVec<Binding, 64>,
    pub steps: BoundedVec<BoundStep, 40>,
}

/// One nearest-phrasing hit of a candidate search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProgramHit {
    pub node_id: String,
    pub task_signature: String,
    pub score_micros: i64,
}

/// The candidates a lookup found, with how they were found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProgramCandidates {
    pub space_digest: Option<String>,
    pub embedding_calls: u32,
    pub hits: BoundedVec<ProgramHit, 64>,
    pub unavailable: Option<String>,
}

/// The pointer key of the match stored for `task_signature`.
pub fn match_key(task_signature: &str) -> String {
    format!("run_record/program_match/{task_signature}")
}

/// The graph node id of phrasing `k` of the program of `task_signature`.
pub fn phrasing_node_id(task_signature: &str, k: usize) -> String {
    format!("program:{task_signature}#phrasing:{k}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    fn bounded<T, const N: usize>(values: Vec<T>) -> BoundedVec<T, N> {
        BoundedVec::new(values).unwrap()
    }

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) {
        let wire = serde_json::to_value(value).unwrap();
        assert_eq!(&serde_json::from_value::<T>(wire.clone()).unwrap(), value);
        // Every type refuses a field it does not declare.
        let serde_json::Value::Object(mut object) = wire else {
            return;
        };
        object.insert("surprise".into(), serde_json::json!(1));
        assert!(serde_json::from_value::<T>(serde_json::Value::Object(object)).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.16.1
    #[test]
    fn match_types_round_trip() {
        round_trip(&ProgramPhrasing { text: "book a table".into() });
        round_trip(&TemplateArgument { name: "city".into(), parameter: "p0".into() });
        let step = StepTemplate {
            index: 1,
            kind: StepKind::ToolCall,
            server: "maps".into(),
            tool: "search".into(),
            arguments: bounded(vec![TemplateArgument { name: "q".into(), parameter: "p0".into() }]),
        };
        round_trip(&step);
        let fixed = FixedValue { parameter: "p1".into(), value: TraceScalar::Int(3) };
        round_trip(&fixed);
        let index = ProgramMatchIndex {
            schema_version: MATCH_SCHEMA_VERSION,
            task_signature: "sig".into(),
            program_digest: "ab".repeat(32),
            threshold_micros: Some(850_000),
            phrasings: bounded(vec![ProgramPhrasing { text: "x".into() }]),
            slots: bounded(vec!["p0".into()]),
            fixed: bounded(vec![fixed]),
            steps: bounded(vec![step]),
            executable: true,
            space_digest: Some("cd".repeat(32)),
            embedded_nodes: bounded(vec!["program:sig#phrasing:0".into()]),
        };
        round_trip(&index);
        let binding = Binding {
            parameter: "p0".into(),
            source: BindingSource::Slot,
            value: TraceScalar::Text("Paris".into()),
        };
        round_trip(&binding);
        let bound = BoundStep {
            index: 1,
            server: "maps".into(),
            tool: "search".into(),
            arguments: bounded(vec![BoundArgument { name: "q".into(), value: TraceScalar::Bool(true) }]),
        };
        round_trip(&bound.arguments.as_slice()[0]);
        round_trip(&bound);
        round_trip(&ProgramMatch {
            schema_version: MATCH_SCHEMA_VERSION,
            task_signature: "sig".into(),
            program_digest: "ab".repeat(32),
            match_kind: MatchKind::Semantic,
            score_micros: Some(900_000),
            threshold_micros: 850_000,
            matched_node: Some("program:sig#phrasing:0".into()),
            bindings: bounded(vec![binding]),
            steps: bounded(vec![bound]),
        });
        let hit = ProgramHit { node_id: "n".into(), task_signature: "sig".into(), score_micros: 1 };
        round_trip(&hit);
        round_trip(&ProgramCandidates {
            space_digest: None,
            embedding_calls: 1,
            hits: bounded(vec![hit]),
            unavailable: None,
        });
        assert_eq!(serde_json::to_value(MatchKind::Exact).unwrap(), "exact");
        assert_eq!(serde_json::to_value(BindingSource::Fixed).unwrap(), "fixed");
        assert!(serde_json::from_value::<MatchKind>(serde_json::json!("fuzzy")).is_err());
        assert_eq!(match_key("sig"), "run_record/program_match/sig");
        assert_eq!(phrasing_node_id("sig", 2), "program:sig#phrasing:2");
        // A commit body without phrasings decodes to an empty list.
        let body = serde_json::json!({
            "kind": "compiled_program",
            "trace": serde_json::Value::Null,
        });
        assert!(body.get("phrasings").is_none());
        let empty: BoundedVec<ProgramPhrasing, 16> = serde_json::from_value(serde_json::json!([])).unwrap();
        assert!(empty.is_empty());
        let too_many = vec![serde_json::json!({"text": "x"}); 17];
        assert!(serde_json::from_value::<BoundedVec<ProgramPhrasing, 16>>(serde_json::Value::Array(too_many)).is_err());
    }
}
