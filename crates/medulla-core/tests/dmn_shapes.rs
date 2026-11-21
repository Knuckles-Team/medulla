//! SHACL coverage for DMN decision tables, decisions and rules
//! (MEDULLA-ORCH-VOCAB-R014.6).

use medulla_shacl::{validate_turtle, ValidationReport};

const SHAPES: &str = include_str!("../ontology/dmn-v1.shapes.ttl");

fn report(facts: &str) -> ValidationReport {
    let data = format!(
        r#"@prefix eg: <https://knuckles-team.github.io/ontology/core#> .
@prefix ex: <http://example.org/dmn/> .

{facts}
"#
    );
    validate_turtle(SHAPES, &data).expect("DMN Turtle should parse and validate")
}

fn messages(report: &ValidationReport) -> Vec<String> {
    report
        .results
        .iter()
        .filter_map(|result| result.message.clone())
        .collect()
}

const CLEAN: &str = r#"
ex:t a eg:DmnDecisionTable ; eg:dmnHitPolicy "COLLECT" ; eg:dmnAggregation "SUM" ;
    eg:dmnDecision "http://example.org/dmn/d1" .
ex:r a eg:DmnRule ; eg:dmnTable "http://example.org/dmn/t" ; eg:dmnRuleIndex 0 .
ex:d1 a eg:DmnDecision ; eg:dmnRequiresDecision ex:d2 .
ex:d2 a eg:DmnDecision .
"#;

// spec: MEDULLA-ORCH-VOCAB-R014.6
#[test]
fn dmn_shapes_refuse_planted_faults() {
    let clean = report(CLEAN);
    assert!(clean.conforms, "a clean model conforms: {:?}", clean.results);

    // A table with no hit policy.
    let no_policy = report("ex:t a eg:DmnDecisionTable .");
    assert!(!no_policy.conforms, "a table needs its hit policy");
    assert!(no_policy
        .results
        .iter()
        .any(|result| result.path.as_deref().is_some_and(|p| p.contains("dmnHitPolicy"))));

    // An aggregation under a hit policy other than COLLECT.
    let aggregated = report(
        r#"ex:t a eg:DmnDecisionTable ; eg:dmnHitPolicy "FIRST" ; eg:dmnAggregation "SUM" ."#,
    );
    assert!(!aggregated.conforms);
    assert!(messages(&aggregated)
        .contains(&"A dmnAggregation appears only with the COLLECT hit policy.".to_string()));

    // A two-decision cycle.
    let cycle = report(
        r#"ex:d1 a eg:DmnDecision ; eg:dmnRequiresDecision ex:d2 .
ex:d2 a eg:DmnDecision ; eg:dmnRequiresDecision ex:d1 ."#,
    );
    assert!(!cycle.conforms);
    assert!(messages(&cycle)
        .contains(&"A dmnRequiresDecision chain does not return to its start.".to_string()));

    // A rule with no table.
    let orphan = report("ex:r a eg:DmnRule ; eg:dmnRuleIndex 0 .");
    assert!(!orphan.conforms, "a rule names its table");
    assert!(orphan
        .results
        .iter()
        .any(|result| result.path.as_deref().is_some_and(|p| p.contains("dmnTable"))));
}
