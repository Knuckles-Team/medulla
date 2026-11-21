//! SHACL shapes that mirror the spec-standard row checks (MEDULLA-SDLC-R014.11):
//! each check has a passing and a failing fixture, and the file is registered as
//! the core source `sdd-row-shapes`.

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_shacl::validate_turtle;

const SHAPES: &str = include_str!("../ontology/sdd-row-v1.shapes.ttl");
const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

fn report(facts: &str) -> medulla_shacl::ValidationReport {
    let data = format!(
        "@prefix eg: <https://knuckles-team.github.io/ontology/core#> .\n\
         @prefix ex: <http://example.org/row/> .\n\n{facts}\n"
    );
    validate_turtle(SHAPES, &data).expect("row Turtle parses and validates")
}

/// The report entries naming `code`.
fn entries(report: &medulla_shacl::ValidationReport, code: &str) -> usize {
    report
        .results
        .iter()
        .filter(|r| r.message.as_deref().is_some_and(|m| m.starts_with(code)))
        .count()
}

fn passes(facts: &str, code: &str) {
    let result = report(facts);
    assert_eq!(entries(&result, code), 0, "{code}: {:?}", result.results);
}

fn fails(facts: &str, code: &str) {
    let result = report(facts);
    assert!(
        entries(&result, code) > 0,
        "{code} expected: {:?}",
        result.results
    );
}

// spec: MEDULLA-SDLC-R014.11
#[test]
fn each_check_has_a_passing_and_a_failing_fixture() {
    // A row has at least one bound test.
    passes(
        "ex:r a eg:Requirement ; eg:boundTest ex:t .",
        "ROW_TEST_UNBOUND",
    );
    fails("ex:r a eg:Requirement .", "ROW_TEST_UNBOUND");

    // A seam is named on both sides.
    passes(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:seamProvider ex:p ; eg:seamConsumer ex:c .",
        "SEAM_SIDE_MISSING",
    );
    fails(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:seamProvider ex:p .",
        "SEAM_SIDE_MISSING",
    );
    fails(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:seamConsumer ex:c .",
        "SEAM_SIDE_MISSING",
    );

    // Each Depends target is a row.
    passes(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:dependsOnRow ex:q .\n\
         ex:q a eg:Requirement ; eg:boundTest ex:t .",
        "DEPENDS_ON_UNRESOLVED",
    );
    fails(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:dependsOnRow ex:missing .",
        "DEPENDS_ON_UNRESOLVED",
    );

    // A LANDED or VERIFIED row has at least one commit.
    passes(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:rowStatus ex:s ; eg:implementedByCommit ex:c .\n\
         ex:s a eg:LandedStatus .",
        "LANDED_WITHOUT_COMMIT",
    );
    fails(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:rowStatus ex:s .\n\
         ex:s a eg:LandedStatus .",
        "LANDED_WITHOUT_COMMIT",
    );
    fails(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:rowStatus ex:s .\n\
         ex:s a eg:VerifiedStatus .",
        "LANDED_WITHOUT_COMMIT",
    );

    // A VERIFIED row has a test that exists.
    passes(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:rowStatus ex:s ; \
         eg:implementedByCommit ex:c ; eg:verifiedBy ex:t .\n\
         ex:s a eg:VerifiedStatus .",
        "VERIFIED_WITHOUT_TEST",
    );
    fails(
        "ex:r a eg:Requirement ; eg:boundTest ex:t ; eg:rowStatus ex:s ; \
         eg:implementedByCommit ex:c .\n\
         ex:s a eg:VerifiedStatus .",
        "VERIFIED_WITHOUT_TEST",
    );

    // A commit has one 40-character hash.
    let good = report(&format!("ex:c a eg:Commit ; eg:commitHash \"{HASH}\" ."));
    assert!(good.conforms, "{:?}", good.results);
    assert!(!report("ex:c a eg:Commit ; eg:commitHash \"abc123\" .").conforms);
    assert!(
        !report(&format!(
            "ex:c a eg:Commit ; eg:commitHash \"{HASH}\", \"{}\" .",
            HASH.replace('0', "1")
        ))
        .conforms
    );
}

// spec: MEDULLA-SDLC-R014.11
#[test]
fn the_row_shapes_are_a_core_source() {
    let sources = GraphSchemaSources::default();
    let (_, document) = sources
        .shapes()
        .find(|(id, _)| *id == "core:sdd-row-shapes@1")
        .expect("sdd-row-shapes is a registered core source");
    assert_eq!(document, SHAPES);
    assert!(
        !sources
            .ontologies()
            .any(|(id, _)| id == "core:sdd-row-shapes@1"),
        "the source carries shapes only"
    );
}
