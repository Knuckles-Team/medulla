//! Grounding against a class hierarchy (MEDULLA-INGEST-WORKER-R701.8).

use std::collections::{BTreeMap, BTreeSet};

use medulla_ingestion_types::extracted_fact::{EvidenceSpan, ExtractedFact, GroundingResult};
use medulla_ingestion_types::ingest_rules::{
    draft_from_profile, ground_fact, judge_proposal, subclass_closure, GroundingCache,
    IngestRefusal, MAX_HIERARCHY_STEPS, MAX_PROFILE_ENTRIES,
};
use medulla_ingestion_types::ingestion_wire::InstanceProfile;
use medulla_ingestion_types::ontology_proposal::{
    ProposalCheck, ProposalProvenance, ProposalThresholds,
};
use medulla_types::ontology_proposal_machine::ProposalState;

fn fact(subject: &str, predicate: &str, object: &str) -> ExtractedFact {
    ExtractedFact {
        subject: subject.into(),
        predicate: predicate.into(),
        object: object.into(),
        evidence: EvidenceSpan::new("doc:1", 0, 10).unwrap(),
        confidence: 0.9,
    }
}

fn edges(pairs: &[(&str, &str)]) -> BTreeMap<String, BTreeSet<String>> {
    let mut direct: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (class, parent) in pairs {
        direct
            .entry((*class).to_string())
            .or_default()
            .insert((*parent).to_string());
    }
    direct
}

// spec: MEDULLA-INGEST-WORKER-R701.8
/// Verifies MEDULLA-INGEST-WORKER-R701.8: a `Dog` subject grounds a predicate whose domain is
/// `Animal`, the grounded result keeps the declared classes, direction repair uses the same
/// check, and a cyclic or over-deep hierarchy refuses `ONTOLOGY_HIERARCHY_CYCLE`.
#[test]
fn subclass_entity_grounds_to_declared_domain_and_cycle_refuses() {
    let closure = subclass_closure(&edges(&[("Dog", "Mammal"), ("Mammal", "Animal")])).unwrap();
    assert_eq!(
        closure["Dog"],
        BTreeSet::from(["Mammal".to_string(), "Animal".to_string()])
    );
    let cache = GroundingCache {
        predicates: BTreeMap::from([(
            "owns".to_string(),
            ("Animal".to_string(), "Toy".to_string()),
        )]),
        classes: BTreeMap::from([
            ("Rex".to_string(), "Dog".to_string()),
            ("Bone".to_string(), "Toy".to_string()),
            ("Rock".to_string(), "Mineral".to_string()),
        ]),
        subclass_of: closure,
    };
    match ground_fact(fact("Rex", "owns", "Bone"), &cache).grounding {
        GroundingResult::Grounded {
            subject_class,
            object_class,
            direction_repaired,
            ..
        } => {
            assert_eq!(
                (subject_class.as_str(), object_class.as_str()),
                ("Animal", "Toy")
            );
            assert!(!direction_repaired);
        }
        other => panic!("a Dog grounds to the Animal domain, got {other:?}"),
    }
    // The swapped pair repairs by the same check.
    assert!(matches!(
        ground_fact(fact("Bone", "owns", "Rex"), &cache).grounding,
        GroundingResult::Grounded {
            direction_repaired: true,
            ..
        }
    ));
    // A class outside the hierarchy does not fit.
    assert_eq!(
        ground_fact(fact("Rock", "owns", "Bone"), &cache).grounding,
        GroundingResult::Ungrounded
    );
    // A cycle refuses with the stable code.
    let cycle = subclass_closure(&edges(&[("A", "B"), ("B", "C"), ("C", "A")])).unwrap_err();
    assert_eq!(cycle.code(), "ONTOLOGY_HIERARCHY_CYCLE");
    assert!(matches!(cycle, IngestRefusal::OntologyHierarchyCycle(_)));
    // A chain of exactly 64 steps closes; one more refuses.
    let chain = |links: usize| {
        let pairs: Vec<(String, String)> = (0..links)
            .map(|n| (format!("C{n}"), format!("C{}", n + 1)))
            .collect();
        let refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        edges(&refs)
    };
    assert!(subclass_closure(&chain(MAX_HIERARCHY_STEPS)).is_ok());
    assert_eq!(
        subclass_closure(&chain(MAX_HIERARCHY_STEPS + 1))
            .unwrap_err()
            .code(),
        "ONTOLOGY_HIERARCHY_CYCLE"
    );
}

fn profile(value: serde_json::Value) -> InstanceProfile {
    serde_json::from_value(value).unwrap()
}

fn provenance() -> ProposalProvenance {
    ProposalProvenance {
        source_ref: "instance_profile".into(),
        induced_by: "instance-profile".into(),
    }
}

// spec: MEDULLA-INGEST-WORKER-R002.8
/// Verifies MEDULLA-INGEST-WORKER-R002.8: a class with fewer than 2 instances and a property used
/// fewer than 2 times do not enter the draft, a property of an excluded class stays out, the
/// draft starts as a draft, and the checks of R002.2 decide its admission.
#[test]
fn instance_profile_becomes_a_draft_proposal_with_thresholds() {
    let sampled = profile(serde_json::json!({
        "classes": [
            {"iri": "Person", "instances": 5},
            {"iri": "Org", "instances": 3},
            {"iri": "Tag", "instances": 1},
        ],
        "datatype_properties": [
            {"iri": "name", "class": "Person", "datatype": "xsd:string", "presence": 1.0},
            {"iri": "age", "class": "Person", "datatype": "xsd:integer", "presence": 0.2},
            {"iri": "title", "class": "Org", "datatype": "xsd:string", "presence": 1.0},
        ],
        "object_properties": [
            {"iri": "worksAt", "domain": "Person", "range": "Org", "usage": 4},
            {"iri": "taggedWith", "domain": "Org", "range": "Tag", "usage": 3},
            {"iri": "knows", "domain": "Person", "range": "Person", "usage": 1},
        ],
    }));
    let (draft, observed) = draft_from_profile("p1", &sampled, provenance()).unwrap();
    assert_eq!(draft.state, ProposalState::Draft);
    let classes: Vec<&str> = draft.classes.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(classes, ["Org", "Person"], "Tag has one instance");
    let kept: Vec<&str> = draft.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(kept, ["name", "title", "worksAt"]);
    let person = draft.classes.iter().find(|c| c.name == "Person").unwrap();
    assert_eq!(
        person.properties.iter().map(String::as_str).collect::<Vec<_>>(),
        ["name", "worksAt"]
    );
    assert_eq!(observed, ["age", "knows", "name", "taggedWith", "title", "worksAt"]);
    // Half the observed names fell below a threshold, so the checks leave it a draft.
    let thresholds = ProposalThresholds::default();
    assert_eq!(
        judge_proposal(&draft, &observed, &thresholds).unwrap_err(),
        IngestRefusal::ProposalNotAdmitted(ProposalCheck::Coverage)
    );

    // A profile whose every entry clears the thresholds drafts a proposal the checks admit.
    let clean = profile(serde_json::json!({
        "classes": [{"iri": "Person", "instances": 5}, {"iri": "Org", "instances": 3}],
        "datatype_properties": [
            {"iri": "name", "class": "Person", "datatype": "xsd:string", "presence": 1.0},
        ],
        "object_properties": [
            {"iri": "worksAt", "domain": "Person", "range": "Org", "usage": 4},
        ],
    }));
    let (draft, observed) = draft_from_profile("p2", &clean, provenance()).unwrap();
    assert_eq!(draft.state, ProposalState::Draft);
    assert_eq!(
        judge_proposal(&draft, &observed, &thresholds).unwrap().state,
        ProposalState::Admitted
    );
}

// spec: MEDULLA-INGEST-WORKER-R002.9
/// Verifies MEDULLA-INGEST-WORKER-R002.9: an oversized profile and a property that names an
/// unlisted class refuse with their codes before any draft is built.
#[test]
fn a_profile_over_its_limits_refuses() {
    let classes: Vec<serde_json::Value> = (0..=MAX_PROFILE_ENTRIES)
        .map(|n| serde_json::json!({"iri": format!("C{n}"), "instances": 2}))
        .collect();
    let refusal =
        draft_from_profile("big", &profile(serde_json::json!({"classes": classes})), provenance())
            .unwrap_err();
    assert_eq!(refusal.code(), "PROFILE_TOO_LARGE");
    let dangling = profile(serde_json::json!({
        "classes": [{"iri": "Person", "instances": 5}],
        "object_properties": [{"iri": "worksAt", "domain": "Person", "range": "Org", "usage": 4}],
    }));
    let refusal = draft_from_profile("d", &dangling, provenance()).unwrap_err();
    assert_eq!(refusal.code(), "PROFILE_DANGLING_CLASS");
}
