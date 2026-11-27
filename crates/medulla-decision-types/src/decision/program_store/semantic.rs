//! Program phrasings, the match index and the bound match
//! (MEDULLA-DECISION-ENGINE-R135.16.1).
//!
//! Types only: a compiled program may carry up to [`MAX_PHRASINGS`] phrasings
//! of the request it answers, `Decide` with `QuestionKind::ProgramLookup`
//! finds the nearest program by request text, and a [`ProgramMatch`] is the
//! program with its parameters bound to values. Nothing here reads a store.

use super::super::run_governance::{RunTraceRecord, StepKind, TraceScalar};
use super::{literal_walk, CompiledProgram, ProgramError, ProgramParameterKind};
use crate::decision::record::outcome::DecisionRefusal;
use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;
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
    /// The kind of each slot, in `slots` order; a capture converts to it.
    #[serde(default)]
    pub slot_kinds: BoundedVec<ProgramParameterKind, 64>,
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

/// Refusal code `PHRASING_INVALID` (MEDULLA-DECISION-ENGINE-R135.16.4).
pub const PHRASING_INVALID: RefusalCode = RefusalCode::new("PHRASING_INVALID");
/// Refusal code `PHRASING_SLOT_UNKNOWN` (MEDULLA-DECISION-ENGINE-R135.16.4).
pub const PHRASING_SLOT_UNKNOWN: RefusalCode = RefusalCode::new("PHRASING_SLOT_UNKNOWN");
/// Refusal code `PHRASING_SLOTS_ADJACENT` (MEDULLA-DECISION-ENGINE-R135.16.4).
pub const PHRASING_SLOTS_ADJACENT: RefusalCode = RefusalCode::new("PHRASING_SLOTS_ADJACENT");
/// Refusal code `PHRASING_SLOTS_DIFFER` (MEDULLA-DECISION-ENGINE-R135.16.4).
pub const PHRASING_SLOTS_DIFFER: RefusalCode = RefusalCode::new("PHRASING_SLOTS_DIFFER");
/// Refusal code `MATCH_THRESHOLD_OUT_OF_RANGE` (MEDULLA-DECISION-ENGINE-R135.16.4).
pub const MATCH_THRESHOLD_OUT_OF_RANGE: RefusalCode =
    RefusalCode::new("MATCH_THRESHOLD_OUT_OF_RANGE");
/// Refusal code `PROGRAM_EMBEDDING_SPACE_MISMATCH` (MEDULLA-DECISION-ENGINE-R135.16.4).
pub const PROGRAM_EMBEDDING_SPACE_MISMATCH: RefusalCode =
    RefusalCode::new("PROGRAM_EMBEDDING_SPACE_MISMATCH");
/// Refusal code `REQUEST_TEXT_TOO_LONG` (MEDULLA-DECISION-ENGINE-R135.16.4).
pub const REQUEST_TEXT_TOO_LONG: RefusalCode = RefusalCode::new("REQUEST_TEXT_TOO_LONG");
/// Refusal code `PROGRAM_GRAPH_REQUIRED` (MEDULLA-DECISION-ENGINE-R135.16.4).
pub const PROGRAM_GRAPH_REQUIRED: RefusalCode = RefusalCode::new("PROGRAM_GRAPH_REQUIRED");
/// Refusal code `PROGRAM_CANDIDATES_RESERVED` (MEDULLA-DECISION-ENGINE-R135.16.4).
pub const PROGRAM_CANDIDATES_RESERVED: RefusalCode =
    RefusalCode::new("PROGRAM_CANDIDATES_RESERVED");
/// Abstain reason: the best score is below the program's threshold.
pub const PROGRAM_BELOW_THRESHOLD: RefusalCode = RefusalCode::new("PROGRAM_BELOW_THRESHOLD");
/// Abstain reason: no phrasing fits the request text, or a capture does not convert.
pub const PROGRAM_SLOTS_UNBOUND: RefusalCode = RefusalCode::new("PROGRAM_SLOTS_UNBOUND");
/// Abstain reason: the matched program's index is not executable.
pub const PROGRAM_NOT_EXECUTABLE: RefusalCode = RefusalCode::new("PROGRAM_NOT_EXECUTABLE");
/// Abstain reason: the candidate search could not embed the request text.
pub const PROGRAM_EMBEDDING_UNAVAILABLE: RefusalCode =
    RefusalCode::new("PROGRAM_EMBEDDING_UNAVAILABLE");

/// Every abstain code of the semantic lookup, joined to `program_store::REFUSALS`.
pub const MATCH_REFUSALS: &[RefusalCode] = &[
    PHRASING_INVALID,
    PHRASING_SLOT_UNKNOWN,
    PHRASING_SLOTS_ADJACENT,
    PHRASING_SLOTS_DIFFER,
    MATCH_THRESHOLD_OUT_OF_RANGE,
    PROGRAM_EMBEDDING_SPACE_MISMATCH,
    REQUEST_TEXT_TOO_LONG,
    PROGRAM_GRAPH_REQUIRED,
    PROGRAM_CANDIDATES_RESERVED,
    PROGRAM_BELOW_THRESHOLD,
    PROGRAM_SLOTS_UNBOUND,
    PROGRAM_NOT_EXECUTABLE,
    PROGRAM_EMBEDDING_UNAVAILABLE,
];

/// Why a semantic lookup abstains (MEDULLA-DECISION-ENGINE-R135.16.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchRefusal {
    BelowThreshold {
        score_micros: i64,
        threshold_micros: i64,
    },
    SlotsUnbound {
        parameters: Vec<String>,
    },
    NotExecutable {
        task_signature: String,
    },
    EmbeddingUnavailable {
        cause: String,
    },
}

impl DecisionRefusal for MatchRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::BelowThreshold { .. } => PROGRAM_BELOW_THRESHOLD,
            Self::SlotsUnbound { .. } => PROGRAM_SLOTS_UNBOUND,
            Self::NotExecutable { .. } => PROGRAM_NOT_EXECUTABLE,
            Self::EmbeddingUnavailable { .. } => PROGRAM_EMBEDDING_UNAVAILABLE,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::BelowThreshold {
                score_micros,
                threshold_micros,
            } => format!("the best score {score_micros} is below the threshold {threshold_micros}"),
            Self::SlotsUnbound { parameters } => {
                format!(
                    "the request text binds no value for {}",
                    parameters.join(", ")
                )
            }
            Self::NotExecutable { task_signature } => {
                format!("the program of `{task_signature}` is not executable")
            }
            Self::EmbeddingUnavailable { cause } => {
                format!("the request text could not be embedded: {cause}")
            }
        }
    }
}

/// The pointer key of the match stored for `task_signature`.
pub fn match_key(task_signature: &str) -> String {
    format!("run_record/program_match/{task_signature}")
}

/// The graph node id of phrasing `k` of the program of `task_signature`.
pub fn phrasing_node_id(task_signature: &str, k: usize) -> String {
    format!("program:{task_signature}#phrasing:{k}")
}

/// Lowest threshold a program may declare, in cosine millionths.
pub const MIN_MATCH_THRESHOLD_MICROS: i64 = 500_000;
/// Highest threshold a program may declare, in cosine millionths.
pub const MAX_MATCH_THRESHOLD_MICROS: i64 = 1_000_000;

/// The slots a request text could not bind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unbound {
    /// The slot names, in index order.
    pub parameters: Vec<String>,
}

/// The nearest stored program of a candidate search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// The nearest program with an index scored at or above its threshold.
    Selected {
        /// The program's task signature.
        task_signature: String,
        /// The phrasing node of its best hit.
        node_id: String,
        /// Its highest hit score.
        score_micros: i64,
        /// The threshold it was held to.
        threshold_micros: i64,
    },
    /// The nearest program with an index scored below its threshold.
    BelowThreshold {
        /// Its highest hit score.
        score_micros: i64,
        /// The threshold it was held to.
        threshold_micros: i64,
    },
    /// No hit names a program with an index.
    NoProgram,
}

/// One part of a parsed phrasing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Slot(String),
}

/// Trim and collapse each whitespace run to one space.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn invalid(detail: String) -> ProgramError {
    ProgramError::Invalid(detail)
}

/// Split a phrasing into literal text and `{p<k>}` placeholders.
fn parse_phrasing(text: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut rest = text;
    while let Some(open) = rest.find("{p") {
        let after = &rest[open + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with('}') {
            literal.push_str(&rest[..open]);
            if !literal.is_empty() {
                segments.push(Segment::Literal(std::mem::take(&mut literal)));
            }
            segments.push(Segment::Slot(format!("p{}", &after[..digits])));
            rest = &after[digits + 1..];
        } else {
            literal.push_str(&rest[..open + 2]);
            rest = after;
        }
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    segments
}

fn slot_names(segments: &[Segment]) -> std::collections::BTreeSet<String> {
    segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::Slot(name) => Some(name.clone()),
            Segment::Literal(_) => None,
        })
        .collect()
}

/// Build the match index of one stored program (MEDULLA-DECISION-ENGINE-R135.16.2).
///
/// Pure. `program` is the program compiled from `trace`; `phrasings` are the request phrasings
/// the caller committed with it. The index holds the trimmed phrasings, the placeholder set,
/// the literals of every other parameter and one template per trace step.
pub fn build_match_index(
    trace: &RunTraceRecord,
    program: &CompiledProgram,
    phrasings: &[ProgramPhrasing],
    threshold_micros: Option<i64>,
) -> Result<ProgramMatchIndex, ProgramError> {
    if let Some(threshold) = threshold_micros {
        if !(MIN_MATCH_THRESHOLD_MICROS..=MAX_MATCH_THRESHOLD_MICROS).contains(&threshold) {
            return Err(invalid(format!(
                "MATCH_THRESHOLD_OUT_OF_RANGE: {threshold} is outside \
                 [{MIN_MATCH_THRESHOLD_MICROS}, {MAX_MATCH_THRESHOLD_MICROS}]"
            )));
        }
    }
    if phrasings.len() > MAX_PHRASINGS {
        return Err(invalid(format!(
            "PHRASING_INVALID: {} phrasings exceed {MAX_PHRASINGS}",
            phrasings.len()
        )));
    }
    let declared: std::collections::BTreeSet<&str> = program
        .parameters
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect();
    let mut trimmed = Vec::with_capacity(phrasings.len());
    let mut slots: Option<std::collections::BTreeSet<String>> = None;
    for phrasing in phrasings {
        let text = phrasing.text.trim();
        if text.is_empty() || text.len() > MAX_PHRASING_BYTES {
            return Err(invalid(format!(
                "PHRASING_INVALID: a phrasing holds 1 to {MAX_PHRASING_BYTES} bytes"
            )));
        }
        let segments = parse_phrasing(text);
        for pair in segments.windows(2) {
            if matches!(pair, [Segment::Slot(_), Segment::Slot(_)]) {
                return Err(invalid(
                    "PHRASING_SLOTS_ADJACENT: two placeholders need literal text between them"
                        .to_string(),
                ));
            }
        }
        for window in segments.windows(3) {
            if let [Segment::Slot(_), Segment::Literal(between), Segment::Slot(_)] = window {
                if between.trim().is_empty() {
                    return Err(invalid(
                        "PHRASING_SLOTS_ADJACENT: two placeholders need literal text between them"
                            .to_string(),
                    ));
                }
            }
        }
        let names = slot_names(&segments);
        if let Some(unknown) = names.iter().find(|name| !declared.contains(name.as_str())) {
            return Err(invalid(format!(
                "PHRASING_SLOT_UNKNOWN: {unknown} names no declared parameter"
            )));
        }
        match &slots {
            Some(first) if *first != names => {
                return Err(invalid(
                    "PHRASING_SLOTS_DIFFER: every phrasing names the same placeholders".to_string(),
                ));
            }
            Some(_) => {}
            None => slots = Some(names),
        }
        trimmed.push(ProgramPhrasing {
            text: text.to_string(),
        });
    }
    let slots: Vec<String> = slots.unwrap_or_default().into_iter().collect();
    let walk = literal_walk(trace);
    let mut fixed = Vec::new();
    let mut slot_kinds = Vec::new();
    if !trimmed.is_empty() {
        for (index, value) in walk.values.iter().enumerate() {
            let name = format!("p{index}");
            if !slots.contains(&name) {
                fixed.push(FixedValue {
                    parameter: name,
                    value: value.clone(),
                });
            }
        }
        for name in &slots {
            let kind = name
                .strip_prefix('p')
                .and_then(|k| k.parse::<usize>().ok())
                .and_then(|k| walk.kinds.get(k).copied())
                .unwrap_or(ProgramParameterKind::Text);
            slot_kinds.push(kind);
        }
    }
    let mut steps = Vec::with_capacity(trace.steps.len());
    let mut tool_calls = 0usize;
    let mut executable = true;
    for (k, step) in trace.steps.iter().enumerate() {
        let (mut server, mut tool, mut arguments) = (String::new(), String::new(), Vec::new());
        match step.kind {
            StepKind::ToolCall => {
                tool_calls += 1;
                match step.action_ref.split_once('/') {
                    Some((s, t)) if !s.is_empty() && !t.is_empty() => {
                        server = s.to_string();
                        tool = t.to_string();
                    }
                    _ => executable = false,
                }
                arguments = step
                    .arguments
                    .iter()
                    .zip(&walk.slots[k])
                    .map(|(argument, slot)| TemplateArgument {
                        name: argument.name.clone(),
                        parameter: format!("p{slot}"),
                    })
                    .collect();
            }
            StepKind::ModelCall => {}
            StepKind::Other => executable = false,
        }
        steps.push(StepTemplate {
            index: step.index,
            kind: step.kind,
            server,
            tool,
            arguments: BoundedVec::new(arguments).map_err(ProgramError::Invalid)?,
        });
    }
    Ok(ProgramMatchIndex {
        schema_version: MATCH_SCHEMA_VERSION,
        task_signature: program.task_signature.clone(),
        program_digest: program.program_digest.clone(),
        threshold_micros,
        phrasings: BoundedVec::new(trimmed).map_err(ProgramError::Invalid)?,
        slots: BoundedVec::new(slots).map_err(ProgramError::Invalid)?,
        slot_kinds: BoundedVec::new(slot_kinds).map_err(ProgramError::Invalid)?,
        fixed: BoundedVec::new(fixed).map_err(ProgramError::Invalid)?,
        steps: BoundedVec::new(steps).map_err(ProgramError::Invalid)?,
        executable: executable && tool_calls > 0,
        space_digest: None,
        embedded_nodes: BoundedVec::default(),
    })
}

/// Try to fit `segments` over the whole of `text`, a placeholder capturing the shortest
/// non-empty text before the next literal. `lower` is `text` with ASCII case folded, so the
/// byte offsets of both agree.
fn fit(
    segments: &[Segment],
    text: &str,
    lower: &str,
    pos: usize,
    captures: &mut Vec<(String, String)>,
) -> bool {
    let Some((head, rest)) = segments.split_first() else {
        return pos == text.len();
    };
    match head {
        Segment::Literal(literal) => {
            !literal.is_empty()
                && lower[pos..].starts_with(literal.as_str())
                && fit(rest, text, lower, pos + literal.len(), captures)
        }
        Segment::Slot(name) => {
            let capture = |end: usize, captures: &mut Vec<(String, String)>| {
                let taken = text[pos..end].trim();
                if taken.is_empty() {
                    return None;
                }
                let earlier = captures.iter().find(|(known, _)| known == name);
                if earlier.is_some_and(|(_, value)| value != taken) {
                    return None;
                }
                let added = earlier.is_none();
                if added {
                    captures.push((name.clone(), taken.to_string()));
                }
                Some(added)
            };
            match rest.first() {
                None => match capture(text.len(), captures) {
                    Some(added) => {
                        let done = fit(rest, text, lower, text.len(), captures);
                        if !done && added {
                            captures.pop();
                        }
                        done
                    }
                    None => false,
                },
                Some(Segment::Literal(next)) if !next.is_empty() => {
                    let mut from = pos;
                    while let Some(found) = lower[from..].find(next.as_str()) {
                        let end = from + found;
                        if let Some(added) = capture(end, captures) {
                            if fit(rest, text, lower, end, captures) {
                                return true;
                            }
                            if added {
                                captures.pop();
                            }
                        }
                        from = end + next.len().max(1);
                        if from > lower.len() {
                            break;
                        }
                    }
                    false
                }
                Some(_) => false,
            }
        }
    }
}

fn convert(kind: ProgramParameterKind, capture: &str) -> Option<TraceScalar> {
    match kind {
        ProgramParameterKind::Text => Some(TraceScalar::Text(capture.to_string())),
        ProgramParameterKind::Int => capture.parse::<i64>().ok().map(TraceScalar::Int),
        ProgramParameterKind::Bool => {
            if capture.eq_ignore_ascii_case("true") {
                Some(TraceScalar::Bool(true))
            } else if capture.eq_ignore_ascii_case("false") {
                Some(TraceScalar::Bool(false))
            } else {
                None
            }
        }
    }
}

/// Bind the parameters of an indexed program from a request text
/// (MEDULLA-DECISION-ENGINE-R135.16.2). Slots come from the first phrasing that fits and whose
/// captures all convert; the fixed values bind the rest.
pub fn bind_request(
    index: &ProgramMatchIndex,
    request_text: &str,
) -> Result<Vec<Binding>, Unbound> {
    let parameters: std::collections::BTreeSet<&str> = index
        .steps
        .iter()
        .flat_map(|step| {
            step.arguments
                .iter()
                .map(|argument| argument.parameter.as_str())
        })
        .collect();
    if parameters.is_empty() {
        return Ok(Vec::new());
    }
    let unbound = || Unbound {
        parameters: if index.phrasings.is_empty() {
            parameters.iter().map(|name| (*name).to_string()).collect()
        } else {
            index.slots.iter().cloned().collect()
        },
    };
    let text = normalize(request_text);
    let lower = text.to_ascii_lowercase();
    for phrasing in &index.phrasings {
        let segments: Vec<Segment> = parse_phrasing(&phrasing.text)
            .into_iter()
            .map(|segment| match segment {
                Segment::Literal(literal) => {
                    Segment::Literal(normalize(&literal).to_ascii_lowercase())
                }
                slot => slot,
            })
            .collect();
        let mut captures = Vec::new();
        if !fit(&segments, &text, &lower, 0, &mut captures) {
            continue;
        }
        let mut bindings = Vec::with_capacity(index.slots.len() + index.fixed.len());
        for (position, slot) in index.slots.iter().enumerate() {
            let kind = index
                .slot_kinds
                .as_slice()
                .get(position)
                .copied()
                .unwrap_or(ProgramParameterKind::Text);
            let value = captures
                .iter()
                .find(|(name, _)| name == slot)
                .and_then(|(_, capture)| convert(kind, capture));
            match value {
                Some(value) => bindings.push(Binding {
                    parameter: slot.clone(),
                    source: BindingSource::Slot,
                    value,
                }),
                None => break,
            }
        }
        if bindings.len() != index.slots.len() {
            continue;
        }
        bindings.extend(index.fixed.iter().map(|fixed| Binding {
            parameter: fixed.parameter.clone(),
            source: BindingSource::Fixed,
            value: fixed.value.clone(),
        }));
        return Ok(bindings);
    }
    Err(unbound())
}

/// Pick the nearest program of `candidates` (MEDULLA-DECISION-ENGINE-R135.16.2). A program
/// scores as its best hit; programs order by score, then task signature. `threshold_of` answers
/// `None` for a program with no index and `Some(own threshold)` otherwise.
pub fn select_match(
    candidates: &ProgramCandidates,
    threshold_of: impl Fn(&str) -> Option<Option<i64>>,
    default_micros: i64,
) -> Selection {
    let mut best: std::collections::BTreeMap<&str, (&str, i64)> = std::collections::BTreeMap::new();
    for hit in &candidates.hits {
        let slot = best
            .entry(hit.task_signature.as_str())
            .or_insert((hit.node_id.as_str(), hit.score_micros));
        if hit.score_micros > slot.1 {
            *slot = (hit.node_id.as_str(), hit.score_micros);
        }
    }
    let mut ordered: Vec<(&str, &str, i64)> = best
        .into_iter()
        .map(|(signature, (node, score))| (signature, node, score))
        .collect();
    ordered.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.as_bytes().cmp(b.0.as_bytes()))
    });
    for (signature, node, score) in ordered {
        let Some(own) = threshold_of(signature) else {
            continue;
        };
        let threshold = own.unwrap_or(default_micros);
        return if score < threshold {
            Selection::BelowThreshold {
                score_micros: score,
                threshold_micros: threshold,
            }
        } else {
            Selection::Selected {
                task_signature: signature.to_string(),
                node_id: node.to_string(),
                score_micros: score,
                threshold_micros: threshold,
            }
        };
    }
    Selection::NoProgram
}

/// The `tool_call` templates of `index` with each argument bound, in order.
pub fn bind_steps(index: &ProgramMatchIndex, bindings: &[Binding]) -> Vec<BoundStep> {
    index
        .steps
        .iter()
        .filter(|step| step.kind == StepKind::ToolCall)
        .map(|step| {
            let arguments = step
                .arguments
                .iter()
                .filter_map(|argument| {
                    let bound = bindings
                        .iter()
                        .find(|binding| binding.parameter == argument.parameter)?;
                    Some(BoundArgument {
                        name: argument.name.clone(),
                        value: bound.value.clone(),
                    })
                })
                .collect();
            BoundStep {
                index: step.index,
                server: step.server.clone(),
                tool: step.tool.clone(),
                // At most the template's own 32 arguments, so the bound cannot be exceeded.
                arguments: BoundedVec::new(arguments).expect("bounded by the template"),
            }
        })
        .collect()
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
        round_trip(&ProgramPhrasing {
            text: "book a table".into(),
        });
        round_trip(&TemplateArgument {
            name: "city".into(),
            parameter: "p0".into(),
        });
        let step = StepTemplate {
            index: 1,
            kind: StepKind::ToolCall,
            server: "maps".into(),
            tool: "search".into(),
            arguments: bounded(vec![TemplateArgument {
                name: "q".into(),
                parameter: "p0".into(),
            }]),
        };
        round_trip(&step);
        let fixed = FixedValue {
            parameter: "p1".into(),
            value: TraceScalar::Int(3),
        };
        round_trip(&fixed);
        let index = ProgramMatchIndex {
            schema_version: MATCH_SCHEMA_VERSION,
            task_signature: "sig".into(),
            program_digest: "ab".repeat(32),
            threshold_micros: Some(850_000),
            phrasings: bounded(vec![ProgramPhrasing { text: "x".into() }]),
            slots: bounded(vec!["p0".into()]),
            slot_kinds: bounded(vec![ProgramParameterKind::Text]),
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
            arguments: bounded(vec![BoundArgument {
                name: "q".into(),
                value: TraceScalar::Bool(true),
            }]),
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
        let hit = ProgramHit {
            node_id: "n".into(),
            task_signature: "sig".into(),
            score_micros: 1,
        };
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
        let empty: BoundedVec<ProgramPhrasing, 16> =
            serde_json::from_value(serde_json::json!([])).unwrap();
        assert!(empty.is_empty());
        let too_many = vec![serde_json::json!({"text": "x"}); 17];
        assert!(serde_json::from_value::<BoundedVec<ProgramPhrasing, 16>>(
            serde_json::Value::Array(too_many)
        )
        .is_err());
    }

    fn sample_trace() -> RunTraceRecord {
        use crate::decision::run_governance::{RunOutcome, StepOutcome, TraceArgument, TraceStep};
        let argument = |name: &str, value: TraceScalar| TraceArgument {
            name: name.into(),
            value,
        };
        RunTraceRecord {
            schema_version: 1,
            run_id: "run-1".into(),
            task_signature: "refund-status".into(),
            model_id: "m".into(),
            steps: bounded(vec![TraceStep {
                index: 0,
                kind: StepKind::ToolCall,
                action_ref: "orders/status".into(),
                outcome: StepOutcome::Ok,
                arguments: bounded(vec![
                    argument("order", TraceScalar::Text("Z-9".into())),
                    argument("limit", TraceScalar::Int(3)),
                ]),
            }]),
            outcome: RunOutcome::Succeeded,
            started_at_ms: 1,
        }
    }

    fn sample_index(
        phrasings: &[&str],
        threshold: Option<i64>,
    ) -> Result<ProgramMatchIndex, ProgramError> {
        let trace = sample_trace();
        let program = super::super::compile_program(
            &trace,
            &super::super::ProgramKnobs::from_policy(
                &crate::decision::policy::DecisionPolicy::engine_default(),
            ),
        )
        .unwrap();
        let phrasings: Vec<ProgramPhrasing> = phrasings
            .iter()
            .map(|text| ProgramPhrasing {
                text: (*text).into(),
            })
            .collect();
        build_match_index(&trace, &program, &phrasings, threshold)
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.16.2
    #[test]
    fn phrasing_binds_slots_and_fixed_values() {
        let index = sample_index(&["refund status for order {p0}"], None).unwrap();
        assert_eq!(index.slots.as_slice(), ["p0"]);
        assert_eq!(
            index.fixed.as_slice(),
            [FixedValue {
                parameter: "p1".into(),
                value: TraceScalar::Int(3)
            }]
        );
        assert!(index.executable);
        assert_eq!(
            (
                index.steps.as_slice()[0].server.as_str(),
                index.steps.as_slice()[0].tool.as_str()
            ),
            ("orders", "status")
        );
        let bound = bind_request(&index, "Refund  status for order A-1001").unwrap();
        assert_eq!(
            bound,
            vec![
                Binding {
                    parameter: "p0".into(),
                    source: BindingSource::Slot,
                    value: TraceScalar::Text("A-1001".into())
                },
                Binding {
                    parameter: "p1".into(),
                    source: BindingSource::Fixed,
                    value: TraceScalar::Int(3)
                },
            ]
        );
        let steps = bind_steps(&index, &bound);
        assert_eq!(steps.len(), 1);
        assert_eq!(
            steps[0].arguments.as_slice()[0].value,
            TraceScalar::Text("A-1001".into())
        );
        // No phrasing fits.
        assert_eq!(
            bind_request(&index, "where is my parcel").unwrap_err(),
            Unbound {
                parameters: vec!["p0".into()]
            }
        );
        // No phrasings: a program with parameters is unbound; the index holds no slots or fixed values.
        let bare = sample_index(&[], None).unwrap();
        assert!(bare.slots.is_empty() && bare.fixed.is_empty());
        assert!(bind_request(&bare, "anything").is_err());
        // Threshold and phrasing rules.
        for (phrasing, code) in [
            ("{p0} {p1}", "PHRASING_SLOTS_ADJACENT"),
            ("order {p7}", "PHRASING_SLOT_UNKNOWN"),
            ("   ", "PHRASING_INVALID"),
        ] {
            let error = sample_index(&[phrasing], None).unwrap_err().to_string();
            assert!(error.contains(code), "{error}");
        }
        assert!(sample_index(&["a {p0}", "b {p1}"], None)
            .unwrap_err()
            .to_string()
            .contains("PHRASING_SLOTS_DIFFER"));
        assert!(sample_index(&["a {p0}"], Some(499_999))
            .unwrap_err()
            .to_string()
            .contains("MATCH_THRESHOLD_OUT_OF_RANGE"));
        assert!(sample_index(&["a {p0}"], Some(500_000)).is_ok());
        // Kinds convert: an int slot refuses text and binds a decimal.
        let int_slot = sample_index(&["limit {p1} for {p0}"], None).unwrap();
        assert_eq!(
            int_slot.slot_kinds.as_slice(),
            [ProgramParameterKind::Text, ProgramParameterKind::Int]
        );
        assert!(bind_request(&int_slot, "limit many for A").is_err());
        let ok = bind_request(&int_slot, "LIMIT 12 for A").unwrap();
        assert_eq!(ok[1].value, TraceScalar::Int(12));
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.16.2
    #[test]
    fn nearest_program_at_threshold_wins_ties_by_signature() {
        let hit = |node: &str, signature: &str, score: i64| ProgramHit {
            node_id: node.into(),
            task_signature: signature.into(),
            score_micros: score,
        };
        let candidates = ProgramCandidates {
            space_digest: None,
            embedding_calls: 1,
            hits: bounded(vec![
                hit("n1", "b-sig", 900_000),
                hit("n2", "a-sig", 900_000),
                hit("n3", "a-sig", 800_000),
                hit("n4", "no-index", 990_000),
            ]),
            unavailable: None,
        };
        let known = |signature: &str| match signature {
            "a-sig" | "b-sig" => Some(None),
            _ => None,
        };
        // The tie goes to the smaller signature; a program with no index is passed over.
        assert_eq!(
            select_match(&candidates, known, 850_000),
            Selection::Selected {
                task_signature: "a-sig".into(),
                node_id: "n2".into(),
                score_micros: 900_000,
                threshold_micros: 850_000,
            }
        );
        // A score exactly at the threshold wins; one below it does not.
        assert!(matches!(
            select_match(&candidates, known, 900_000),
            Selection::Selected { .. }
        ));
        assert_eq!(
            select_match(&candidates, known, 900_001),
            Selection::BelowThreshold {
                score_micros: 900_000,
                threshold_micros: 900_001
            }
        );
        // The program's own threshold overrides the default.
        let own = |signature: &str| (signature == "a-sig").then_some(Some(950_000));
        assert_eq!(
            select_match(&candidates, own, 500_000),
            Selection::BelowThreshold {
                score_micros: 900_000,
                threshold_micros: 950_000
            }
        );
        assert_eq!(
            select_match(&candidates, |_| None, 850_000),
            Selection::NoProgram
        );
    }
}
