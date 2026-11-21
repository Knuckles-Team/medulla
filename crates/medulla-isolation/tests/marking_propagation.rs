//! Marking propagation along lineage (MEDULLA-TYPED-PACKS-R652.2).

use std::collections::BTreeSet;

use medulla_identity_types::classification::{ClaimKind, ClaimState};
use medulla_identity_types::tenancy::Marking;
use medulla_isolation::isolation::marking_propagation::{legal_move, propagate, LineageEdges};

fn edges(pairs: &[(&str, &str)]) -> LineageEdges {
    let mut edges = LineageEdges::new();
    for (from, to) in pairs {
        edges
            .entry((*from).to_string())
            .or_default()
            .insert((*to).to_string());
    }
    edges
}

fn assets(claims: &[medulla_identity_types::classification::ClassificationClaim]) -> Vec<&str> {
    claims.iter().map(|c| c.asset.as_str()).collect()
}

// spec: MEDULLA-TYPED-PACKS-R652.2
#[test]
fn closure_is_monotone_bounded_and_deterministic() {
    let pci = vec![Marking::new("pci").unwrap()];
    let evidence = vec!["scan:1".to_string()];
    // source -> view -> dashboard, and source -> dashboard (a second path), plus a cycle back.
    let graph = edges(&[
        ("source", "view"),
        ("source", "dashboard"),
        ("view", "dashboard"),
        ("dashboard", "source"),
        ("dashboard", "report"),
    ]);
    let none = BTreeSet::new();
    let claims = propagate(&graph, "source", &pci, &evidence, 64, &none);

    // Each reachable node is proposed once, in breadth-first sorted order; the source never.
    assert_eq!(assets(&claims), ["dashboard", "view", "report"]);
    assert!(claims.iter().all(|c| c.state == ClaimState::Proposed
        && c.provenance.kind == ClaimKind::Propagated
        && c.categories == pci
        && c.evidence == evidence));
    assert!(claims.iter().all(|c| c.validate().is_ok()));

    // Deterministic: the same inputs give the same claims.
    assert_eq!(claims, propagate(&graph, "source", &pci, &evidence, 64, &none));

    // Bounded: one hop reaches only the direct downstream; zero hops reach nothing.
    assert_eq!(
        assets(&propagate(&graph, "source", &pci, &evidence, 1, &none)),
        ["dashboard", "view"]
    );
    assert!(propagate(&graph, "source", &pci, &evidence, 0, &none).is_empty());

    // Monotone: a node that already holds the claim is skipped but the walk goes through it.
    let held: BTreeSet<String> = ["dashboard".to_string()].into();
    assert_eq!(
        assets(&propagate(&graph, "source", &pci, &evidence, 64, &held)),
        ["view", "report"]
    );

    // Only a proposed claim moves, to promoted or rejected.
    assert!(legal_move(ClaimState::Proposed, ClaimState::Promoted));
    assert!(legal_move(ClaimState::Proposed, ClaimState::Rejected));
    assert!(!legal_move(ClaimState::Promoted, ClaimState::Rejected));
    assert!(!legal_move(ClaimState::Rejected, ClaimState::Promoted));
    assert!(!legal_move(ClaimState::Proposed, ClaimState::Proposed));
}
