//! BPMN 2.0 reader (MEDULLA-CQ-ORCH-R702.2): one streaming pass, no DTD,
//! no external entity.

use std::collections::BTreeMap;

use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::{NsReader, Writer};

use super::{
    BpmnCollaboration, BpmnDocument, BpmnEventDefinition, BpmnLane, BpmnNode, BpmnProcess,
    BpmnSequenceFlow, PreservedXml, ProcessExchangeError, BPMN_MODEL_NS, BPMN_NODE_KINDS,
};

/// Bounds of one read (`process_document_max_bytes`, `process_document_max_depth`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BpmnLimits {
    pub max_bytes: u64,
    pub max_depth: u32,
}

impl Default for BpmnLimits {
    fn default() -> Self {
        Self {
            max_bytes: medulla_types::scheduler_config::DEFAULT_PROCESS_DOCUMENT_MAX_BYTES,
            max_depth: medulla_types::scheduler_config::DEFAULT_PROCESS_DOCUMENT_MAX_DEPTH,
        }
    }
}

/// Where the reader is inside the document.
enum Frame {
    Definitions,
    Process,
    Collaboration,
    LaneSet,
    Lane,
    FlowNodeRef,
    Node,
    EventDefinition,
    Flow,
    Condition,
    /// A BPMN element the reader passes through without a model of its own.
    Skip,
}

struct Capture {
    owner: String,
    depth: usize,
    writer: Writer<Vec<u8>>,
    in_process: bool,
}

#[derive(Default)]
struct State {
    doc: BpmnDocument,
    process: Option<BpmnProcess>,
    collaboration: Option<BpmnCollaboration>,
    lane: Option<BpmnLane>,
    node: Option<BpmnNode>,
    flow: Option<BpmnSequenceFlow>,
    text: String,
}

fn position(bytes: &[u8], offset: u64) -> ProcessExchangeError {
    let end = (offset as usize).min(bytes.len());
    let before = &bytes[..end];
    let line = before.iter().filter(|&&b| b == b'\n').count() as u64 + 1;
    let column = before.iter().rev().take_while(|&&b| b != b'\n').count() as u64 + 1;
    ProcessExchangeError::Malformed { line, column }
}

fn attributes(
    element: &BytesStart<'_>,
    bytes: &[u8],
    offset: u64,
) -> Result<BTreeMap<String, String>, ProcessExchangeError> {
    let mut out = BTreeMap::new();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| position(bytes, offset))?;
        let key = std::str::from_utf8(attribute.key.local_name().as_ref())
            .map_err(|_| position(bytes, offset))?
            .to_string();
        if attribute.key.as_ref().starts_with(b"xmlns") {
            continue;
        }
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
            .map_err(|error| match error {
                quick_xml::Error::Escape(_) => ProcessExchangeError::EntityRefused,
                _ => position(bytes, offset),
            })?
            .into_owned();
        out.insert(key, value);
    }
    Ok(out)
}

fn reference_text(reference: &BytesRef<'_>) -> Result<String, ProcessExchangeError> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|_| ProcessExchangeError::EntityRefused)?
    {
        return Ok(character.to_string());
    }
    let name = reference
        .decode()
        .map_err(|_| ProcessExchangeError::EntityRefused)?;
    Ok(match name.as_ref() {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "apos" => "'",
        "quot" => "\"",
        _ => return Err(ProcessExchangeError::EntityRefused),
    }
    .to_string())
}

/// Parse one BPMN 2.0 document. A DTD or an undeclared entity refuses
/// `EntityRefused`; a root outside the BPMN model namespace refuses
/// `NamespaceUnknown`; elements the reader does not model are preserved.
pub fn parse_bpmn(bytes: &[u8], limits: BpmnLimits) -> Result<BpmnDocument, ProcessExchangeError> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(ProcessExchangeError::TooLarge {
            bytes: limits.max_bytes,
        });
    }
    let text =
        std::str::from_utf8(bytes).map_err(|error| position(bytes, error.valid_up_to() as u64))?;
    let mut reader = NsReader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut frames: Vec<Frame> = Vec::new();
    let mut capture: Option<Capture> = None;
    let mut state = State::default();
    loop {
        let offset = reader.buffer_position();
        let event = reader.read_event().map_err(|_| position(bytes, offset))?;
        if matches!(event, Event::DocType(_)) {
            return Err(ProcessExchangeError::EntityRefused);
        }
        if let Event::GeneralRef(reference) = &event {
            let resolved = reference_text(reference)?;
            if capture.is_none()
                && matches!(frames.last(), Some(Frame::Condition | Frame::FlowNodeRef))
            {
                state.text.push_str(&resolved);
            }
        }
        if let Some(active) = capture.as_mut() {
            match &event {
                Event::Start(_) => active.depth += 1,
                Event::End(_) => active.depth -= 1,
                Event::Eof => return Err(position(bytes, offset)),
                _ => {}
            }
            if frames.len() + active.depth > limits.max_depth as usize {
                return Err(ProcessExchangeError::TooLarge {
                    bytes: limits.max_bytes,
                });
            }
            active
                .writer
                .write_event(event.clone())
                .map_err(|_| position(bytes, offset))?;
            if active.depth == 0 {
                let done = capture.take().expect("active capture");
                let preserved = PreservedXml {
                    owner: done.owner,
                    xml: String::from_utf8(done.writer.into_inner())
                        .map_err(|_| position(bytes, offset))?,
                };
                match state.process.as_mut() {
                    Some(process) if done.in_process => process.preserved.push(preserved),
                    _ => state.doc.preserved.push(preserved),
                }
            }
            continue;
        }
        match event {
            Event::Start(element) => {
                if frames.len() + 1 > limits.max_depth as usize {
                    return Err(ProcessExchangeError::TooLarge {
                        bytes: limits.max_bytes,
                    });
                }
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let local = String::from_utf8_lossy(local.as_ref()).into_owned();
                let in_model = matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == BPMN_MODEL_NS.as_bytes());
                if frames.is_empty() {
                    if !in_model || local != "definitions" {
                        let namespace = match namespace {
                            ResolveResult::Bound(ns) => {
                                String::from_utf8_lossy(ns.as_ref()).into_owned()
                            }
                            _ => String::new(),
                        };
                        return Err(ProcessExchangeError::NamespaceUnknown { namespace });
                    }
                    let attrs = attributes(&element, bytes, offset)?;
                    state.doc.id = attrs.get("id").cloned().unwrap_or_default();
                    frames.push(Frame::Definitions);
                    continue;
                }
                match enter(
                    &mut state,
                    frames.last(),
                    in_model,
                    &local,
                    &element,
                    bytes,
                    offset,
                )? {
                    Some(frame) => frames.push(frame),
                    None => {
                        let mut writer = Writer::new(Vec::new());
                        writer
                            .write_event(Event::Start(element.into_owned()))
                            .map_err(|_| position(bytes, offset))?;
                        capture = Some(Capture {
                            owner: owner(&state),
                            depth: 1,
                            writer,
                            in_process: state.process.is_some(),
                        });
                    }
                }
            }
            Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let local = String::from_utf8_lossy(local.as_ref()).into_owned();
                let in_model = matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == BPMN_MODEL_NS.as_bytes());
                if frames.is_empty() {
                    // An empty root holds no process; it is still checked.
                    let namespace = match namespace {
                        ResolveResult::Bound(ns) if !in_model || local != "definitions" => {
                            String::from_utf8_lossy(ns.as_ref()).into_owned()
                        }
                        _ if in_model && local == "definitions" => {
                            let attrs = attributes(&element, bytes, offset)?;
                            state.doc.id = attrs.get("id").cloned().unwrap_or_default();
                            return Ok(state.doc);
                        }
                        _ => String::new(),
                    };
                    return Err(ProcessExchangeError::NamespaceUnknown { namespace });
                }
                match enter(
                    &mut state,
                    frames.last(),
                    in_model,
                    &local,
                    &element,
                    bytes,
                    offset,
                )? {
                    Some(frame) => leave(&mut state, frame),
                    None => {
                        let mut writer = Writer::new(Vec::new());
                        writer
                            .write_event(Event::Empty(element.into_owned()))
                            .map_err(|_| position(bytes, offset))?;
                        let preserved = PreservedXml {
                            owner: owner(&state),
                            xml: String::from_utf8(writer.into_inner())
                                .map_err(|_| position(bytes, offset))?,
                        };
                        match state.process.as_mut() {
                            Some(process) => process.preserved.push(preserved),
                            None => state.doc.preserved.push(preserved),
                        }
                    }
                }
            }
            Event::Text(text) => {
                if matches!(frames.last(), Some(Frame::Condition | Frame::FlowNodeRef)) {
                    state
                        .text
                        .push_str(&text.decode().map_err(|_| position(bytes, offset))?);
                }
            }
            Event::CData(text) => {
                if matches!(frames.last(), Some(Frame::Condition | Frame::FlowNodeRef)) {
                    state
                        .text
                        .push_str(&text.decode().map_err(|_| position(bytes, offset))?);
                }
            }
            Event::End(_) => match frames.pop() {
                Some(frame) => leave(&mut state, frame),
                None => return Err(position(bytes, offset)),
            },
            Event::Eof => break,
            _ => {}
        }
    }
    if !frames.is_empty() {
        return Err(position(bytes, bytes.len() as u64));
    }
    Ok(state.doc)
}

fn owner(state: &State) -> String {
    if let Some(node) = &state.node {
        return node.id.clone();
    }
    if let Some(flow) = &state.flow {
        return flow.id.clone();
    }
    if let Some(process) = &state.process {
        return process.id.clone();
    }
    state.doc.id.clone()
}

/// Open one element; `None` means the element is preserved as XML.
fn enter(
    state: &mut State,
    parent: Option<&Frame>,
    in_model: bool,
    local: &str,
    element: &BytesStart<'_>,
    bytes: &[u8],
    offset: u64,
) -> Result<Option<Frame>, ProcessExchangeError> {
    if !in_model {
        return Ok(None);
    }
    let attrs = || attributes(element, bytes, offset);
    let frame = match (parent, local) {
        (Some(Frame::Definitions), "process") => {
            let mut a = attrs()?;
            state.process = Some(BpmnProcess {
                id: a.remove("id").unwrap_or_default(),
                name: a.remove("name").unwrap_or_default(),
                ..BpmnProcess::default()
            });
            Frame::Process
        }
        (Some(Frame::Definitions), "collaboration") => {
            state.collaboration = Some(BpmnCollaboration {
                id: attrs()?.remove("id").unwrap_or_default(),
                participants: Vec::new(),
            });
            Frame::Collaboration
        }
        (Some(Frame::Collaboration), "participant") => {
            let mut a = attrs()?;
            if let Some(collaboration) = state.collaboration.as_mut() {
                collaboration.participants.push((
                    a.remove("id").unwrap_or_default(),
                    a.remove("processRef").unwrap_or_default(),
                ));
            }
            Frame::Skip
        }
        (Some(Frame::Process), "laneSet") => Frame::LaneSet,
        (Some(Frame::LaneSet), "lane") => {
            let mut a = attrs()?;
            state.lane = Some(BpmnLane {
                id: a.remove("id").unwrap_or_default(),
                name: a.remove("name").unwrap_or_default(),
                flow_node_refs: Vec::new(),
            });
            Frame::Lane
        }
        (Some(Frame::Lane), "flowNodeRef") => {
            state.text.clear();
            Frame::FlowNodeRef
        }
        (Some(Frame::Process), "sequenceFlow") => {
            let mut a = attrs()?;
            state.flow = Some(BpmnSequenceFlow {
                id: a.remove("id").unwrap_or_default(),
                name: a.remove("name").unwrap_or_default(),
                source_ref: a.remove("sourceRef").unwrap_or_default(),
                target_ref: a.remove("targetRef").unwrap_or_default(),
                condition: None,
            });
            Frame::Flow
        }
        (Some(Frame::Flow), "conditionExpression") => {
            state.text.clear();
            Frame::Condition
        }
        (Some(Frame::Process), kind) if BPMN_NODE_KINDS.contains(&kind) => {
            let mut a = attrs()?;
            state.node = Some(BpmnNode {
                id: a.remove("id").unwrap_or_default(),
                kind: kind.to_string(),
                name: a.remove("name").unwrap_or_default(),
                attributes: a,
                event_definitions: Vec::new(),
            });
            Frame::Node
        }
        (Some(Frame::Node), kind) if kind.ends_with("EventDefinition") => {
            let definition = BpmnEventDefinition {
                kind: kind.to_string(),
                attributes: attrs()?,
            };
            if let Some(node) = state.node.as_mut() {
                node.event_definitions.push(definition);
            }
            Frame::EventDefinition
        }
        (Some(Frame::Node | Frame::Flow), "incoming" | "outgoing") => Frame::Skip,
        _ => return Ok(None),
    };
    Ok(Some(frame))
}

/// Close one element and attach what it built to its parent.
fn leave(state: &mut State, frame: Frame) {
    match frame {
        Frame::Process => {
            if let Some(process) = state.process.take() {
                state.doc.processes.push(process);
            }
        }
        Frame::Collaboration => {
            if let Some(collaboration) = state.collaboration.take() {
                state.doc.collaborations.push(collaboration);
            }
        }
        Frame::Lane => {
            if let (Some(lane), Some(process)) = (state.lane.take(), state.process.as_mut()) {
                process.lanes.push(lane);
            }
        }
        Frame::FlowNodeRef => {
            let reference = state.text.trim().to_string();
            if let Some(lane) = state.lane.as_mut() {
                lane.flow_node_refs.push(reference);
            }
        }
        Frame::Node => {
            if let (Some(node), Some(process)) = (state.node.take(), state.process.as_mut()) {
                process.nodes.push(node);
            }
        }
        Frame::Flow => {
            if let (Some(flow), Some(process)) = (state.flow.take(), state.process.as_mut()) {
                process.flows.push(flow);
            }
        }
        Frame::Condition => {
            let condition = state.text.trim().to_string();
            if let Some(flow) = state.flow.as_mut() {
                flow.condition = Some(condition);
            }
        }
        Frame::Definitions | Frame::LaneSet | Frame::EventDefinition | Frame::Skip => {}
    }
}
