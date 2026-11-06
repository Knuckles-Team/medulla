//! MEDULLA-IDENTITY-R008.1: tool-set admission is attached to `assemble_for`
//! exactly once, after the candidate read and before the candidate facts.

// spec: MEDULLA-IDENTITY-R008.1
#[test]
fn attach_point_is_reached() {
    let source = include_str!("../assemble.rs");
    let body_start = source
        .find("async fn assemble_for(")
        .expect("assemble_for is defined");
    let body_end = source[body_start..]
        .find("\n}\n")
        .map(|offset| body_start + offset)
        .expect("assemble_for body ends");
    let body = &source[body_start..body_end];
    assert_eq!(
        body.matches("admit_tool_set(").count(),
        1,
        "enforce_scope runs once per assembly"
    );
    let candidates = body
        .find("store.assembly_candidates(")
        .expect("candidate read");
    let admit = body.find("admit_tool_set(").expect("admission call");
    let facts = body.find("candidate_facts(").expect("candidate facts");
    assert!(candidates < admit && admit < facts);
    let admit_fn = source
        .find("async fn admit_tool_set(")
        .expect("admit_tool_set is defined");
    assert!(source[admit_fn..].contains("medulla_isolation::isolation::enforce_scope("));

    // With no stored policy node, the plan's tool set is outside the empty bound.
    let core = medulla_graph_core::graph::GraphCore::new();
    let refusal =
        medulla_isolation::isolation::enforce_scope(&core, "worker", &["tool:a".to_string()])
            .expect_err("empty bound refuses every tool");
    assert_eq!(refusal.code(), "SCOPE_EXCEEDS_POLICY");
}
