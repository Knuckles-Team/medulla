//! DMN 1.3 writer (MEDULLA-CQ-ORCH-R702.7): canonical DMN XML.

use std::fmt::Write as _;

use medulla_types::contract::Digest256;
use quick_xml::escape::escape;

use super::dmn_read::{
    DmnClause, DmnDecision, DmnDocument, DmnRequirementKind, DmnTable, DMN_MODEL_NS,
};
use super::{PreservedXml, ProcessExchangeError};

fn fragments<'a>(preserved: &'a [PreservedXml], owner: &'a str) -> impl Iterator<Item = &'a str> {
    preserved
        .iter()
        .filter(move |p| p.owner == owner)
        .map(|p| p.xml.as_str())
}

fn text(out: &mut String, value: &str) {
    let _ = write!(out, "<text>{}</text>", escape(value));
}

fn optional_attr(out: &mut String, name: &str, value: &str) {
    if !value.is_empty() {
        let _ = write!(out, " {name}=\"{}\"", escape(value));
    }
}

fn input(out: &mut String, clause: &DmnClause) {
    let _ = write!(out, "<input id=\"{}\"", escape(clause.id.as_str()));
    optional_attr(out, "label", &clause.label);
    out.push_str("><inputExpression");
    optional_attr(out, "typeRef", &clause.type_ref);
    out.push('>');
    text(out, &clause.expression);
    out.push_str("</inputExpression></input>");
}

fn output(out: &mut String, clause: &DmnClause) {
    let _ = write!(out, "<output id=\"{}\"", escape(clause.id.as_str()));
    optional_attr(out, "label", &clause.label);
    optional_attr(out, "name", &clause.expression);
    optional_attr(out, "typeRef", &clause.type_ref);
    out.push_str("/>");
}

fn table(out: &mut String, table: &DmnTable) {
    let _ = write!(
        out,
        "<decisionTable id=\"{}\" hitPolicy=\"{}\"",
        escape(table.id.as_str()),
        escape(table.hit_policy.as_str())
    );
    if let Some(aggregation) = &table.aggregation {
        let _ = write!(out, " aggregation=\"{}\"", escape(aggregation.as_str()));
    }
    out.push('>');
    table.inputs.iter().for_each(|c| input(out, c));
    table.outputs.iter().for_each(|c| output(out, c));
    // Rules keep their declared order: the order is the meaning of a
    // FIRST or RULE ORDER table.
    for rule in &table.rules {
        let _ = write!(out, "<rule id=\"{}\">", escape(rule.id.as_str()));
        for entry in &rule.input_entries {
            out.push_str("<inputEntry>");
            text(out, entry);
            out.push_str("</inputEntry>");
        }
        for entry in &rule.output_entries {
            out.push_str("<outputEntry>");
            text(out, entry);
            out.push_str("</outputEntry>");
        }
        out.push_str("</rule>");
    }
    out.push_str("</decisionTable>");
}

fn decision(out: &mut String, decision: &DmnDecision, preserved: &[PreservedXml]) {
    let _ = write!(out, "<decision id=\"{}\"", escape(decision.id.as_str()));
    optional_attr(out, "name", &decision.name);
    out.push('>');
    for requirement in &decision.requirements {
        let element = match requirement.kind {
            DmnRequirementKind::Decision => "requiredDecision",
            DmnRequirementKind::Input => "requiredInput",
        };
        let _ = write!(
            out,
            "<informationRequirement><{element} href=\"#{}\"/></informationRequirement>",
            escape(requirement.target.as_str())
        );
    }
    if let Some(t) = &decision.table {
        table(out, t);
    }
    fragments(preserved, &decision.id).for_each(|xml| out.push_str(xml));
    out.push_str("</decision>");
}

/// Write one DMN 1.3 document as canonical XML: the DMN namespace as the
/// default namespace, the root's prefixed namespaces kept, input data and
/// decisions in id order, rules in their declared order, and each preserved
/// fragment written back unchanged at its owner element. Reading the result
/// gives the same document, so a round trip keeps [`dmn_model_digest`].
pub fn write_dmn(doc: &DmnDocument) -> Result<Vec<u8>, ProcessExchangeError> {
    if doc.decisions.is_empty() && doc.input_data.is_empty() {
        return Err(ProcessExchangeError::NotDeployable {
            reason: "the document has no decision and no input data".into(),
        });
    }
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = write!(out, "<definitions xmlns=\"{DMN_MODEL_NS}\"");
    for (prefix, iri) in &doc.namespaces {
        let _ = write!(out, " xmlns:{prefix}=\"{}\"", escape(iri.as_str()));
    }
    let _ = write!(out, " id=\"{}\"", escape(doc.id.as_str()));
    optional_attr(&mut out, "name", &doc.name);
    out.push('>');
    let mut inputs: Vec<_> = doc.input_data.iter().collect();
    inputs.sort_by(|a, b| a.id.cmp(&b.id));
    for data in inputs {
        let _ = write!(out, "<inputData id=\"{}\"", escape(data.id.as_str()));
        optional_attr(&mut out, "name", &data.name);
        out.push_str("/>");
    }
    let mut decisions: Vec<_> = doc.decisions.iter().collect();
    decisions.sort_by(|a, b| a.id.cmp(&b.id));
    for d in decisions {
        decision(&mut out, d, &doc.preserved);
    }
    fragments(&doc.preserved, &doc.id).for_each(|xml| out.push_str(xml));
    out.push_str("</definitions>\n");
    Ok(out.into_bytes())
}

/// The canonical model digest of a DMN document: SHA-256 hex of the model in
/// id order (rules keep their order), so equal models compare equal whatever
/// order their elements were written in.
pub fn dmn_model_digest(doc: &DmnDocument) -> String {
    let mut canonical = doc.clone();
    canonical.input_data.sort_by(|a, b| a.id.cmp(&b.id));
    canonical.decisions.sort_by(|a, b| a.id.cmp(&b.id));
    canonical
        .preserved
        .sort_by(|a, b| (&a.owner, &a.xml).cmp(&(&b.owner, &b.xml)));
    let text = serde_json::to_string(&canonical).unwrap_or_default();
    Digest256::sha256(text.as_bytes()).to_hex()
}
