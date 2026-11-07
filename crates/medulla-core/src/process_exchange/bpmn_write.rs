//! BPMN 2.0 writer (MEDULLA-CQ-ORCH-R702.6): canonical BPMN 2.0 XML.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use quick_xml::escape::escape;

use super::{
    BpmnDocument, BpmnEventDefinition, BpmnNode, BpmnProcess, PreservedXml, ProcessExchangeError,
    BPMN_MODEL_NS,
};

fn attrs(out: &mut String, pairs: &BTreeMap<String, String>) {
    for (key, value) in pairs {
        let _ = write!(out, " {key}=\"{}\"", escape(value.as_str()));
    }
}

fn fragments<'a>(preserved: &'a [PreservedXml], owner: &'a str) -> impl Iterator<Item = &'a str> {
    preserved
        .iter()
        .filter(move |p| p.owner == owner)
        .map(|p| p.xml.as_str())
}

fn definition(out: &mut String, definition: &BpmnEventDefinition, nested: &[&str]) {
    let _ = write!(out, "<{}", definition.kind);
    attrs(out, &definition.attributes);
    if nested.is_empty() {
        out.push_str("/>");
    } else {
        out.push('>');
        nested.iter().for_each(|xml| out.push_str(xml));
        let _ = write!(out, "</{}>", definition.kind);
    }
}

fn node(out: &mut String, node: &BpmnNode, preserved: &[PreservedXml]) {
    let mut pairs = node.attributes.clone();
    pairs.insert("id".into(), node.id.clone());
    if !node.name.is_empty() {
        pairs.insert("name".into(), node.name.clone());
    }
    let _ = write!(out, "<{}", node.kind);
    attrs(out, &pairs);
    let kept: Vec<&str> = fragments(preserved, &node.id).collect();
    if node.event_definitions.is_empty() && kept.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    // A fragment the reader found inside an event definition (a timer's
    // duration) goes back inside the one definition; the rest follow it.
    let (inner, outer): (Vec<&str>, Vec<&str>) = if node.event_definitions.len() == 1 {
        kept.iter().partition(|xml| {
            !xml.starts_with("<extensionElements") && !xml.contains(":extensionElements")
        })
    } else {
        (Vec::new(), kept.clone())
    };
    for (n, def) in node.event_definitions.iter().enumerate() {
        definition(out, def, if n == 0 { &inner } else { &[] });
    }
    outer.iter().for_each(|xml| out.push_str(xml));
    let _ = write!(out, "</{}>", node.kind);
}

fn process(out: &mut String, process: &BpmnProcess) {
    let mut pairs = BTreeMap::from([
        ("id".to_string(), process.id.clone()),
        ("isExecutable".to_string(), "true".to_string()),
    ]);
    if !process.name.is_empty() {
        pairs.insert("name".into(), process.name.clone());
    }
    out.push_str("<process");
    attrs(out, &pairs);
    out.push('>');
    if !process.lanes.is_empty() {
        let _ = write!(
            out,
            "<laneSet id=\"{}_lanes\">",
            escape(process.id.as_str())
        );
        let mut lanes: Vec<_> = process.lanes.iter().collect();
        lanes.sort_by(|a, b| a.id.cmp(&b.id));
        for lane in lanes {
            let _ = write!(
                out,
                "<lane id=\"{}\" name=\"{}\">",
                escape(lane.id.as_str()),
                escape(lane.name.as_str())
            );
            for reference in &lane.flow_node_refs {
                let _ = write!(
                    out,
                    "<flowNodeRef>{}</flowNodeRef>",
                    escape(reference.as_str())
                );
            }
            out.push_str("</lane>");
        }
        out.push_str("</laneSet>");
    }
    let mut nodes: Vec<&BpmnNode> = process.nodes.iter().collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    for n in nodes {
        node(out, n, &process.preserved);
    }
    let mut flows: Vec<_> = process.flows.iter().collect();
    flows.sort_by(|a, b| a.id.cmp(&b.id));
    for flow in flows {
        let mut pairs = BTreeMap::from([
            ("id".to_string(), flow.id.clone()),
            ("sourceRef".to_string(), flow.source_ref.clone()),
            ("targetRef".to_string(), flow.target_ref.clone()),
        ]);
        if !flow.name.is_empty() {
            pairs.insert("name".into(), flow.name.clone());
        }
        out.push_str("<sequenceFlow");
        attrs(out, &pairs);
        let kept: Vec<&str> = fragments(&process.preserved, &flow.id).collect();
        if flow.condition.is_none() && kept.is_empty() {
            out.push_str("/>");
            continue;
        }
        out.push('>');
        if let Some(condition) = &flow.condition {
            let _ = write!(
                out,
                "<conditionExpression>{}</conditionExpression>",
                escape(condition.as_str())
            );
        }
        kept.iter().for_each(|xml| out.push_str(xml));
        out.push_str("</sequenceFlow>");
    }
    fragments(&process.preserved, &process.id).for_each(|xml| out.push_str(xml));
    out.push_str("</process>");
}

/// Write one BPMN 2.0 document as canonical XML: the BPMN namespace as the
/// default namespace, the root's prefixed namespaces kept, elements in id
/// order, `isExecutable="true"` on each process and each preserved fragment
/// written back at its owner element. Reading the result gives the same
/// document, so a round trip keeps the canonical digest.
pub fn write_bpmn(doc: &BpmnDocument) -> Result<Vec<u8>, ProcessExchangeError> {
    if doc.processes.is_empty() {
        return Err(ProcessExchangeError::NotDeployable {
            reason: "the document has no process".into(),
        });
    }
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = write!(out, "<definitions xmlns=\"{BPMN_MODEL_NS}\"");
    for (prefix, iri) in &doc.namespaces {
        let _ = write!(out, " xmlns:{prefix}=\"{}\"", escape(iri.as_str()));
    }
    let _ = write!(out, " id=\"{}\">", escape(doc.id.as_str()));
    for collaboration in &doc.collaborations {
        let _ = write!(
            out,
            "<collaboration id=\"{}\">",
            escape(collaboration.id.as_str())
        );
        for (id, process_ref) in &collaboration.participants {
            let _ = write!(
                out,
                "<participant id=\"{}\" processRef=\"{}\"/>",
                escape(id.as_str()),
                escape(process_ref.as_str())
            );
        }
        out.push_str("</collaboration>");
    }
    let mut processes: Vec<&BpmnProcess> = doc.processes.iter().collect();
    processes.sort_by(|a, b| a.id.cmp(&b.id));
    for p in processes {
        process(&mut out, p);
    }
    doc.preserved.iter().for_each(|p| out.push_str(&p.xml));
    out.push_str("</definitions>\n");
    Ok(out.into_bytes())
}
