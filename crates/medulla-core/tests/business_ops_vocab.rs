//! Contract tests for the business operations pack and its shapes companion
//! (MEDULLA-BUSINESS-OPS-R001.1 to R001.8).

use std::collections::BTreeSet;

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedNode, NamedOrBlankNode, Term, Triple};
use medulla_shacl::validate_turtle;

const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const PREFIXES: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n\
                        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                        @prefix ex: <http://example.org/business/> .\n";

const ONTOLOGY: &str = include_str!("../ontology/business_ops-v1.ttl");
const SHAPES: &str = include_str!("../ontology/business_ops-v1.shapes.ttl");

fn triples() -> Vec<Triple> {
    parse_turtle(ONTOLOGY).expect("the business operations ontology parses")
}

fn has_iri(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
    triples.contains(&Triple::new(
        NamedNode::new(subject).unwrap(),
        NamedNode::new(predicate).unwrap(),
        NamedNode::new(object).unwrap(),
    ))
}

fn subject_str(node: &NamedOrBlankNode) -> Option<&str> {
    match node {
        NamedOrBlankNode::NamedNode(iri) => Some(iri.as_str()),
        _ => None,
    }
}

fn object_iri(term: &Term) -> Option<&str> {
    match term {
        Term::NamedNode(node) => Some(node.as_str()),
        _ => None,
    }
}

/// The subjects of one document that are declared `owl:Class`.
fn declared_classes(triples: &[Triple]) -> BTreeSet<String> {
    triples
        .iter()
        .filter(|t| {
            t.predicate.as_str() == format!("{RDF}type")
                && object_iri(&t.object) == Some(&format!("{OWL}Class"))
        })
        .filter_map(|t| subject_str(&t.subject).map(str::to_string))
        .collect()
}

fn assert_class(triples: &[Triple], local: &str) {
    assert!(
        has_iri(
            triples,
            &format!("{KG}{local}"),
            &format!("{RDF}type"),
            &format!("{OWL}Class")
        ),
        "{local} is a declared class"
    );
}

fn assert_parent(triples: &[Triple], local: &str, parent: &str) {
    assert!(
        has_iri(
            triples,
            &format!("{KG}{local}"),
            &format!("{RDFS}subClassOf"),
            &format!("{KG}{parent}")
        ),
        "{local} is a subclass of {parent}"
    );
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

// spec: MEDULLA-BUSINESS-OPS-R001.1
#[test]
fn profile_and_governance_terms_with_shapes() {
    // The module loads through the core catalog under its one canonical id.
    let sources = GraphSchemaSources::default();
    sources.validate().expect("the core catalog validates");
    assert_eq!(
        sources
            .core
            .keys()
            .filter(|id| id.starts_with("core:business_ops@"))
            .count(),
        1,
        "business_ops is registered exactly once"
    );
    let triples = triples();
    for class in [
        "BusinessProfile",
        "Brand",
        "RoleAssignment",
        "ApprovalRule",
        "ClaimPolicy",
        "ApprovedPhrase",
        "ForbiddenPhrase",
    ] {
        assert_class(&triples, class);
    }
    assert_parent(&triples, "BusinessProfile", "LegalEntity");
    assert_parent(&triples, "ClaimPolicy", "Policy");
    assert_parent(&triples, "ApprovalRule", "GovernanceRule");

    // A profile and a rule that carry their required terms conform.
    let good = "ex:p a :BusinessProfile ; rdfs:label \"Acme\" .\n\
                ex:r a :ApprovalRule ; rdfs:label \"Refunds\" ; \
                  :approvalCeilingMinor 50000 ; :approvalCurrency \"USD\" ; \
                  :approvalQuorum 1 ; :approvalActionKind \"refund\" .\n";
    assert_eq!(conforms(good), (true, Vec::new()));

    // A profile with no label fails.
    let (ok, _) = conforms("ex:p a :BusinessProfile .\n");
    assert!(!ok, "a profile without a label is refused");

    // A rule with a ceiling that is not an integer, or no currency, fails.
    let (ok, paths) = conforms(
        "ex:r a :ApprovalRule ; rdfs:label \"Refunds\" ; \
         :approvalCeilingMinor 500.5 ; :approvalCurrency \"USD\" .\n",
    );
    assert!(!ok, "a non-integer ceiling is refused");
    assert!(paths
        .iter()
        .any(|path| path.ends_with("approvalCeilingMinor>")));
    let (ok, paths) =
        conforms("ex:r a :ApprovalRule ; rdfs:label \"Refunds\" ; :approvalCeilingMinor 500 .\n");
    assert!(!ok, "a rule with no currency is refused");
    assert!(paths.iter().any(|path| path.ends_with("approvalCurrency>")));
}

// spec: MEDULLA-BUSINESS-OPS-R001.3
#[test]
fn money_terms_refuse_floats_and_overrefunds() {
    let triples = triples();
    for class in ["OrderLine", "Payment", "Refund", "Payout", "Fee"] {
        assert_class(&triples, class);
    }
    // The core :Order is reused, never redeclared.
    assert!(!declared_classes(&triples).contains(&format!("{KG}Order")));
    assert!(!declared_classes(&triples).contains(&format!("{KG}Customer")));

    let good = "ex:order a :Order .\n\
                ex:line a :OrderLine ; :lineOfOrder ex:order ; :lineQuantity 2 ; \
                  :unitPriceMinor 1999 ; :taxMinor 160 .\n\
                ex:pay a :Payment ; :paymentOfOrder ex:order ; :amountMinor 4158 ; \
                  :amountCurrency \"USD\" .\n\
                ex:ref a :Refund ; :refundOf ex:pay ; :amountMinor 1000 ; \
                  :amountCurrency \"USD\" .\n\
                ex:out a :Payout ; :settlesPayment ex:pay ; :amountMinor 4000 ; \
                  :amountCurrency \"USD\" .\n\
                ex:fee a :Fee ; :feeOfPayout ex:out ; :amountMinor 158 ; \
                  :amountCurrency \"USD\" .\n";
    assert_eq!(conforms(good), (true, Vec::new()));

    // A float literal on an amount property is refused.
    let (ok, paths) =
        conforms("ex:pay a :Payment ; :amountMinor 41.58 ; :amountCurrency \"USD\" .\n");
    assert!(!ok, "a float amount is refused");
    assert!(paths.iter().any(|path| path.ends_with("amountMinor>")));
    let (ok, _) = conforms("ex:line a :OrderLine ; :unitPriceMinor 19.99 .\n");
    assert!(!ok, "a float unit price is refused");

    // An amount with no currency is refused.
    let (ok, _) = conforms("ex:pay a :Payment ; :amountMinor 100 .\n");
    assert!(!ok, "an amount without a currency is refused");

    // A refund with no payment is refused.
    let (ok, paths) = conforms("ex:ref a :Refund ; :amountMinor 100 ; :amountCurrency \"USD\" .\n");
    assert!(!ok, "a refund without a payment is refused");
    assert!(paths.iter().any(|path| path.ends_with("refundOf>")));

    // A refund above its payment is refused; a refund equal to it conforms.
    let over = "ex:pay a :Payment ; :amountMinor 1000 ; :amountCurrency \"USD\" .\n\
                ex:ref a :Refund ; :refundOf ex:pay ; :amountMinor 1001 ; \
                  :amountCurrency \"USD\" .\n";
    let (ok, _) = conforms(over);
    assert!(!ok, "a refund above its payment is refused");
    let equal = over.replace("1001", "1000");
    assert_eq!(conforms(&equal), (true, Vec::new()));
}

// spec: MEDULLA-BUSINESS-OPS-R001.4
#[test]
fn procurement_terms_reuse_commercial_and_supply_chain() {
    let triples = triples();
    for class in [
        "PurchaseOrderLine",
        "GoodsReceipt",
        "StockLevel",
        "ReorderPolicy",
    ] {
        assert_class(&triples, class);
    }
    // Reuse: the classes of the commercial and supply-chain modules are declared there
    // and nowhere in this pack.
    let declared = declared_classes(&triples);
    for reused in ["PurchaseOrder", "PurchaseInvoice", "Supplier"] {
        assert!(
            !declared.contains(&format!("{KG}{reused}")),
            "{reused} is reused, not redeclared"
        );
    }
    let commercial = parse_turtle(include_str!("../ontology/commercial-v1.ttl")).unwrap();
    let supply = parse_turtle(include_str!("../ontology/supply_chain-v1.ttl")).unwrap();
    assert_class(&commercial, "PurchaseOrder");
    assert_class(&commercial, "PurchaseInvoice");
    assert_class(&supply, "Supplier");
    // The new terms point at the reused classes.
    for (property, range) in [
        ("purchaseLineOfOrder", "PurchaseOrder"),
        ("receiptOfOrder", "PurchaseOrder"),
        ("reorderSupplier", "Supplier"),
    ] {
        assert!(
            has_iri(
                &triples,
                &format!("{KG}{property}"),
                &format!("{RDFS}range"),
                &format!("{KG}{range}")
            ),
            "{property} ranges over {range}"
        );
    }
    assert!(
        triples.iter().any(|t| {
            subject_str(&t.subject) == Some(&format!("{KG}leadTimeDays"))
                && t.predicate.as_str() == format!("{RDF}type")
        }),
        "leadTimeDays is declared"
    );
}

// spec: MEDULLA-BUSINESS-OPS-R001.5
#[test]
fn obligation_terms_and_instance_state_machine() {
    use medulla_core::obligation_machine::{
        ObligationEvent, ObligationFacts, ObligationInstanceMachine, ObligationStatus,
        OBLIGATION_INSTANCE_MACHINE,
    };
    use medulla_types::transition_refused::{MachineEvent, MachineState, TransitionRefusalReason};

    let triples = triples();
    for class in [
        "Jurisdiction",
        "ComplianceObligation",
        "RecurrenceRule",
        "HolidayCalendar",
        "ObligationInstance",
        "EvidenceItem",
    ] {
        assert_class(&triples, class);
    }
    assert_parent(&triples, "ComplianceObligation", "GovernanceRule");
    assert_parent(&triples, "EvidenceItem", "Evidence");
    // The core :GovernanceRule and :Evidence are reused, not redeclared.
    let declared = declared_classes(&triples);
    assert!(!declared.contains(&format!("{KG}GovernanceRule")));
    assert!(!declared.contains(&format!("{KG}Evidence")));

    // The shape accepts a well-formed instance and refuses a status outside the machine.
    let (ok, _) = conforms(
        "ex:i a :ObligationInstance ; :instanceDueDate \"2027-04-15\"^^xsd:date ; \
         :obligationStatus \"open\" .\n",
    );
    assert!(ok);
    let (ok, paths) = conforms(
        "ex:i a :ObligationInstance ; :instanceDueDate \"2027-04-15\"^^xsd:date ; \
         :obligationStatus \"pending\" .\n",
    );
    assert!(!ok, "a status outside the machine is refused");
    assert!(paths.iter().any(|path| path.ends_with("obligationStatus>")));

    // The transition function: the edge table is closed and guarded.
    let all = ObligationFacts {
        evidence_recorded: true,
        waiver_recorded: true,
        due_date_passed: true,
    };
    use ObligationEvent::{MarkOverdue, Satisfy, Waive};
    use ObligationStatus::{Open, Overdue, Satisfied, Waived};
    assert_eq!(
        ObligationInstanceMachine::apply(Open, Satisfy, all),
        Ok(Satisfied)
    );
    assert_eq!(
        ObligationInstanceMachine::apply(Open, Waive, all),
        Ok(Waived)
    );
    assert_eq!(
        ObligationInstanceMachine::apply(Open, MarkOverdue, all),
        Ok(Overdue)
    );
    assert_eq!(
        ObligationInstanceMachine::apply(Overdue, Satisfy, all),
        Ok(Satisfied)
    );
    assert_eq!(
        ObligationInstanceMachine::apply(Overdue, Waive, all),
        Ok(Waived)
    );
    // A guard that does not hold refuses.
    for (from, event) in [
        (Open, Satisfy),
        (Open, Waive),
        (Open, MarkOverdue),
        (Overdue, Satisfy),
    ] {
        let refused =
            ObligationInstanceMachine::apply(from, event, ObligationFacts::default()).unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
    }
    // Overdue cannot go back to open, and the terminals accept no event.
    let refused = ObligationInstanceMachine::apply(Overdue, MarkOverdue, all).unwrap_err();
    assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
    for terminal in [Satisfied, Waived] {
        for event in [Satisfy, Waive, MarkOverdue] {
            let refused = ObligationInstanceMachine::apply(terminal, event, all).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
        }
    }

    // The machine is registered in state-machines-v1.ttl with the same edges.
    let machines = parse_turtle(include_str!("../ontology/state-machines-v1.ttl")).unwrap();
    let machine = &OBLIGATION_INSTANCE_MACHINE;
    let edge_id = format!("{KG}edgeId");
    for edge in machine.edges {
        let transition = format!("{KG}{}_{}", machine.name, edge.id);
        let literal_of = |predicate: &str| {
            machines
                .iter()
                .find(|t| {
                    subject_str(&t.subject) == Some(&transition)
                        && t.predicate.as_str() == format!("{KG}{predicate}")
                })
                .and_then(|t| object_iri(&t.object).map(str::to_string))
        };
        assert_eq!(
            literal_of("fromState"),
            Some(format!("{KG}{}_{:?}", machine.name, edge.from)),
            "{} from",
            edge.id
        );
        assert_eq!(
            literal_of("toState"),
            Some(format!("{KG}{}_{:?}", machine.name, edge.to)),
            "{} to",
            edge.id
        );
        assert_eq!(
            literal_of("firedBy"),
            Some(format!("{KG}{}_Event_{}", machine.name, edge.event.name())),
            "{} event",
            edge.id
        );
        assert!(machines.iter().any(|t| {
            subject_str(&t.subject) == Some(&transition) && t.predicate.as_str() == edge_id
        }));
    }
    let registered_edges = machines
        .iter()
        .filter(|t| {
            t.predicate.as_str() == format!("{RDF}type")
                && object_iri(&t.object) == Some(&format!("{KG}ObligationInstanceTransition"))
        })
        .count();
    assert_eq!(registered_edges, machine.edges.len());
    for state in machine.states {
        let subject = format!("{KG}{}_{:?}", machine.name, state);
        let stored = machines.iter().find_map(|t| {
            (subject_str(&t.subject) == Some(&subject)
                && t.predicate.as_str() == format!("{KG}storedForm"))
            .then(|| match &t.object {
                Term::Literal(value) => value.value().to_string(),
                _ => String::new(),
            })
        });
        assert_eq!(stored.as_deref(), Some(state.stored()));
    }
}

// spec: MEDULLA-BUSINESS-OPS-R001.6
#[test]
fn tax_terms_store_only_hashed_identifiers() {
    let triples = triples();
    for class in ["TaxRegistration", "TaxRate", "TaxReturn"] {
        assert_class(&triples, class);
    }
    // A tax return is an obligation instance.
    assert_parent(&triples, "TaxReturn", "ObligationInstance");

    let hash = "a".repeat(64);
    let good = format!(
        "ex:reg a :TaxRegistration ; :taxIdentifierHash \"{hash}\" ; :taxScheme \"sales_tax\" .\n\
         ex:rate a :TaxRate ; :rateBasisPoints 725 ; :taxCategory \"goods\" .\n\
         ex:ret a :TaxReturn ; :returnRegistration ex:reg ; :returnPeriod \"2027-Q1\" ; \
           :instanceDueDate \"2027-04-30\"^^xsd:date ; :obligationStatus \"open\" ; \
           :returnTaxableMinor 1000000 ; :returnTaxDueMinor 72500 .\n"
    );
    assert_eq!(conforms(&good), (true, Vec::new()));

    // A raw identifier property is refused, with or without a hash beside it.
    let (ok, paths) = conforms(&format!(
        "ex:reg a :TaxRegistration ; :taxIdentifierHash \"{hash}\" ; \
         :taxIdentifier \"12-3456789\" .\n"
    ));
    assert!(!ok, "a raw tax identifier is refused");
    assert!(paths.iter().any(|path| path.ends_with("taxIdentifier>")));
    let (ok, _) = conforms("ex:reg a :TaxRegistration ; :taxIdentifier \"12-3456789\" .\n");
    assert!(!ok);

    // A registration whose hash is not 64 lowercase hex characters is refused.
    let (ok, _) = conforms("ex:reg a :TaxRegistration ; :taxIdentifierHash \"123456789\" .\n");
    assert!(!ok, "a short hash is refused");

    // A fractional rate or a float amount is refused.
    let (ok, _) = conforms("ex:rate a :TaxRate ; :rateBasisPoints 7.25 .\n");
    assert!(!ok, "a fractional basis-point rate is refused");
    let (ok, _) = conforms("ex:ret a :TaxReturn ; :returnTaxDueMinor 725.5 .\n");
    assert!(!ok, "a float tax due is refused");
}

// spec: MEDULLA-BUSINESS-OPS-R001.7
#[test]
fn approval_gate_deploy_incident_terms_with_shapes() {
    use medulla_core::approval_machine::{
        ApprovalEvent, ApprovalFacts, ApprovalRequestMachine, ApprovalStatus,
        APPROVAL_REQUEST_MACHINE,
    };
    use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

    let triples = triples();
    for class in ["ApprovalRequest", "GateDecision", "DeployRecord"] {
        assert_class(&triples, class);
    }
    assert_parent(&triples, "GateDecision", "Decision");
    // The core :Incident is reused, never redeclared; its new terms are declared here.
    assert!(!declared_classes(&triples).contains(&format!("{KG}Incident")));
    for property in ["incidentSeverity", "resolvedBy"] {
        assert!(
            triples.iter().any(|t| {
                subject_str(&t.subject) == Some(&format!("{KG}{property}"))
                    && t.predicate.as_str() == format!("{RDF}type")
            }),
            "{property} is declared"
        );
        assert!(
            has_iri(
                &triples,
                &format!("{KG}{property}"),
                &format!("{RDFS}domain"),
                &format!("{KG}Incident")
            ),
            "{property} applies to :Incident"
        );
    }

    let good = "ex:who a :Person .\n\
                ex:req a :ApprovalRequest ; :requestStatus \"pending\" ; \
                  :requestActionKind \"refund\" ; :requestAmountMinor 25000 .\n\
                ex:gate a :GateDecision ; :decisionRequest ex:req ; :gateApprover ex:who ; \
                  :gateOutcome \"approve\" .\n\
                ex:dep a :DeployRecord ; :deployDecision ex:gate ; :deployOutcome \"succeeded\" .\n\
                ex:inc a :Incident ; :incidentSeverity \"high\" .\n";
    assert_eq!(conforms(good), (true, Vec::new()));

    // A gate decision with no approver or no request is refused.
    let (ok, paths) = conforms(
        "ex:req a :ApprovalRequest ; :requestStatus \"pending\" .\n\
         ex:gate a :GateDecision ; :decisionRequest ex:req ; :gateOutcome \"approve\" .\n",
    );
    assert!(!ok, "a gate decision without an approver is refused");
    assert!(paths.iter().any(|path| path.ends_with("gateApprover>")));
    let (ok, paths) = conforms(
        "ex:who a :Person .\n\
         ex:gate a :GateDecision ; :gateApprover ex:who ; :gateOutcome \"approve\" .\n",
    );
    assert!(!ok, "a gate decision without a request is refused");
    assert!(paths.iter().any(|path| path.ends_with("decisionRequest>")));
    // An outcome outside approve, reject, defer is refused.
    let (ok, _) = conforms(
        "ex:who a :Person .\n\
         ex:req a :ApprovalRequest ; :requestStatus \"pending\" .\n\
         ex:gate a :GateDecision ; :decisionRequest ex:req ; :gateApprover ex:who ; \
           :gateOutcome \"maybe\" .\n",
    );
    assert!(!ok);
    // A request status outside the machine is refused.
    let (ok, _) = conforms("ex:req a :ApprovalRequest ; :requestStatus \"stale\" .\n");
    assert!(!ok);

    // The request machine: the edge table is closed and guarded.
    use ApprovalEvent::{Approve, Defer, Reject};
    use ApprovalStatus::{Approved, Deferred, Pending, Rejected};
    let all = ApprovalFacts {
        approver_holds_role: true,
        quorum_met: true,
    };
    assert_eq!(
        ApprovalRequestMachine::apply(Pending, Approve, all),
        Ok(Approved)
    );
    assert_eq!(
        ApprovalRequestMachine::apply(Pending, Reject, all),
        Ok(Rejected)
    );
    assert_eq!(
        ApprovalRequestMachine::apply(Pending, Defer, all),
        Ok(Deferred)
    );
    assert_eq!(
        ApprovalRequestMachine::apply(Deferred, Approve, all),
        Ok(Approved)
    );
    assert_eq!(
        ApprovalRequestMachine::apply(Deferred, Reject, all),
        Ok(Rejected)
    );
    let none = ApprovalFacts::default();
    for (from, event) in [
        (Pending, Approve),
        (Pending, Reject),
        (Pending, Defer),
        (Deferred, Approve),
    ] {
        let refused = ApprovalRequestMachine::apply(from, event, none).unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
    }
    let refused = ApprovalRequestMachine::apply(Deferred, Defer, all).unwrap_err();
    assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
    for terminal in [Approved, Rejected] {
        for event in [Approve, Reject, Defer] {
            let refused = ApprovalRequestMachine::apply(terminal, event, all).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
        }
    }
    // The machine is registered in state-machines-v1.ttl with one transition per edge.
    let machines = parse_turtle(include_str!("../ontology/state-machines-v1.ttl")).unwrap();
    let registered = machines
        .iter()
        .filter(|t| {
            t.predicate.as_str() == format!("{RDF}type")
                && object_iri(&t.object) == Some(&format!("{KG}ApprovalRequestTransition"))
        })
        .count();
    assert_eq!(registered, APPROVAL_REQUEST_MACHINE.edges.len());
    for state in APPROVAL_REQUEST_MACHINE.states {
        let subject = format!("{KG}{}_{:?}", APPROVAL_REQUEST_MACHINE.name, state);
        assert!(machines.iter().any(|t| {
            subject_str(&t.subject) == Some(&subject)
                && t.predicate.as_str() == format!("{KG}storedForm")
                && matches!(&t.object, Term::Literal(v) if v.value() == state.stored())
        }));
    }
}

// spec: MEDULLA-BUSINESS-OPS-R001.8
#[test]
fn every_class_rolls_up_and_no_product_names() {
    let business = triples();
    let ontology_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ontology");
    let foundation = parse_turtle(include_str!("../ontology/core-foundation-v1.ttl")).unwrap();
    let commercial = parse_turtle(include_str!("../ontology/commercial-v1.ttl")).unwrap();
    let supply = parse_turtle(include_str!("../ontology/supply_chain-v1.ttl")).unwrap();
    let core_classes = declared_classes(&foundation);

    // The subclass edges of this pack plus the modules it reuses and core-foundation.
    let mut graph: Vec<Triple> = business.clone();
    graph.extend(foundation);
    graph.extend(commercial);
    graph.extend(supply);
    let rolls_up = |class: &str| -> bool {
        let mut seen = BTreeSet::new();
        let mut stack = vec![class.to_string()];
        while let Some(current) = stack.pop() {
            for parent in graph
                .iter()
                .filter(|t| {
                    subject_str(&t.subject) == Some(current.as_str())
                        && t.predicate.as_str() == format!("{RDFS}subClassOf")
                })
                .filter_map(|t| object_iri(&t.object))
            {
                if core_classes.contains(parent) {
                    return true;
                }
                if seen.insert(parent.to_string()) {
                    stack.push(parent.to_string());
                }
            }
        }
        false
    };
    let classes = declared_classes(&business);
    assert!(classes.len() >= 25, "the pack declares its classes");
    for class in &classes {
        assert!(
            rolls_up(class),
            "{class} has no rdfs:subClassOf path to a core-foundation class"
        );
    }

    // No IRI local name names a product.
    let denylist: Vec<String> = include_str!("fixtures/product_name_denylist.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_lowercase)
        .collect();
    assert!(!denylist.is_empty());
    for triple in &business {
        if let Some(subject) = subject_str(&triple.subject) {
            if let Some(local) = subject.strip_prefix(KG) {
                let lowered = local.to_lowercase();
                for name in &denylist {
                    assert!(
                        !lowered.contains(name.as_str()),
                        "{local} contains the product name {name}"
                    );
                }
            }
        }
    }

    // One home per class: no other packaged module declares a class of this pack, so the
    // class-collision census stays at its baseline.
    for entry in std::fs::read_dir(&ontology_dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".ttl") || name.ends_with(".shapes.ttl") || name == "business_ops-v1.ttl"
        {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap();
        let Ok(other) = parse_turtle(&text) else {
            continue;
        };
        for class in declared_classes(&other) {
            assert!(
                !classes.contains(&class),
                "{class} is declared by business_ops and by {name}"
            );
        }
    }
}
