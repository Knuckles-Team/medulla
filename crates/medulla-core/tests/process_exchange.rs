//! Native BPMN 2.0 read and export (MEDULLA-CQ-ORCH-R702.2, R702.8).

use medulla_core::process_exchange::bpmn_read::{parse_bpmn, BpmnLimits};
use medulla_core::process_exchange::{ProcessExchangeError, BPMN_NODE_KINDS};

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
