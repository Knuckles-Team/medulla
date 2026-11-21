//! Contract tests for the action provenance vocabulary and its shapes companion
//! (MEDULLA-DECISION-ENGINE-R869.1): the `ActionRecord`, `FactUse` and `RuleUse`
//! classes and their parents, and the shapes that refuse a record with no kind, an
//! unknown kind, no actor, neither used facts nor a stated reason, or a malformed
//! fact digest.

use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedNode, Triple};
use medulla_shacl::validate_turtle;

const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const PREFIXES: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                        @prefix ex: <http://example.org/action/> .\n";

const ONTOLOGY: &str = include_str!("../ontology/action-v1.ttl");
const SHAPES: &str = include_str!("../ontology/action-v1.shapes.ttl");

const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn has_subclass(triples: &[Triple], class: &str, parent: &str) -> bool {
    triples.contains(&Triple::new(
        NamedNode::new(format!("{KG}{class}")).unwrap(),
        NamedNode::new(RDFS_SUBCLASS).unwrap(),
        NamedNode::new(format!("{KG}{parent}")).unwrap(),
    ))
}

/// Validates `data` (turtle terms under the `:` and `ex:` prefixes) against the shapes.
fn conforms(data: &str) -> (bool, Vec<String>) {
    let report = validate_turtle(SHAPES, &format!("{PREFIXES}{data}"))
        .expect("the data parses before validation");
    let paths = report
        .results
        .iter()
        .map(|result| result.path.clone().unwrap_or_default())
        .collect();
    (report.conforms, paths)
}

fn record(kind: Option<&str>, actor: bool, inputs: &str) -> String {
    let mut text = String::from("ex:a1 a :ActionRecord ; :idempotencyKey \"k1\" ");
    if let Some(kind) = kind {
        text.push_str(&format!("; :actionKind \"{kind}\" "));
    }
    if actor {
        text.push_str("; :actor ex:agent1 ");
    }
    text.push_str(inputs);
    text.push_str(" .\n");
    text
}

const USES_FACT: &str = "; :usedFact ex:f1 ";

fn fact(digest: &str) -> String {
    format!(
        "ex:f1 a :FactUse ; :factNode \"node-1\" ; :factRevision 3 ; :factDigest \"{digest}\" .\n"
    )
}

// spec: MEDULLA-DECISION-ENGINE-R869.1
#[test]
fn action_record_terms_and_shape() {
    let triples = parse_turtle(ONTOLOGY).expect("the action ontology parses");
    assert!(has_subclass(&triples, "ActionRecord", "ActionInvocation"));
    assert!(has_subclass(&triples, "FactUse", "DomainEntity"));
    assert!(has_subclass(&triples, "RuleUse", "DomainEntity"));
    parse_turtle(SHAPES).expect("the action shapes parse");

    // A complete record conforms: every kind, with used facts or a stated reason.
    for kind in ["tool_call", "approval", "filing", "money_movement", "publish"] {
        let data = format!("{}{}", record(Some(kind), true, USES_FACT), fact(DIGEST));
        assert_eq!(conforms(&data), (true, vec![]), "{kind}");
    }
    let no_inputs = record(
        Some("publish"),
        true,
        "; :noInputs \"the action reads no fact\" ",
    );
    assert!(conforms(&no_inputs).0);

    // No kind, an unknown kind, no actor, and neither input nor reason all fail.
    for (name, data, path) in [
        ("no kind", record(None, true, USES_FACT) + &fact(DIGEST), "actionKind"),
        (
            "unknown kind",
            record(Some("telepathy"), true, USES_FACT) + &fact(DIGEST),
            "actionKind",
        ),
        (
            "no actor",
            record(Some("filing"), false, USES_FACT) + &fact(DIGEST),
            "actor",
        ),
    ] {
        let (ok, paths) = conforms(&data);
        assert!(!ok, "{name} must not conform");
        assert!(paths.iter().any(|p| p.ends_with(path)), "{name}: {paths:?}");
    }
    assert!(
        !conforms(&record(Some("filing"), true, "")).0,
        "neither :usedFact nor :noInputs must not conform"
    );

    // A fact use whose digest lacks the sha256: form fails; so does a negative revision.
    let bad_digest = format!(
        "{}{}",
        record(Some("filing"), true, USES_FACT),
        fact(&DIGEST.replace("sha256:", ""))
    );
    let (ok, paths) = conforms(&bad_digest);
    assert!(!ok && paths.iter().any(|p| p.ends_with("factDigest")), "{paths:?}");
    let negative = format!(
        "{}ex:f1 a :FactUse ; :factNode \"n\" ; :factRevision -1 ; :factDigest \"{DIGEST}\" .\n",
        record(Some("filing"), true, USES_FACT)
    );
    assert!(!conforms(&negative).0);

    // A rule use pins the rule, its version and its digest.
    let rule = format!(
        "ex:r1 a :RuleUse ; :ruleNode \"rule-1\" ; :ruleVersion 2 ; :ruleDigest \"{DIGEST}\" .\n"
    );
    assert!(conforms(&rule).0);
    assert!(!conforms("ex:r2 a :RuleUse ; :ruleNode \"rule-1\" .\n").0);
}
