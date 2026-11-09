//! SHACL coverage for the deployment-placement target vocabulary
//! (MEDULLA-PLACEMENT-R001.5).

use medulla_shacl::validate_turtle;

const SHAPES: &str = include_str!("../ontology/placement.shapes.ttl");

fn report(facts: &str) -> medulla_shacl::ValidationReport {
    let data = format!(
        r#"@prefix placement: <http://knuckles.team/kg/placement#> .
@prefix ex: <http://example.org/placement/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

{facts}
"#
    );
    validate_turtle(SHAPES, &data).expect("placement target Turtle should parse and validate")
}

// spec: MEDULLA-PLACEMENT-R001
#[test]
fn a_complete_target_passes_validation() {
    let result = report(
        r#"ex:swarm a placement:Target ;
    placement:supportsHa true ;
    placement:supportsVolumes true ;
    placement:maxReplicas 16 ;
    placement:relativeCost 4 ."#,
    );
    assert!(
        result.conforms,
        "a complete target must pass: {:?}",
        result.results
    );
}

// spec: MEDULLA-PLACEMENT-R001
#[test]
fn missing_relative_cost_fails_validation() {
    let result = report(
        r#"ex:helm a placement:Target ;
    placement:supportsHa true ;
    placement:supportsVolumes true ;
    placement:maxReplicas 64 ."#,
    );
    assert!(
        !result.conforms,
        "a target missing relativeCost must fail: {:?}",
        result.results
    );
    let expected_path = "<http://knuckles.team/kg/placement#relativeCost>";
    assert!(
        result
            .results
            .iter()
            .any(|violation| violation.path.as_deref() == Some(expected_path)),
        "the missing relativeCost property must be the reported violation: {:?}",
        result.results
    );
}
