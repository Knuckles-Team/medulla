//! Canonical form rules, admission and export (MEDULLA-CQ-ORCH-R701.2).
//!
//! Every process source lowers its document to [`LoweredStep`]s and
//! [`LoweredFlow`]s and ends in [`canonicalize`]. Admission
//! ([`admit_canonical`]) recomputes the canonical ids and the digest of a
//! stated canonical document and refuses one that differs. No function here
//! names a source format.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};

use medulla_types::contract::Digest256;
use medulla_types::process_import::{
    CanonicalFlow, CanonicalStep, CanonicalWorkflow, GateKind, ImportError, LoweredFlow,
    LoweredStep, ProcessLoss, ProcessSourceSystem, SourceTrace, StepKind, LOSS_ANNEX_NOT_HELD,
    PROCESS_CANONICAL_SCHEMA, PROCESS_IMPORT_SCHEMA_VERSION,
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
pub fn lower_rules(
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

/// Assign canonical ids, sort flows and compute the digest of a lowered
/// process. `name`, `source` system and model id pass through.
pub fn canonicalize(
    name: &str,
    system: ProcessSourceSystem,
    model_id: &str,
    steps: Vec<LoweredStep>,
    flows: Vec<LoweredFlow>,
) -> Result<CanonicalWorkflow, ImportError> {
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
    let canonical_flows: Vec<CanonicalFlow> = sorted
        .into_iter()
        .map(|(s, t, condition, default)| CanonicalFlow {
            source: format!("s{s}"),
            target: format!("s{t}"),
            condition,
            default,
        })
        .collect();
    let digest = workflow_digest(&canonical_steps, &canonical_flows);
    Ok(CanonicalWorkflow {
        schema_version: PROCESS_IMPORT_SCHEMA_VERSION,
        schema: PROCESS_CANONICAL_SCHEMA.to_string(),
        name: normalize_text(name),
        steps: canonical_steps,
        flows: canonical_flows,
        digest,
        source: SourceTrace {
            schema_version: PROCESS_IMPORT_SCHEMA_VERSION,
            system,
            model_id: model_id.to_string(),
            elements,
        },
        annex: Vec::new(),
        losses: Vec::new(),
    })
}

/// The lowered form of a stated canonical document: each step keyed by its
/// source element id (else its step id), each flow by those keys.
fn lowered_of(doc: &CanonicalWorkflow) -> (Vec<LoweredStep>, Vec<LoweredFlow>) {
    let element = |id: &str| {
        doc.source
            .elements
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };
    let steps = doc
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
        .collect();
    let flows = doc
        .flows
        .iter()
        .map(|f| LoweredFlow {
            source: element(&f.source),
            target: element(&f.target),
            condition: f.condition.clone(),
            default: f.default,
        })
        .collect();
    (steps, flows)
}

/// Admit a stated canonical document with the default element limit.
pub fn admit_canonical(doc: &CanonicalWorkflow) -> Result<CanonicalWorkflow, ImportError> {
    admit_canonical_with_limit(doc, DEFAULT_PROCESS_IMPORT_MAX_ELEMENTS)
}

/// Recompute the canonical ids and the digest of `doc` and refuse a
/// document that differs. Annex and losses pass through, outside the digest.
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
    let (steps, flows) = lowered_of(doc);
    let mut computed = canonicalize(
        &doc.name,
        doc.source.system,
        &doc.source.model_id,
        steps,
        flows,
    )?;
    if computed.steps != doc.steps || computed.flows != doc.flows || computed.name != doc.name {
        return Err(ImportError::NotCanonical {
            reason: "steps, flows or name differ from their canonical form".into(),
        });
    }
    if computed.digest != doc.digest {
        return Err(ImportError::DigestMismatch {
            stated: doc.digest.clone(),
            computed: computed.digest,
        });
    }
    computed.source.elements = doc.source.elements.clone();
    computed.annex = doc.annex.clone();
    computed.losses = doc.losses.clone();
    Ok(computed)
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
