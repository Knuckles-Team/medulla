//! Native BPMN 2.0 read and export (MEDULLA-CQ-ORCH-R702.2, R702.8).

use medulla_core::process_exchange::bpmn_read::{parse_bpmn, BpmnLimits};
use medulla_core::process_exchange::workflow_bpmn::workflow_to_bpmn;
use medulla_core::process_exchange::{ProcessExchangeError, BPMN_NODE_KINDS};
use medulla_core::process_import::canonicalize;
use medulla_types::process_import::{
    LoweredFlow, LoweredProcess, LoweredStep, ProcessSourceSystem, StepKind,
};

const EVERY_KIND: &[u8] = include_bytes!("fixtures/process_exchange/every_kind.bpmn");

// spec: MEDULLA-CQ-ORCH-R702.2
#[test]
fn bpmn_reader_keeps_every_element() {
    let doc = parse_bpmn(EVERY_KIND, BpmnLimits::default()).unwrap();
    assert_eq!(doc.id, "defs1");
    assert_eq!(
        doc.collaborations[0].participants,
        vec![("p1".into(), "proc1".into())]
    );
    let process = &doc.processes[0];
    assert_eq!(process.name, "Every kind");
    let kinds: Vec<&str> = process.nodes.iter().map(|n| n.kind.as_str()).collect();
    for kind in BPMN_NODE_KINDS {
        assert!(kinds.contains(kind), "{kind} kept");
    }
    let user = process.nodes.iter().find(|n| n.id == "user1").unwrap();
    assert_eq!(user.name, "User & review");
    let rule = process.nodes.iter().find(|n| n.id == "rule1").unwrap();
    assert_eq!(rule.attributes["decisionRef"], "limits");
    let xor = process.nodes.iter().find(|n| n.id == "xor1").unwrap();
    assert_eq!(xor.attributes["default"], "flow3");
    let catch = process.nodes.iter().find(|n| n.id == "catch1").unwrap();
    assert_eq!(catch.event_definitions[0].kind, "timerEventDefinition");
    let boundary = process.nodes.iter().find(|n| n.id == "boundary1").unwrap();
    assert_eq!(boundary.attributes["attachedToRef"], "user1");
    assert_eq!(process.lanes[0].flow_node_refs, vec!["user1", "manual1"]);
    assert_eq!(process.flows.len(), 3);
    assert_eq!(process.flows[1].condition.as_deref(), Some("amount > 1000"));
    assert_eq!(process.flows[1].name, "high");
    // Elements without a model of their own keep their XML and owner id.
    let owners: Vec<&str> = process.preserved.iter().map(|p| p.owner.as_str()).collect();
    assert!(owners.contains(&"user1"), "extension kept on its task");
    assert!(owners.contains(&"sub1"), "sub-process content kept");
    assert!(owners.contains(&"catch1"), "timer duration kept");
    assert!(process.preserved.iter().any(|p| p.xml.contains("ext:form")));
    assert!(doc
        .preserved
        .iter()
        .any(|p| p.owner == "defs1" && p.xml.contains("BPMNDiagram")));
}

// spec: MEDULLA-CQ-ORCH-R702.2
#[test]
fn bpmn_reader_refuses_entities_namespaces_and_limits() {
    let planted = br#"<?xml version="1.0"?>
<!DOCTYPE definitions [<!ENTITY xxe SYSTEM "file:///etc/hostname">]>
<definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL" id="d"><process id="p" name="&xxe;"/></definitions>"#;
    assert_eq!(
        parse_bpmn(planted, BpmnLimits::default()),
        Err(ProcessExchangeError::EntityRefused)
    );
    let undeclared = br#"<definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL" id="d"><process id="p"><sequenceFlow id="f"><conditionExpression>&xxe;</conditionExpression></sequenceFlow></process></definitions>"#;
    assert_eq!(
        parse_bpmn(undeclared, BpmnLimits::default()),
        Err(ProcessExchangeError::EntityRefused)
    );
    let foreign = br#"<definitions xmlns="urn:not-bpmn" id="d"/>"#;
    assert!(matches!(
        parse_bpmn(foreign, BpmnLimits::default()),
        Err(ProcessExchangeError::NamespaceUnknown { namespace }) if namespace == "urn:not-bpmn"
    ));
    let small = BpmnLimits {
        max_bytes: 64,
        max_depth: 64,
    };
    assert!(matches!(
        parse_bpmn(EVERY_KIND, small),
        Err(ProcessExchangeError::TooLarge { .. })
    ));
    let shallow = BpmnLimits {
        max_bytes: 1 << 20,
        max_depth: 2,
    };
    assert!(matches!(
        parse_bpmn(EVERY_KIND, shallow),
        Err(ProcessExchangeError::TooLarge { .. })
    ));
    let broken = b"<definitions xmlns=\"http://www.omg.org/spec/BPMN/20100524/MODEL\">\n<process id=\"p\"></task></definitions>";
    assert!(matches!(
        parse_bpmn(broken, BpmnLimits::default()),
        Err(ProcessExchangeError::Malformed { line: 2, .. })
    ));
}

fn lowered(id: &str, kind: StepKind, label: &str, role: Option<&str>) -> LoweredStep {
    LoweredStep {
        element_id: id.into(),
        kind,
        gate: None,
        label: label.into(),
        role: role.map(Into::into),
        reference: None,
    }
}

fn is_xml_name(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

// spec: MEDULLA-CQ-ORCH-R702.8
#[test]
fn workflow_exports_as_deployable_bpmn() {
    let steps = vec![
        lowered("a", StepKind::Human, "Review", Some("Finance")),
        lowered("b", StepKind::Agent, "Book", None),
        lowered("c", StepKind::Decision, "Check", Some("Finance")),
    ];
    let flows = ["a:b", "b:c"]
        .iter()
        .map(|pair| {
            let (source, target) = pair.split_once(':').unwrap();
            LoweredFlow {
                source: source.into(),
                target: target.into(),
                condition: None,
                default: false,
            }
        })
        .collect();
    let workflow = canonicalize(LoweredProcess {
        name: "Book invoice".into(),
        system: ProcessSourceSystem::ArisEpc,
        model_id: "m".into(),
        steps,
        flows,
        annex: Vec::new(),
    })
    .unwrap();
    let doc = workflow_to_bpmn(&workflow).unwrap();
    let process = &doc.processes[0];
    let count = |kind: &str| process.nodes.iter().filter(|n| n.kind == kind).count();
    assert_eq!(count("startEvent"), 1);
    assert_eq!(count("endEvent"), 1);
    assert_eq!(count("userTask"), 1);
    assert_eq!(count("serviceTask"), 1);
    assert_eq!(count("businessRuleTask"), 1);
    assert_eq!(process.lanes.len(), 1);
    assert_eq!(process.lanes[0].flow_node_refs.len(), 2);
    assert_eq!(process.flows.len(), 4);
    for id in process
        .nodes
        .iter()
        .map(|n| n.id.as_str())
        .chain(process.flows.iter().map(|f| f.id.as_str()))
        .chain([doc.id.as_str(), process.id.as_str()])
    {
        assert!(is_xml_name(id), "{id} is a valid XML name");
    }
    let mut empty = workflow.clone();
    empty.steps.clear();
    empty.flows.clear();
    assert!(matches!(
        workflow_to_bpmn(&empty),
        Err(ProcessExchangeError::NotDeployable { .. })
    ));
}

// spec: MEDULLA-CQ-ORCH-R702.6
#[test]
fn bpmn_write_then_read_keeps_digest() {
    use medulla_core::process_exchange::bpmn_lower::{bpmn_kind_annotations, bpmn_source_graph};
    use medulla_core::process_exchange::bpmn_write::write_bpmn;
    use medulla_core::process_import::compile_process;

    let digest = |bytes: &[u8]| {
        let doc = parse_bpmn(bytes, BpmnLimits::default()).unwrap();
        let graph = bpmn_source_graph(&doc).unwrap();
        compile_process(&graph, bpmn_kind_annotations().unwrap())
            .unwrap()
            .digest
    };
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_equivalence");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("bpmn") {
            continue;
        }
        let original = std::fs::read(&path).unwrap();
        let written = write_bpmn(&parse_bpmn(&original, BpmnLimits::default()).unwrap()).unwrap();
        assert_eq!(digest(&written), digest(&original), "{}", path.display());
        let again = write_bpmn(&parse_bpmn(&written, BpmnLimits::default()).unwrap()).unwrap();
        assert_eq!(again, written, "writing is canonical: {}", path.display());
        seen += 1;
    }
    assert_eq!(seen, 7);

    // Every element kind and every preserved fragment survives a round trip.
    let doc = parse_bpmn(EVERY_KIND, BpmnLimits::default()).unwrap();
    let written = write_bpmn(&doc).unwrap();
    assert!(String::from_utf8_lossy(&written).contains("isExecutable=\"true\""));
    let back = parse_bpmn(&written, BpmnLimits::default()).unwrap();
    let (a, b) = (&doc.processes[0], &back.processes[0]);
    assert_eq!(b.nodes.len(), a.nodes.len());
    assert_eq!(b.flows, {
        let mut flows = a.flows.clone();
        flows.sort_by(|x, y| x.id.cmp(&y.id));
        flows
    });
    assert_eq!(b.lanes, a.lanes);
    assert_eq!(b.preserved.len(), a.preserved.len());
    assert_eq!(back.preserved.len(), doc.preserved.len());
    assert_eq!(back.collaborations, doc.collaborations);
}

const DMN_FIXTURE: &str = r##"<?xml version="1.0"?>
<definitions xmlns="https://www.omg.org/spec/DMN/20191111/MODEL/" xmlns:ext="urn:ext" id="defs" name="Limits">
  <inputData id="amount" name="Amount"/>
  <decision id="tier" name="Tier">
    <informationRequirement><requiredInput href="#amount"/></informationRequirement>
    <informationRequirement><requiredDecision href="#base"/></informationRequirement>
    <ext:note>kept</ext:note>
    <decisionTable id="t1" hitPolicy="COLLECT" aggregation="SUM">
      <input id="i1" label="Amount"><inputExpression typeRef="number"><text>amount &gt; 10</text></inputExpression></input>
      <output id="o1" name="fee" typeRef="number"/>
      <rule id="r1"><inputEntry><text>&lt; 100</text></inputEntry><outputEntry><text>1.5</text></outputEntry></rule>
      <rule id="r2"><inputEntry><text>&gt;= 100</text></inputEntry><outputEntry><text>2</text></outputEntry></rule>
    </decisionTable>
  </decision>
  <decision id="base" name="Base"><decisionTable id="t2"><input id="i2"><inputExpression><text>x</text></inputExpression></input><output id="o2" name="y"/></decisionTable></decision>
  <ext:extension/>
</definitions>"##;

// spec: MEDULLA-CQ-ORCH-R702.4
#[test]
fn dmn_reader_keeps_tables_and_requirements() {
    use medulla_core::process_exchange::dmn_read::{parse_dmn, DmnRequirementKind};
    use medulla_core::process_exchange::xml_tree::XmlLimits;
    let doc = parse_dmn(DMN_FIXTURE.as_bytes(), XmlLimits::default()).unwrap();
    assert_eq!(doc.id, "defs");
    assert_eq!(doc.input_data[0].name, "Amount");
    let tier = &doc.decisions[0];
    let kinds: Vec<_> = tier
        .requirements
        .iter()
        .map(|r| (r.kind, r.target.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (DmnRequirementKind::Input, "amount"),
            (DmnRequirementKind::Decision, "base")
        ]
    );
    let table = tier.table.as_ref().unwrap();
    assert_eq!(table.hit_policy, "COLLECT");
    assert_eq!(table.aggregation.as_deref(), Some("SUM"));
    assert_eq!(table.inputs[0].expression, "amount > 10");
    assert_eq!(table.outputs[0].expression, "fee");
    assert_eq!(table.rules[0].input_entries, vec!["< 100"]);
    assert_eq!(table.rules[1].output_entries, vec!["2"]);
    // A table without a stated hit policy is UNIQUE.
    assert_eq!(
        doc.decisions[1].table.as_ref().unwrap().hit_policy,
        "UNIQUE"
    );
    // Unmodelled elements keep their XML and owner.
    assert!(doc
        .preserved
        .iter()
        .any(|p| p.owner == "tier" && p.xml.contains("kept")));
    assert!(doc
        .preserved
        .iter()
        .any(|p| p.owner == "defs" && p.xml.contains("ext:extension")));

    let planted = br#"<!DOCTYPE d [<!ENTITY x "y">]><definitions xmlns="https://www.omg.org/spec/DMN/20191111/MODEL/"/>"#;
    assert_eq!(
        parse_dmn(planted, XmlLimits::default()),
        Err(ProcessExchangeError::EntityRefused)
    );
    assert!(matches!(
        parse_dmn(b"<definitions xmlns=\"urn:other\"/>", XmlLimits::default()),
        Err(ProcessExchangeError::NamespaceUnknown { .. })
    ));
    let small = XmlLimits {
        max_bytes: 32,
        max_depth: 64,
    };
    assert!(matches!(
        parse_dmn(DMN_FIXTURE.as_bytes(), small),
        Err(ProcessExchangeError::TooLarge { .. })
    ));
}

// spec: MEDULLA-CQ-ORCH-R702.5
#[test]
fn dmn_mapping_keeps_rule_order() {
    use medulla_core::architecture_exchange::graph::ExchangeMutation;
    use medulla_core::process_exchange::dmn_graph::{dmn_mutations, element_iri};
    use medulla_core::process_exchange::dmn_read::parse_dmn;
    use medulla_core::process_exchange::xml_tree::XmlLimits;

    // One table of five rules whose ids do not sort in declared order.
    let rules = ["r5", "r2", "r4", "r1", "r3"];
    let mut body = String::new();
    for (index, id) in rules.iter().enumerate() {
        body.push_str(&format!(
            "<rule id=\"{id}\"><inputEntry><text>&lt; {index}</text></inputEntry>\
             <outputEntry><text>{index}</text></outputEntry></rule>"
        ));
    }
    let xml = format!(
        "<?xml version=\"1.0\"?><definitions xmlns=\"https://www.omg.org/spec/DMN/20191111/MODEL/\" \
         id=\"defs\" name=\"Fees\"><inputData id=\"amount\" name=\"Amount\"/>\
         <decision id=\"base\" name=\"Base\"/>\
         <decision id=\"tier\" name=\"Tier\">\
         <informationRequirement><requiredInput href=\"#amount\"/></informationRequirement>\
         <informationRequirement><requiredDecision href=\"#base\"/></informationRequirement>\
         <decisionTable id=\"t\" hitPolicy=\"COLLECT\" aggregation=\"SUM\">\
         <input id=\"i1\" label=\"Amount\"><inputExpression><text>amount</text></inputExpression></input>\
         <output id=\"o1\" name=\"fee\"/>{body}</decisionTable></decision></definitions>"
    );
    let doc = parse_dmn(xml.as_bytes(), XmlLimits::default()).unwrap();
    let model = "model:fees";
    let out = dmn_mutations(&doc, model);
    assert_eq!(out, dmn_mutations(&doc, model), "equal input, equal output");

    let nodes: Vec<(&str, &str, &std::collections::BTreeMap<String, String>)> = out
        .iter()
        .filter_map(|m| match m {
            ExchangeMutation::UpsertNode {
                id,
                label,
                properties,
            } => Some((id.as_str(), label.as_str(), properties)),
            ExchangeMutation::Link { .. } => None,
        })
        .collect();
    // Each rule is one DmnRule node carrying its declared position.
    for (declared, id) in rules.iter().enumerate() {
        let iri = element_iri(model, id);
        let (_, label, properties) = nodes.iter().find(|(n, _, _)| *n == iri).unwrap();
        assert_eq!(*label, "DmnRule");
        assert_eq!(properties["dmnRuleIndex"], declared.to_string());
    }
    // Reading the rules back by index recovers the declared order.
    let mut by_index: Vec<_> = nodes
        .iter()
        .filter(|(_, label, _)| *label == "DmnRule")
        .map(|(iri, _, p)| (p["dmnRuleIndex"].parse::<usize>().unwrap(), *iri))
        .collect();
    by_index.sort();
    let recovered: Vec<_> = by_index
        .into_iter()
        .map(|(_, iri)| iri.to_owned())
        .collect();
    let declared: Vec<_> = rules.iter().map(|id| element_iri(model, id)).collect();
    assert_eq!(recovered, declared);
    // Output order is element id order: the rule nodes appear as r1..r5.
    let rule_order: Vec<_> = nodes
        .iter()
        .filter(|(_, label, _)| *label == "DmnRule")
        .map(|(iri, _, _)| *iri)
        .collect();
    let mut sorted = rule_order.clone();
    sorted.sort();
    assert_eq!(rule_order, sorted);
    // Classes and requirement edges.
    for label in [
        "DmnDecisionRequirements",
        "DmnDecision",
        "DmnDecisionTable",
        "DmnInputClause",
        "DmnOutputClause",
        "DmnInputData",
    ] {
        assert!(nodes.iter().any(|(_, l, _)| *l == label), "{label}");
    }
    let links: Vec<_> = out
        .iter()
        .filter_map(|m| match m {
            ExchangeMutation::Link {
                source,
                target,
                predicate,
            } => Some((source.as_str(), target.as_str(), predicate.as_str())),
            ExchangeMutation::UpsertNode { .. } => None,
        })
        .collect();
    let tier = element_iri(model, "tier");
    assert!(links.contains(&(
        tier.as_str(),
        element_iri(model, "amount").as_str(),
        "dmnRequiresInput"
    )));
    assert!(links.contains(&(
        tier.as_str(),
        element_iri(model, "base").as_str(),
        "dmnRequiresDecision"
    )));
}
