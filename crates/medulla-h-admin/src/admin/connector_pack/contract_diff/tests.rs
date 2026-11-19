use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::connector_pack::contract_version::{
    ContractBreak, ContractBreakKind, PackContractVersion,
};
use medulla_core::stewardship::TargetScopes;
use medulla_catalog_types::stewardship::{
    ChangeRequestState, StewardRole, StewardshipAssignment, StewardshipScope,
};

use super::*;

const ONTOLOGY: &str = r#"
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <http://example.org/pack#> .
ex:Order a owl:Class .
ex:Customer a owl:Class .
ex:total a owl:DatatypeProperty ; rdfs:range <http://www.w3.org/2001/XMLSchema#decimal> .
ex:placedBy a owl:ObjectProperty ; rdfs:range ex:Customer .
"#;

const SHAPES: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/pack#> .
ex:OrderShape a sh:NodeShape ; sh:targetClass ex:Order ;
  sh:property [ sh:path ex:total ; sh:minCount 0 ; sh:maxCount 5 ] .
"#;

fn digest(byte: &str) -> String {
    format!("sha256:{}", byte.repeat(32))
}

fn contract(version: &str, byte: &str) -> ReleaseContract {
    ReleaseContract {
        version: PackContractVersion {
            connector: "connector-a".into(),
            version: version.into(),
            contract_digest: digest(byte),
            ontology_digest: digest("bb"),
            shapes_digest: digest("cc"),
            supersedes: None,
        },
        ontology_ttl: ONTOLOGY.into(),
        shapes_ttl: Some(SHAPES.into()),
        fields: BTreeMap::from([
            ("tools.search.query".to_string(), "string".to_string()),
            ("tools.search.limit".to_string(), "integer".to_string()),
        ]),
        mappings: BTreeSet::from(["orders".to_string()]),
    }
}

fn steward() -> Vec<StewardshipAssignment> {
    vec![StewardshipAssignment {
        scope: StewardshipScope::Domain("commerce".into()),
        role: StewardRole::Steward,
        principal: "team:commerce".into(),
    }]
}

fn scopes() -> TargetScopes {
    TargetScopes {
        asset: None,
        concept_scheme: None,
        domain: Some("commerce".into()),
    }
}

fn kinds(breaks: &[ContractBreak]) -> Vec<(ContractBreakKind, &str)> {
    breaks.iter().map(|b| (b.kind, b.item.as_str())).collect()
}

// spec: MEDULLA-TYPED-PACKS-R107.2
#[test]
fn non_breaking_applies_breaking_waits_and_queries_use_active_version() {
    let mut history = ContractHistory::default();
    // Release 1: first release, nothing to break.
    let first = history.submit(contract("1.0.0", "01"), &steward(), &scopes());
    assert_eq!(
        first.unwrap(),
        ReleaseDisposition::Applied { supersedes: None }
    );

    // Release 2: an optional field, a new class and a looser shape. It applies
    // at once and writes `supersedes`.
    let mut second = contract("2.0.0", "02");
    second
        .fields
        .insert("tools.search.cursor".into(), "string".into());
    second.ontology_ttl = format!(
        "{ONTOLOGY}\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         <http://example.org/pack#Refund> a owl:Class ."
    );
    second.shapes_ttl = Some(SHAPES.replace("sh:maxCount 5", "sh:maxCount 9"));
    second.mappings.insert("refunds".into());
    assert_eq!(
        history.submit(second, &steward(), &scopes()).unwrap(),
        ReleaseDisposition::Applied {
            supersedes: Some("1.0.0".into())
        }
    );
    let active = history.active().unwrap();
    assert_eq!(active.contract.version.version, "2.0.0");
    assert_eq!(active.contract.version.supersedes.as_deref(), Some("1.0.0"));
    assert_eq!(
        history.by_version("1.0.0").unwrap().status(),
        ReleaseStatus::Superseded
    );

    // Release 3: retypes a field, removes a class, retypes a property, narrows
    // a shape and drops a mapping. It waits behind a change request.
    let mut third = contract("3.0.0", "03");
    third
        .fields
        .insert("tools.search.limit".into(), "string".into());
    third.fields.remove("tools.search.query");
    third.ontology_ttl = ONTOLOGY
        .replace("ex:Customer a owl:Class .", "")
        .replace("decimal", "string");
    third.shapes_ttl = Some(SHAPES.replace("sh:minCount 0", "sh:minCount 1"));
    third.mappings.clear();
    let held = history
        .submit(third.clone(), &steward(), &scopes())
        .unwrap();
    let ReleaseDisposition::Pending {
        breaks,
        total_breaks,
        change_request,
    } = held
    else {
        panic!("a breaking release must wait");
    };
    let pack = "http://example.org/pack#";
    let shape = format!("{pack}OrderShape#{pack}total");
    let (class, property) = (format!("{pack}Customer"), format!("{pack}total"));
    assert_eq!(
        kinds(&breaks),
        vec![
            (ContractBreakKind::RemovedClass, class.as_str()),
            (ContractBreakKind::RetypedProperty, property.as_str()),
            (ContractBreakKind::RemovedField, "tools.search.query"),
            (ContractBreakKind::RetypedField, "tools.search.limit"),
            (ContractBreakKind::NarrowedShape, shape.as_str()),
            (ContractBreakKind::RemovedMapping, "orders"),
        ]
    );
    assert_eq!(total_breaks, 6);
    assert_eq!(change_request.state, ChangeRequestState::Open);
    assert_eq!(change_request.reviewers, vec!["team:commerce".to_string()]);
    assert_eq!(change_request.target, "pack:connector-a");
    assert_eq!(change_request.proposed_digest, digest("03"));

    // Queries still resolve through release 2; release 3 is stored, unserved.
    assert_eq!(history.active().unwrap().contract.version.version, "2.0.0");
    assert_eq!(
        history.by_version("3.0.0").unwrap().status(),
        ReleaseStatus::Pending
    );
    // The removed surface stays readable by revision.
    assert!(history
        .by_version("2.0.0")
        .unwrap()
        .contract
        .fields
        .contains_key("tools.search.query"));

    // Approval activates release 3 and supersedes release 2.
    history.approve("3.0.0").unwrap();
    let active = history.active().unwrap();
    assert_eq!(active.contract.version.version, "3.0.0");
    assert_eq!(active.contract.version.supersedes.as_deref(), Some("2.0.0"));
    assert_eq!(
        history.by_version("2.0.0").unwrap().status(),
        ReleaseStatus::Superseded
    );
}

// spec: MEDULLA-TYPED-PACKS-R107.2
#[test]
fn every_legal_and_illegal_release_transition_is_typed() {
    use medulla_types::transition_refused::{MachineEvent, MachineState};
    use ReleaseEvent::{Activate, Reject, Supersede};
    use ReleaseStatus::{Active, Pending, Rejected, Superseded};
    let legal = [
        (Pending, Activate, Active),
        (Pending, Reject, Rejected),
        (Active, Supersede, Superseded),
    ];
    for (from, event, to) in legal {
        assert_eq!(from.apply(event), Ok(to));
    }
    assert_eq!(PACK_RELEASE_MACHINE.edges.len(), legal.len());
    for from in PACK_RELEASE_MACHINE.states.iter().copied() {
        for event in PACK_RELEASE_MACHINE.events.iter().copied() {
            if legal.iter().any(|(f, e, _)| *f == from && *e == event) {
                continue;
            }
            let error = from.apply(event).unwrap_err();
            assert_eq!(error.machine, "PackReleaseMachine");
            assert_eq!(error.from, from.stored());
            assert_eq!(error.event, event.name());
        }
    }
}

// spec: MEDULLA-TYPED-PACKS-R107.2
#[test]
fn the_ontology_describes_the_same_release_machine() {
    use medulla_types::transition_refused::{MachineEvent, MachineState};
    const MACHINES: &str =
        include_str!("../../../../../medulla-core/ontology/state-machines-v1.ttl");
    let title = |stored: &str| {
        let mut chars = stored.chars();
        chars
            .next()
            .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
            .unwrap_or_default()
    };
    assert!(MACHINES.contains(":PackReleaseMachine a :StateMachine"));
    for state in PACK_RELEASE_MACHINE.states {
        assert!(MACHINES.contains(&format!(":PackReleaseMachine_{} a", title(state.stored()))));
    }
    for edge in PACK_RELEASE_MACHINE.edges {
        let start = MACHINES
            .find(&format!(
                ":PackReleaseMachine_{} a :PackReleaseTransition",
                edge.id
            ))
            .expect("edge declared");
        let text = &MACHINES[start..];
        let text = &text[..text.find(" .\n").unwrap()];
        assert!(text.contains(&format!(
            ":fromState :PackReleaseMachine_{}",
            title(edge.from.stored())
        )));
        assert!(text.contains(&format!(
            ":toState :PackReleaseMachine_{}",
            title(edge.to.stored())
        )));
        assert!(text.contains(&format!(
            ":firedBy :PackReleaseMachine_Event_{}",
            edge.event.name()
        )));
    }
}

// spec: MEDULLA-TYPED-PACKS-R107.2
#[test]
fn breaking_release_without_a_steward_and_rejection_leave_active_unchanged() {
    let mut history = ContractHistory::default();
    history
        .submit(contract("1.0.0", "01"), &steward(), &scopes())
        .unwrap();
    let mut breaking = contract("2.0.0", "02");
    breaking.mappings.clear();
    let error = history
        .submit(breaking.clone(), &[], &scopes())
        .unwrap_err();
    assert!(matches!(error, ContractDiffError::Stewardship(_)));
    assert!(history.by_version("2.0.0").is_none());

    history.submit(breaking, &steward(), &scopes()).unwrap();
    history.reject("2.0.0").unwrap();
    assert_eq!(history.active().unwrap().contract.version.version, "1.0.0");
    assert_eq!(
        history.by_version("2.0.0").unwrap().status(),
        ReleaseStatus::Rejected
    );
    assert!(matches!(
        history.approve("2.0.0"),
        Err(ContractDiffError::Transition(_))
    ));
    assert!(matches!(
        history.submit(contract("2.0.0", "09"), &steward(), &scopes()),
        Err(ContractDiffError::DuplicateVersion(_))
    ));
}
