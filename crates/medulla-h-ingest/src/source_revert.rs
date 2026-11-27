//! `RevertChangeset` (MEDULLA-SCM-R009): the plan that inverts one changeset of a branch chain.
//!
//! The router reads the window of the chain from the changeset to the head and calls
//! [`plan_revert`]. The plan is either the revert already on the chain (so a repeated request
//! appends nothing) or the paths of the inverse changeset, which the router submits through
//! the one accept path of `SubmitChangeset`. Reverting a changeset whose paths a later
//! changeset touched would take that changeset's base away, so it refuses
//! `REVERT_HAS_DEPENDENTS`; the choice between revert and fix-forward is the decision
//! plane's (`QuestionKind::RevertOrFixForward`). Nothing here writes.

use std::collections::BTreeSet;

use medulla_source_types::source_changeset::{
    revert_subject, Changeset, PathOp, SubmitChangesetRequest, SubmittedBlobRef, SubmittedPath,
};

/// The most changesets one revert window holds. A longer window refuses `INVALID_ARGUMENT`.
pub const MAX_REVERT_WINDOW: u64 = 4096;

/// Why a revert has no plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevertRefusal {
    /// The changeset is not on the chain of the branch.
    NotOnChain(String),
    /// Later changesets touch the same paths.
    HasDependents(usize),
    /// A path of the changeset cannot be inverted or its blob is not stored.
    Invalid(String),
}

impl RevertRefusal {
    /// The wire refusal: `CODE: detail`.
    pub fn message(&self) -> String {
        match self {
            Self::NotOnChain(detail) => format!("REVISION_NOT_ON_CHAIN: {detail}"),
            Self::HasDependents(count) => format!(
                "REVERT_HAS_DEPENDENTS: reverting the changeset would remove the base of {count} later changeset(s)"
            ),
            Self::Invalid(detail) => format!("CHANGESET_INVALID: {detail}"),
        }
    }
}

/// What a revert request resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevertPlan {
    /// The revert is already on the chain: the index of its changeset in the window.
    Existing(usize),
    /// The inverse changeset to submit.
    Submit(Box<SubmitChangesetRequest>),
}

fn inverse_path(
    changed: &medulla_source_types::source_changeset::ChangedPath,
    size_of: &dyn Fn(&str) -> Option<u64>,
) -> Result<SubmittedPath, RevertRefusal> {
    let blob = |digest: &Option<String>| -> Result<SubmittedBlobRef, RevertRefusal> {
        let digest = digest.clone().ok_or_else(|| {
            RevertRefusal::Invalid(format!("{} records no blob to restore", changed.path))
        })?;
        let size = size_of(&digest).ok_or_else(|| {
            RevertRefusal::Invalid(format!("the blob of {} is no longer stored", changed.path))
        })?;
        Ok(SubmittedBlobRef { digest, size })
    };
    let (op, blob) = match changed.op {
        // What the changeset created (or renamed into place) is removed.
        PathOp::Add | PathOp::Rename => (PathOp::Delete, None),
        // What it changed is put back.
        PathOp::Modify => (PathOp::Modify, Some(blob(&changed.old_blob)?)),
        // What it removed is restored; a rename's removed source is the same case.
        PathOp::Delete => (PathOp::Add, Some(blob(&changed.old_blob)?)),
    };
    Ok(SubmittedPath {
        path: changed.path.clone(),
        op,
        blob,
        mode: changed.mode,
        renamed_from: None,
    })
}

/// Plan the revert of `target` over `window`: the changesets of the chain from `target` (first)
/// to the head (last), oldest first. `size_of` answers the stored length of a blob digest.
pub fn plan_revert(
    window: &[Changeset],
    target: &str,
    size_of: &dyn Fn(&str) -> Option<u64>,
) -> Result<RevertPlan, RevertRefusal> {
    let Some(first) = window.first().filter(|first| first.id == target) else {
        return Err(RevertRefusal::NotOnChain(format!(
            "{target} is not a changeset of the branch"
        )));
    };
    let subject = revert_subject(target);
    if let Some(index) = window
        .iter()
        .skip(1)
        .position(|changeset| changeset.message.lines().next() == Some(subject.as_str()))
    {
        return Ok(RevertPlan::Existing(index + 1));
    }
    let touched: BTreeSet<&str> = first
        .changed_paths
        .iter()
        .map(|changed| changed.path.as_str())
        .collect();
    let dependents = window
        .iter()
        .skip(1)
        .filter(|later| {
            later
                .changed_paths
                .iter()
                .any(|changed| touched.contains(changed.path.as_str()))
        })
        .count();
    if dependents > 0 {
        return Err(RevertRefusal::HasDependents(dependents));
    }
    let paths = first
        .changed_paths
        .iter()
        .map(|changed| inverse_path(changed, size_of))
        .collect::<Result<Vec<_>, _>>()?;
    let head = window.last().map(|head| head.id.clone());
    let summary = first.message.lines().next().unwrap_or_default();
    Ok(RevertPlan::Submit(Box::new(SubmitChangesetRequest {
        repo: first.repo.clone(),
        branch: String::new(),
        base_head: head,
        message: format!("{subject}\n\nThis reverts changeset {target}: {summary}"),
        paths,
        trailers: None,
        run_trace_id: None,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::source_changeset::{ChangedPath, Trailers};

    fn changeset(
        id: &str,
        message: &str,
        paths: &[(&str, PathOp, Option<&str>, Option<&str>)],
    ) -> Changeset {
        Changeset {
            id: id.into(),
            repo: "repo".into(),
            parents: Vec::new(),
            author_principal: "p".into(),
            message: message.into(),
            changed_paths: paths
                .iter()
                .map(|(path, op, old, new)| ChangedPath {
                    path: (*path).into(),
                    op: *op,
                    old_blob: old.map(str::to_string),
                    new_blob: new.map(str::to_string),
                    mode: 0o644,
                })
                .collect(),
            created_at: 1,
            trailers: Trailers::default(),
            agent_identity: None,
            run_trace_id: None,
            git: None,
            symbol_edit: None,
        }
    }

    fn sizes(digest: &str) -> Option<u64> {
        (digest != "gone").then_some(7)
    }

    // spec: MEDULLA-SCM-R009.1
    #[test]
    fn the_inverse_restores_what_changed_and_removes_what_was_added() {
        let window = [
            changeset(
                "c1",
                "change things",
                &[
                    ("a.rs", PathOp::Add, None, Some("blob-a")),
                    ("b.rs", PathOp::Modify, Some("blob-b0"), Some("blob-b1")),
                    ("c.rs", PathOp::Delete, Some("blob-c"), None),
                ],
            ),
            changeset(
                "c2",
                "unrelated",
                &[("z.rs", PathOp::Add, None, Some("blob-z"))],
            ),
        ];
        let RevertPlan::Submit(request) = plan_revert(&window, "c1", &sizes).unwrap() else {
            panic!("a fresh revert submits");
        };
        let by_path = |path: &str| {
            request
                .paths
                .iter()
                .find(|entry| entry.path == path)
                .unwrap()
        };
        assert_eq!(by_path("a.rs").op, PathOp::Delete);
        assert!(by_path("a.rs").blob.is_none());
        assert_eq!(by_path("b.rs").op, PathOp::Modify);
        assert_eq!(by_path("b.rs").blob.as_ref().unwrap().digest, "blob-b0");
        assert_eq!(by_path("c.rs").op, PathOp::Add);
        assert_eq!(by_path("c.rs").blob.as_ref().unwrap().size, 7);
        // It extends the head the window ends at, and names what it reverts.
        assert_eq!(request.base_head.as_deref(), Some("c2"));
        assert_eq!(request.message.lines().next(), Some("Revert changeset c1"));
    }

    // spec: MEDULLA-SCM-R009.1
    #[test]
    fn a_later_changeset_on_the_same_paths_refuses_and_an_existing_revert_is_found() {
        let target = changeset("c1", "x", &[("a.rs", PathOp::Modify, Some("o"), Some("n"))]);
        let dependent = changeset("c2", "y", &[("a.rs", PathOp::Modify, Some("n"), Some("m"))]);
        let refused = plan_revert(&[target.clone(), dependent], "c1", &sizes).unwrap_err();
        assert_eq!(refused, RevertRefusal::HasDependents(1));
        assert!(refused.message().starts_with("REVERT_HAS_DEPENDENTS: "));
        // Once reverted, the same request answers the revert on the chain: it is idempotent, and
        // the revert itself (which touches the same paths) is not a dependent.
        let revert = changeset(
            "c3",
            "Revert changeset c1\n\nThis reverts changeset c1: x",
            &[("a.rs", PathOp::Modify, Some("n"), Some("o"))],
        );
        assert_eq!(
            plan_revert(&[target, revert], "c1", &sizes).unwrap(),
            RevertPlan::Existing(1)
        );
    }

    // spec: MEDULLA-SCM-R009.1
    #[test]
    fn an_unknown_changeset_or_a_missing_blob_refuses_with_its_code() {
        let target = changeset("c1", "x", &[("a.rs", PathOp::Delete, Some("gone"), None)]);
        let missing = plan_revert(&[target.clone()], "c1", &sizes).unwrap_err();
        assert!(missing.message().starts_with("CHANGESET_INVALID: "));
        let unknown = plan_revert(&[target], "c9", &sizes).unwrap_err();
        assert!(unknown.message().starts_with("REVISION_NOT_ON_CHAIN: "));
        assert!(plan_revert(&[], "c1", &sizes).is_err());
    }
}
