//! MEDULLA-SDLC-R019.12: a person's proposal that carries the work supersedes the delivery.

use std::collections::{BTreeMap, BTreeSet};

use medulla_sdlc::lifecycle::delivery::superseded::{
    superseded_verdict, BranchChangeset, CandidateProposal, SupersededVerdict,
};

fn changeset(id: &str, paths: &[(&str, &str)]) -> BranchChangeset {
    BranchChangeset {
        id: id.into(),
        path_digests: paths
            .iter()
            .map(|(p, d)| (p.to_string(), d.to_string()))
            .collect(),
    }
}

fn candidate(by_person: bool, ids: &[&str], paths: &[(&str, &str)]) -> CandidateProposal {
    CandidateProposal {
        proposal_id: "pr-9".into(),
        by_person,
        changeset_ids: ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        path_digests: paths
            .iter()
            .map(|(p, d)| (p.to_string(), d.to_string()))
            .collect::<BTreeMap<_, _>>(),
    }
}

// spec: MEDULLA-SDLC-R019.12
#[test]
fn containing_person_proposal_supersedes_and_related_does_not() {
    let branch = vec![
        changeset("c1", &[("a.rs", "d-a")]),
        changeset("c2", &[("b.rs", "d-b")]),
    ];
    // Equal changeset ids.
    assert_eq!(
        superseded_verdict(&branch, &candidate(true, &["c1", "c2", "c3"], &[])),
        SupersededVerdict::CloseSuperseded
    );
    // Equal per-path content digests (a squash carries other ids).
    assert_eq!(
        superseded_verdict(
            &branch,
            &candidate(true, &[], &[("a.rs", "d-a"), ("b.rs", "d-b"), ("z.rs", "d-z")])
        ),
        SupersededVerdict::CloseSuperseded
    );
    // Mixed: one by id, one by digest.
    assert_eq!(
        superseded_verdict(&branch, &candidate(true, &["c1"], &[("b.rs", "d-b")])),
        SupersededVerdict::CloseSuperseded
    );
    // Related only: one changeset is missing or its content differs.
    assert_eq!(
        superseded_verdict(&branch, &candidate(true, &["c1"], &[])),
        SupersededVerdict::Keep
    );
    assert_eq!(
        superseded_verdict(&branch, &candidate(true, &["c1"], &[("b.rs", "other")])),
        SupersededVerdict::Keep
    );
    // A bot's proposal never supersedes, and an empty branch has nothing to carry.
    assert_eq!(
        superseded_verdict(&branch, &candidate(false, &["c1", "c2"], &[])),
        SupersededVerdict::Keep
    );
    assert_eq!(
        superseded_verdict(&[], &candidate(true, &["c1"], &[])),
        SupersededVerdict::Keep
    );
}
