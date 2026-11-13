use super::*;
use medulla_decision_types::decision::load_set::{LoadSetAnswer, DEFAULT_LOAD_SET_GRAPH};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, PremiseRef};

// spec: MEDULLA-DECISION-ENGINE-R176.1
#[test]
fn load_set_round_trips() {
    let question: LoadSetQuestion =
        serde_json::from_value(serde_json::json!({ "job_text": "triage github issues" })).unwrap();
    assert_eq!((question.max_tools, question.max_agents), (32, 4));
    let question = LoadSetQuestion {
        job_embedding: vec![0.25, -0.5],
        required_capabilities: vec!["urn:cap:issues.read".into()],
        max_tools: 128,
        embedding_version: "space-1".into(),
        ..question
    };
    let value = serde_json::to_value(&question).unwrap();
    assert_eq!(
        serde_json::from_value::<LoadSetQuestion>(value.clone()).unwrap(),
        question
    );
    let mut over = value;
    over["max_tools"] = serde_json::json!(129);
    assert!(serde_json::from_value::<LoadSetQuestion>(over).is_err());
    let answer = LoadSetAnswer {
        tools: vec!["github.list_issues".into()],
        skills: vec!["skill:issue-triage".into()],
        agents: vec![AgentStart {
            agent_iri: "urn:agent:triager".into(),
            prompt_iri: "urn:prompt:triage".into(),
            model_profile: "profile:small".into(),
        }],
        graph_revision: 42,
        premises: vec![PremiseRef {
            subject: "github.list_issues".into(),
            fact: "designated_for=urn:cap:issues.read".into(),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: "required_capabilities".into(),
            },
        }],
    };
    let value = serde_json::to_value(&answer).unwrap();
    assert_eq!(
        serde_json::from_value::<LoadSetAnswer>(value).unwrap(),
        answer
    );
    assert_eq!(
        serde_json::to_value(medulla_decision_types::decision::statistical::QuestionKind::LoadSet)
            .unwrap(),
        "load_set"
    );
}

fn tool(id: &str, score: i64, caps: &[&str], pack: &str) -> LoadComponent {
    LoadComponent {
        id: id.into(),
        kind: EntityKind::Tool,
        score_q32: score,
        capabilities: caps.iter().map(|c| c.to_string()).collect(),
        pack: pack.into(),
    }
}

fn skill(id: &str, score: i64, caps: &[&str], pack: &str) -> LoadComponent {
    LoadComponent {
        kind: EntityKind::Skill,
        ..tool(id, score, caps, pack)
    }
}

fn agent(iri: &str, tools: &[&str], skills: &[&str]) -> AgentCandidate {
    AgentCandidate {
        agent_iri: iri.into(),
        prompt_iri: format!("{iri}:prompt"),
        model_profile: "profile:small".into(),
        tools: tools.iter().map(|t| t.to_string()).collect(),
        skills: skills.iter().map(|s| s.to_string()).collect(),
    }
}

fn question(max_tools: u16, max_agents: u16) -> LoadSetQuestion {
    LoadSetQuestion {
        job_text: "list docker containers".into(),
        job_embedding: vec![1.0],
        required_capabilities: vec!["cap:a".into(), "cap:b".into()],
        max_tools,
        max_agents,
        graph: DEFAULT_LOAD_SET_GRAPH.into(),
        embedding_version: "space-1".into(),
    }
}

fn inputs() -> (
    Vec<Vec<LoadComponent>>,
    Vec<LoadComponent>,
    Vec<AgentCandidate>,
) {
    let per_capability = vec![
        vec![
            tool("t.a1", 90, &["cap:a"], "p1"),
            tool("t.a2", 50, &["cap:a"], "p1"),
        ],
        // The best tool of cap:b scores below every cap:a tool: it is reserved.
        vec![tool("t.b1", 10, &["cap:b"], "p2")],
    ];
    let overall = vec![
        tool("t.a1", 95, &["cap:a"], "p1"),
        tool("t.z9", 60, &["cap:z"], "p3"),
        tool("t.z8", 60, &["cap:z"], "p3"),
        skill("s.a", 70, &["cap:a"], "p1"),
        skill("s.other-pack", 80, &["cap:a"], "p9"),
    ];
    let agents = vec![
        agent("urn:agent:two", &["t.a1", "t.b1"], &["s.held"]),
        agent("urn:agent:one", &["t.a1"], &[]),
        agent("urn:agent:none", &["t.unrelated"], &["s.never"]),
    ];
    (per_capability, overall, agents)
}

// spec: MEDULLA-DECISION-ENGINE-R176.2
#[test]
fn bounded_union_in_stable_order() {
    let (per_capability, overall, agents) = inputs();
    let merge = merge_load_set(
        &per_capability,
        &overall,
        &agents,
        &question(3, 1),
        &LoadSetKnobs::default(),
    );
    // Max score per component id: t.a1 keeps 95, not 90.
    // The reserved best of cap:a (t.a1) and cap:b (t.b1) are in; the fill takes t.z8 (60, id first).
    let ids: Vec<&str> = merge.tools.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["t.a1", "t.z8", "t.b1"]);
    assert_eq!(merge.tools[0].score_q32, 95);
    // Five tool candidates existed (a1, a2, b1, z8, z9); two are cut.
    assert_eq!(merge.excluded, 2);
    // At most max_agents agents, the one holding the most chosen tools.
    assert_eq!(merge.agents.len(), 1);
    assert_eq!(merge.agents[0].agent_iri, "urn:agent:two");
    assert_eq!(merge.agents[0].prompt_iri, "urn:agent:two:prompt");
    // Pool skill from the pack of a chosen tool with a shared capability, then the agent's skill.
    assert_eq!(merge.skills, ["s.a", "s.held"]);
    // The skill cap applies.
    let capped = merge_load_set(
        &per_capability,
        &overall,
        &agents,
        &question(3, 1),
        &LoadSetKnobs {
            per_capability_k: 8,
            max_skills: 1,
        },
    );
    assert_eq!(capped.skills, ["s.a"]);
    // No tool is chosen above max_tools even when every capability is reserved.
    let tight = merge_load_set(
        &per_capability,
        &overall,
        &agents,
        &question(1, 4),
        &LoadSetKnobs::default(),
    );
    assert_eq!(tight.tools.len(), 1);
}

// spec: MEDULLA-DECISION-ENGINE-R176.2
#[test]
fn merge_ignores_input_order() {
    let (per_capability, overall, agents) = inputs();
    let baseline = merge_load_set(
        &per_capability,
        &overall,
        &agents,
        &question(4, 2),
        &LoadSetKnobs::default(),
    );
    let mut shuffled_caps = per_capability.clone();
    for list in &mut shuffled_caps {
        list.reverse();
    }
    let mut shuffled_overall = overall.clone();
    shuffled_overall.reverse();
    let mut shuffled_agents = agents.clone();
    shuffled_agents.reverse();
    let shuffled = merge_load_set(
        &shuffled_caps,
        &shuffled_overall,
        &shuffled_agents,
        &question(4, 2),
        &LoadSetKnobs::default(),
    );
    assert_eq!(baseline, shuffled);
}
