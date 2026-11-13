//! SHACL coverage for team records (MEDULLA-DECISION-ENGINE-R170.3).

use medulla_shacl::validate_turtle;

const SHAPES: &str = include_str!("../ontology/a2a-v1.shapes.ttl");

fn report(facts: &str) -> medulla_shacl::ValidationReport {
    let data = format!(
        r#"@prefix eg: <https://knuckles-team.github.io/ontology/core#> .
@prefix ex: <http://example.org/team/> .

{facts}
"#
    );
    validate_turtle(SHAPES, &data).expect("team Turtle should parse and validate")
}

#[test]
fn team_member_bound_to_a_team_conforms() {
    let result = report("ex:member a eg:TeamMember ; eg:boundToTeam ex:ops .");
    assert!(result.conforms, "bound member: {:?}", result.results);
}

#[test]
fn team_member_without_a_binding_is_refused() {
    let result = report("ex:member a eg:TeamMember .");
    assert!(!result.conforms, "an unbound member must fail the shape");
}

#[test]
fn team_member_bound_to_a_literal_is_refused() {
    let result = report(r#"ex:member a eg:TeamMember ; eg:boundToTeam "ops" ."#);
    assert!(!result.conforms, "the binding must be an IRI");
}
