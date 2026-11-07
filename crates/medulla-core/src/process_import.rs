//! Process compilation (MEDULLA-CQ-ORCH-R701.2, R701.8, R701.11).
//!
//! Four stages, each a pure function from one typed form to the next: a
//! source's Parse gives its source document, viewed as a [`SourceGraph`];
//! [`lower`] gives a [`LoweredProcess`] (rules 1, 2 and 6); [`allocate`]
//! gives an [`AllocatedProcess`] (rules 3 and 4); [`emit`] gives the
//! [`CanonicalWorkflow`] (rule 5). [`canonicalize`] is `emit(allocate(x))`.
//! Admission ([`admit_canonical`]) runs Allocate and Emit again over a stated
//! canonical document and refuses a difference. No stage names a source
//! format: a source adds a Parse and its kind annotations only.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};

use medulla_types::contract::Digest256;
use medulla_types::process_import::{
    AllocatedProcess, AnnexEntry, CanonicalFlow, CanonicalStep, CanonicalWorkflow, GateKind,
    ImportError, KindAnnotation, LoweredFlow, LoweredProcess, LoweredStep, ProcessLoss,
    ProcessSourceSystem, SourceGraph, SourceNode, SourceTrace, StepKind, ANNEX_EPC_STATE_EVENT,
    LOSS_ANNEX_NOT_HELD, PROCESS_CANONICAL_SCHEMA, PROCESS_IMPORT_SCHEMA_VERSION,
};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

pub use medulla_types::scheduler_config::DEFAULT_PROCESS_IMPORT_MAX_ELEMENTS;

/// Rule 1: Unicode NFC, whitespace collapsed to single spaces, trimmed.
pub fn normalize_text(text: &str) -> String {
    let nfc: String = text.nfc().collect();
    nfc.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_opt(text: Option<&str>) -> Option<String> {
    text.map(normalize_text).filter(|t| !t.is_empty())
}

/// Rules 1 and 2 over a source graph: normalize text and derive split and
/// join steps from degree. A gate that both joins and splits becomes a join
/// step, one flow and a split step (`<element id>#split`).
fn lower_rules(
    steps: Vec<LoweredStep>,
    flows: Vec<LoweredFlow>,
) -> (Vec<LoweredStep>, Vec<LoweredFlow>) {
    let mut out_degree: HashMap<&str, usize> = HashMap::new();
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for flow in &flows {
        *out_degree.entry(flow.source.as_str()).or_default() += 1;
        *in_degree.entry(flow.target.as_str()).or_default() += 1;
    }
    let mut split_of: HashMap<String, String> = HashMap::new();
    let mut lowered = Vec::with_capacity(steps.len());
    let mut extra_flows = Vec::new();
    for step in &steps {
        let outs = out_degree
            .get(step.element_id.as_str())
            .copied()
            .unwrap_or(0);
        let ins = in_degree
            .get(step.element_id.as_str())
            .copied()
            .unwrap_or(0);
        let gated = matches!(
            step.kind,
            StepKind::Branch | StepKind::Fork | StepKind::Join
        );
        let base = LoweredStep {
            element_id: step.element_id.clone(),
            kind: step.kind,
            gate: step.gate,
            label: normalize_text(&step.label),
            role: normalize_opt(step.role.as_deref()),
            reference: normalize_opt(step.reference.as_deref()),
        };
        if !gated {
            lowered.push(base);
            continue;
        }
        let gate = step.gate.unwrap_or(GateKind::Xor);
        let split_kind = if gate == GateKind::Xor {
            StepKind::Branch
        } else {
            StepKind::Fork
        };
        match (ins >= 2, outs >= 2) {
            (true, true) => {
                let split_id = format!("{}#split", step.element_id);
                lowered.push(LoweredStep {
                    kind: StepKind::Join,
                    gate: Some(gate),
                    ..base.clone()
                });
                lowered.push(LoweredStep {
                    element_id: split_id.clone(),
                    kind: split_kind,
                    gate: Some(gate),
                    ..base
                });
                extra_flows.push(LoweredFlow {
                    source: step.element_id.clone(),
                    target: split_id.clone(),
                    condition: None,
                    default: false,
                });
                split_of.insert(step.element_id.clone(), split_id);
            }
            (true, false) => lowered.push(LoweredStep {
                kind: StepKind::Join,
                gate: Some(gate),
                ..base
            }),
            (false, true) => lowered.push(LoweredStep {
                kind: split_kind,
                gate: Some(gate),
                ..base
            }),
            (false, false) => lowered.push(LoweredStep {
                gate: Some(gate),
                ..base
            }),
        }
    }
    let mut lowered_flows: Vec<LoweredFlow> = flows
        .into_iter()
        .map(|flow| LoweredFlow {
            source: split_of.get(&flow.source).cloned().unwrap_or(flow.source),
            target: flow.target,
            condition: normalize_opt(flow.condition.as_deref()),
            default: flow.default,
        })
        .collect();
    lowered_flows.extend(extra_flows);
    (lowered, lowered_flows)
}

type Key = (usize, StepKind, Option<GateKind>, String, String);

fn key(step: &LoweredStep, min_pred: usize) -> Key {
    (
        min_pred,
        step.kind,
        step.gate,
        step.label.clone(),
        step.element_id.clone(),
    )
}

/// Rules 3 and 4: Kahn order of the steps, then the sorted flows. Returns
/// the order of step indices and the canonical index (1-based) of each step.
fn allocate_order(steps: &[LoweredStep], flows: &[LoweredFlow]) -> Result<Vec<usize>, ImportError> {
    let index: HashMap<&str, usize> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.element_id.as_str(), i))
        .collect();
    if index.len() != steps.len() {
        return Err(ImportError::NotCanonical {
            reason: "duplicate source element id".into(),
        });
    }
    let mut preds = vec![0usize; steps.len()];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); steps.len()];
    for flow in flows {
        let (Some(&s), Some(&t)) = (
            index.get(flow.source.as_str()),
            index.get(flow.target.as_str()),
        ) else {
            return Err(ImportError::NotCanonical {
                reason: format!(
                    "flow {} -> {} names an unknown step",
                    flow.source, flow.target
                ),
            });
        };
        succs[s].push(t);
        preds[t] += 1;
    }
    let mut unnumbered = preds.clone();
    let mut min_pred = vec![0usize; steps.len()];
    let mut numbered = vec![0usize; steps.len()];
    let mut heap: BinaryHeap<Reverse<(Key, usize)>> = BinaryHeap::new();
    for (i, step) in steps.iter().enumerate() {
        if preds[i] == 0 {
            heap.push(Reverse((key(step, 0), i)));
        }
    }
    let mut order = Vec::with_capacity(steps.len());
    while order.len() < steps.len() {
        let next = match heap.pop() {
            Some(Reverse((_, i))) if numbered[i] == 0 => i,
            Some(_) => continue,
            None => (0..steps.len())
                .filter(|&i| numbered[i] == 0)
                .min_by(|&a, &b| {
                    (unnumbered[a], key(&steps[a], min_pred[a]))
                        .cmp(&(unnumbered[b], key(&steps[b], min_pred[b])))
                })
                .expect("a step remains"),
        };
        order.push(next);
        let position = order.len();
        numbered[next] = position;
        for &t in &succs[next] {
            unnumbered[t] -= 1;
            if min_pred[t] == 0 {
                min_pred[t] = position;
            }
            if unnumbered[t] == 0 && numbered[t] == 0 {
                heap.push(Reverse((key(&steps[t], min_pred[t]), t)));
            }
        }
    }
    Ok(order)
}

/// Write `value` as canonical JSON: keys sorted, separators `,` and `:`,
/// non-ASCII kept.
pub fn canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (n, k) in keys.into_iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical_json(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Rule 5: SHA-256 hex of the canonical JSON of `schema`, `steps` and `flows`.
pub fn workflow_digest(steps: &[CanonicalStep], flows: &[CanonicalFlow]) -> String {
    let value = serde_json::json!({
        "schema": PROCESS_CANONICAL_SCHEMA,
        "steps": steps,
        "flows": flows,
    });
    let mut text = String::new();
    canonical_json(&value, &mut text);
    Digest256::sha256(text.as_bytes()).to_hex()
}

/// Lower: the source graph to [`LoweredProcess`]. Each node takes its kind
/// and gate from the annotation of its class; a class without one refuses
/// `UnmappedElement`. Then rules 1, 2 and 6: text is normalized, split and
/// join come from degree, and a `state` node folds into the flow that passes
/// it, as the arm condition after a split, else as an annex entry.
pub fn lower(
    graph: &SourceGraph,
    annotations: &[KindAnnotation],
) -> Result<LoweredProcess, ImportError> {
    let mut steps = Vec::with_capacity(graph.nodes.len());
    let mut states: HashMap<&str, &SourceNode> = HashMap::new();
    for node in &graph.nodes {
        let annotation = annotations
            .iter()
            .find(|a| a.class == node.class)
            .ok_or_else(|| ImportError::UnmappedElement {
                element_type: node.class.clone(),
            })?;
        match annotation.kind {
            Some(kind) => steps.push(LoweredStep {
                element_id: node.element_id.clone(),
                kind,
                gate: annotation.gate,
                label: node.label.clone(),
                role: node.role.clone(),
                reference: node.reference.clone(),
            }),
            None => {
                states.insert(node.element_id.as_str(), node);
            }
        }
    }
    let mut annex = graph.annex.clone();
    let flows = fold_states(&graph.edges, &states, &mut annex)?;
    let (steps, flows) = lower_rules(steps, flows);
    Ok(LoweredProcess {
        name: normalize_text(&graph.name),
        system: graph.system,
        model_id: graph.model_id.clone(),
        steps,
        flows,
        annex,
    })
}

/// Rule 6: replace each in-flow, `state`, out-flow chain by one flow.
fn fold_states(
    edges: &[LoweredFlow],
    states: &HashMap<&str, &SourceNode>,
    annex: &mut Vec<AnnexEntry>,
) -> Result<Vec<LoweredFlow>, ImportError> {
    if states.is_empty() {
        return Ok(edges.to_vec());
    }
    let mut out_degree: HashMap<&str, usize> = HashMap::new();
    for edge in edges {
        *out_degree.entry(edge.source.as_str()).or_default() += 1;
    }
    let mut flows = Vec::new();
    for edge in edges
        .iter()
        .filter(|e| !states.contains_key(e.source.as_str()))
    {
        let mut flow = edge.clone();
        let mut seen = 0usize;
        while let Some(state) = states.get(flow.target.as_str()) {
            seen += 1;
            let next: Vec<&LoweredFlow> = edges
                .iter()
                .filter(|e| e.source == state.element_id)
                .collect();
            if next.len() != 1 || seen > states.len() {
                return Err(ImportError::NotCanonical {
                    reason: format!("state {} has {} out-flows", state.element_id, next.len()),
                });
            }
            let after_split = out_degree.get(flow.source.as_str()).copied().unwrap_or(0) >= 2;
            if after_split && flow.condition.is_none() {
                flow.condition = Some(state.label.clone());
            } else {
                annex.push(AnnexEntry {
                    schema_version: PROCESS_IMPORT_SCHEMA_VERSION,
                    after: Some(flow.source.clone()),
                    before: Some(next[0].target.clone()),
                    construct: ANNEX_EPC_STATE_EVENT.to_string(),
                    element_id: state.element_id.clone(),
                    label: state.label.clone(),
                    type_num: None,
                });
            }
            flow.target = next[0].target.clone();
            flow.default |= next[0].default;
        }
        flows.push(flow);
    }
    Ok(flows)
}

/// Allocate: canonical ids `s1` to `sn` in Kahn order and sorted flows
/// (rules 3 and 4). Idempotent: allocating the lowered form of an allocated
/// process gives that process again.
pub fn allocate(lowered: LoweredProcess) -> Result<AllocatedProcess, ImportError> {
    let LoweredProcess {
        name,
        system,
        model_id,
        steps,
        flows,
        annex,
    } = lowered;
    let order = allocate_order(&steps, &flows)?;
    let mut position = vec![0usize; steps.len()];
    for (n, &i) in order.iter().enumerate() {
        position[i] = n + 1;
    }
    let by_element: HashMap<&str, usize> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.element_id.as_str(), position[i]))
        .collect();
    let mut elements = BTreeMap::new();
    let canonical_steps: Vec<CanonicalStep> = order
        .iter()
        .enumerate()
        .map(|(n, &i)| {
            let step = &steps[i];
            elements.insert(format!("s{}", n + 1), step.element_id.clone());
            CanonicalStep {
                id: format!("s{}", n + 1),
                kind: step.kind,
                gate: step.gate,
                label: step.label.clone(),
                role: step.role.clone(),
                reference: step.reference.clone(),
            }
        })
        .collect();
    let mut sorted: Vec<(usize, usize, Option<String>, bool)> = flows
        .iter()
        .map(|f| {
            (
                by_element[f.source.as_str()],
                by_element[f.target.as_str()],
                f.condition.clone(),
                f.default,
            )
        })
        .collect();
    sorted.sort();
    let canonical_flows = sorted
        .into_iter()
        .map(|(s, t, condition, default)| CanonicalFlow {
            source: format!("s{s}"),
            target: format!("s{t}"),
            condition,
            default,
        })
        .collect();
    Ok(AllocatedProcess {
        name,
        system,
        model_id,
        steps: canonical_steps,
        flows: canonical_flows,
        elements,
        annex,
    })
}

/// The lowered form of an allocated process: each step keyed by its source
/// element id, so that `allocate(relower(&allocate(x)?))` equals `allocate(x)`.
pub fn relower(allocated: &AllocatedProcess) -> LoweredProcess {
    let element = |id: &str| {
        allocated
            .elements
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };
    LoweredProcess {
        name: allocated.name.clone(),
        system: allocated.system,
        model_id: allocated.model_id.clone(),
        steps: allocated
            .steps
            .iter()
            .map(|s| LoweredStep {
                element_id: element(&s.id),
                kind: s.kind,
                gate: s.gate,
                label: s.label.clone(),
                role: s.role.clone(),
                reference: s.reference.clone(),
            })
            .collect(),
        flows: allocated
            .flows
            .iter()
            .map(|f| LoweredFlow {
                source: element(&f.source),
                target: element(&f.target),
                condition: f.condition.clone(),
                default: f.default,
            })
            .collect(),
        annex: allocated.annex.clone(),
    }
}

/// Emit: the digest (rule 5), the source trace and the annex.
pub fn emit(allocated: AllocatedProcess) -> CanonicalWorkflow {
    let digest = workflow_digest(&allocated.steps, &allocated.flows);
    CanonicalWorkflow {
        schema_version: PROCESS_IMPORT_SCHEMA_VERSION,
        schema: PROCESS_CANONICAL_SCHEMA.to_string(),
        name: allocated.name,
        steps: allocated.steps,
        flows: allocated.flows,
        digest,
        source: SourceTrace {
            schema_version: PROCESS_IMPORT_SCHEMA_VERSION,
            system: allocated.system,
            model_id: allocated.model_id,
            elements: allocated.elements,
        },
        annex: allocated.annex,
        losses: Vec::new(),
    }
}

/// `emit(allocate(lowered))`; no other path builds a canonical workflow.
pub fn canonicalize(lowered: LoweredProcess) -> Result<CanonicalWorkflow, ImportError> {
    Ok(emit(allocate(lowered)?))
}

/// The whole compile of one source graph: Lower, Allocate, Emit.
pub fn compile_process(
    graph: &SourceGraph,
    annotations: &[KindAnnotation],
) -> Result<CanonicalWorkflow, ImportError> {
    canonicalize(lower(graph, annotations)?)
}

/// The allocated form a stated canonical document claims.
fn allocated_of(doc: &CanonicalWorkflow) -> AllocatedProcess {
    AllocatedProcess {
        name: doc.name.clone(),
        system: doc.source.system,
        model_id: doc.source.model_id.clone(),
        steps: doc.steps.clone(),
        flows: doc.flows.clone(),
        elements: doc.source.elements.clone(),
        annex: doc.annex.clone(),
    }
}

/// Admit a stated canonical document with the default element limit.
pub fn admit_canonical(doc: &CanonicalWorkflow) -> Result<CanonicalWorkflow, ImportError> {
    admit_canonical_with_limit(doc, DEFAULT_PROCESS_IMPORT_MAX_ELEMENTS)
}

/// Run Allocate and Emit again over a stated canonical document and refuse
/// a difference. Annex and losses pass through, outside the digest.
pub fn admit_canonical_with_limit(
    doc: &CanonicalWorkflow,
    max_elements: u64,
) -> Result<CanonicalWorkflow, ImportError> {
    if doc.schema != PROCESS_CANONICAL_SCHEMA {
        return Err(ImportError::NotCanonical {
            reason: format!("schema {}", doc.schema),
        });
    }
    let elements = (doc.steps.len() + doc.flows.len()) as u64;
    if elements > max_elements {
        return Err(ImportError::NotCanonical {
            reason: format!(
                "{elements} elements exceed process_import_max_elements {max_elements}"
            ),
        });
    }
    let stated = allocated_of(doc);
    let mut recomputed = allocate(relower(&stated))?;
    if recomputed.steps != stated.steps
        || recomputed.flows != stated.flows
        || normalize_text(&doc.name) != doc.name
    {
        return Err(ImportError::NotCanonical {
            reason: "steps, flows or name differ from their canonical form".into(),
        });
    }
    recomputed.elements = stated.elements;
    let mut admitted = emit(recomputed);
    if admitted.digest != doc.digest {
        return Err(ImportError::DigestMismatch {
            stated: doc.digest.clone(),
            computed: admitted.digest,
        });
    }
    admitted.losses = doc.losses.clone();
    Ok(admitted)
}

/// Export a stored workflow toward `target`: the annex is kept for a
/// same-source export and every construct the target cannot hold is listed
/// in `losses` (MEDULLA-CQ-ORCH-R701.8). Steps, flows and digest are kept.
pub fn export_canonical(
    workflow: &CanonicalWorkflow,
    target: ProcessSourceSystem,
) -> CanonicalWorkflow {
    let mut out = workflow.clone();
    out.losses = losses_for(workflow, target);
    if target != workflow.source.system {
        out.annex.clear();
    }
    out
}

/// The wire code of a strict export that would lose a construct.
pub const PROCESS_EXPORT_LOSSY: &str = "PROCESS_EXPORT_LOSSY";

/// A strict export refused because the target cannot hold every construct.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessExportLossy {
    pub losses: Vec<ProcessLoss>,
}

impl ProcessExportLossy {
    pub fn code(&self) -> &'static str {
        PROCESS_EXPORT_LOSSY
    }
}

fn treatment(target: ProcessSourceSystem, construct: &str) -> Option<&'static str> {
    target
        .inexpressible()
        .iter()
        .find(|(name, _)| *name == construct)
        .map(|(_, treatment)| *treatment)
}

/// Every construct of `workflow` that `target` cannot hold, one loss each,
/// never a silent drop (MEDULLA-CQ-ORCH-R701.8).
pub fn losses_for(workflow: &CanonicalWorkflow, target: ProcessSourceSystem) -> Vec<ProcessLoss> {
    let loss = |element: &str, construct: &str, treatment: &str| ProcessLoss {
        schema_version: PROCESS_IMPORT_SCHEMA_VERSION,
        element: element.to_string(),
        construct: construct.to_string(),
        target,
        treatment: treatment.to_string(),
    };
    let mut losses = Vec::new();
    if target != workflow.source.system {
        let held = target.annex_constructs();
        for entry in &workflow.annex {
            if !held.contains(&entry.construct.as_str()) {
                losses.push(loss(
                    &entry.element_id,
                    &entry.construct,
                    LOSS_ANNEX_NOT_HELD,
                ));
            }
        }
    }
    for step in &workflow.steps {
        let construct = match (step.kind, step.gate) {
            (StepKind::Wait, None) => "wait",
            (StepKind::Wait, Some(_)) => "event-based-wait",
            (StepKind::Boundary, _) => "boundary",
            (StepKind::Decision, _) => "decision",
            _ => continue,
        };
        if let Some(treatment) = treatment(target, construct) {
            losses.push(loss(&step.id, construct, treatment));
        }
    }
    for flow in workflow.flows.iter().filter(|f| f.default) {
        if let Some(treatment) = treatment(target, "default-flow") {
            let element = format!("{}->{}", flow.source, flow.target);
            losses.push(loss(&element, "default-flow", treatment));
        }
    }
    losses
}

/// Export with `strict`: refuse `PROCESS_EXPORT_LOSSY` with the losses
/// instead of returning a lossy export.
pub fn export_canonical_strict(
    workflow: &CanonicalWorkflow,
    target: ProcessSourceSystem,
    strict: bool,
) -> Result<CanonicalWorkflow, ProcessExportLossy> {
    let exported = export_canonical(workflow, target);
    if strict && !exported.losses.is_empty() {
        return Err(ProcessExportLossy {
            losses: exported.losses,
        });
    }
    Ok(exported)
}
