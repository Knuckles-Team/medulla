//! Proposals follow the project mode (MEDULLA-SDLC-R017.5).
//!
//! A `reference` codebase takes no proposal. A `development` codebase takes the
//! proposals a person requested. An `evolving` codebase takes those and the proposals the
//! loop raised unprompted. The gate reads the mode and never writes it: the loop cannot
//! change the mode through a proposal.

use medulla_source_types::lifecycle::{
    is_spec_file_path, LifecycleRefusal, ProjectMode, ProposalOrigin, ProposalState,
};

/// Whether a codebase in `mode` takes a proposal of `origin`.
pub fn mode_admits(mode: ProjectMode, origin: ProposalOrigin) -> bool {
    match mode {
        ProjectMode::Reference => false,
        ProjectMode::Development => origin == ProposalOrigin::Requested,
        ProjectMode::Evolving => true,
    }
}

/// Admit or refuse one proposal of `project`: a `reference` codebase refuses
/// `MODE_REFERENCE_NO_PROPOSALS`, and a proposal whose origin the mode does not admit
/// refuses `MODE_ORIGIN_NOT_ADMITTED`.
pub fn admit_proposal(
    mode: ProjectMode,
    origin: ProposalOrigin,
    project: &str,
) -> Result<(), LifecycleRefusal> {
    if mode == ProjectMode::Reference {
        return Err(LifecycleRefusal::ModeReferenceNoProposals(
            project.to_owned(),
        ));
    }
    if !mode_admits(mode, origin) {
        return Err(LifecycleRefusal::ModeOriginNotAdmitted(format!(
            "{project} is in mode {} and takes no {} proposal",
            mode.name(),
            origin.name()
        )));
    }
    Ok(())
}

/// A stored proposal as the changeset guard reads it (MEDULLA-SDLC-R017.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredBinding<'a> {
    /// The id of the stored proposal.
    pub id: &'a str,
    /// The project the proposal belongs to.
    pub project: &'a str,
    /// The lifecycle state of the proposal.
    pub state: ProposalState,
}

/// Whether any of `paths` is a file of the spec tree (a spec, plan or task file).
pub fn touches_spec_tree<'a>(paths: impl IntoIterator<Item = &'a str>) -> bool {
    paths.into_iter().any(is_spec_file_path)
}

/// The one rule of MEDULLA-SDLC-R017.4. A changeset the loop authored (`loop_authored`, read from
/// the verified principal, never from the request) that edits the spec tree lands only as the
/// delivery of an approved proposal of the same project, named by a `Proposal:` trailer
/// (`named`). Anything else is a direct write and refuses `PROPOSAL_DIRECT_WRITE_REFUSED`. A
/// person's changeset, and a loop changeset that touches no spec file, pass.
pub fn check_changeset_provenance(
    loop_authored: bool,
    repo: &str,
    touched: &[&str],
    named: &[String],
    stored: &[StoredBinding<'_>],
) -> Result<(), LifecycleRefusal> {
    if !loop_authored || !touches_spec_tree(touched.iter().copied()) {
        return Ok(());
    }
    let refuse = |detail: String| Err(LifecycleRefusal::ProposalDirectWriteRefused(detail));
    if named.is_empty() {
        return refuse(format!(
            "{repo}: a loop-authored spec edit names the approved proposal it delivers in a `Proposal:` trailer"
        ));
    }
    for id in named {
        let Some(held) = stored
            .iter()
            .find(|held| held.id == id && held.project == repo)
        else {
            return refuse(format!("{repo}: {id} is no proposal of this project"));
        };
        if held.state != ProposalState::Approved {
            return refuse(format!(
                "{repo}: {id} is {} and only an approved proposal lands",
                held.state.name()
            ));
        }
    }
    Ok(())
}

/// How many failed results of one task raise a split proposal (MEDULLA-SDLC-R015.5). The task
/// is one the loop cannot finish in one piece when it fails this often.
pub const SPLIT_PROPOSAL_FAILURES: u32 = 3;

/// The failures recorded for `task`: one per entry of `failed_tasks`, the task ids of the
/// failed gate verdicts and failed runs, in any order.
pub fn failures_of_task(task: &str, failed_tasks: &[String]) -> u32 {
    u32::try_from(
        failed_tasks
            .iter()
            .filter(|failed| failed.as_str() == task)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

/// Whether `failures` of one task raise a split proposal (R017.2 kind `row_split` or
/// `task_split`): the count has reached [`SPLIT_PROPOSAL_FAILURES`]. A pure counter, so equal
/// failure records give an equal answer.
pub fn split_proposal_due(failures: u32) -> bool {
    failures >= SPLIT_PROPOSAL_FAILURES
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-SDLC-R017.5
    #[test]
    fn the_mode_decides_which_origins_are_admitted() {
        use ProjectMode::*;
        use ProposalOrigin::*;
        assert!(!mode_admits(Reference, Requested));
        assert!(!mode_admits(Reference, Unprompted));
        assert!(mode_admits(Development, Requested));
        assert!(!mode_admits(Development, Unprompted));
        assert!(mode_admits(Evolving, Requested));
        assert!(mode_admits(Evolving, Unprompted));
        let refusal = admit_proposal(Reference, Requested, "demo").unwrap_err();
        assert_eq!(refusal.code(), "MODE_REFERENCE_NO_PROPOSALS");
        let refusal = admit_proposal(Development, Unprompted, "demo").unwrap_err();
        assert_eq!(refusal.code(), "MODE_ORIGIN_NOT_ADMITTED");
        assert!(admit_proposal(Evolving, Unprompted, "demo").is_ok());
    }

    // spec: MEDULLA-SDLC-R017.4
    #[test]
    fn a_loop_spec_edit_needs_an_approved_proposal_of_its_project() {
        let approved = StoredBinding {
            id: "proposal:a",
            project: "demo",
            state: ProposalState::Approved,
        };
        let open = StoredBinding {
            id: "proposal:b",
            project: "demo",
            state: ProposalState::Open,
        };
        let other = StoredBinding {
            id: "proposal:c",
            project: "other",
            state: ProposalState::Approved,
        };
        let stored = [approved, open, other];
        let spec = ["specs/area/requirements.md"];
        let named = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        let code = |result: Result<(), LifecycleRefusal>| result.unwrap_err().code();
        // A person's edit and a loop edit outside the spec tree pass without a proposal.
        assert!(check_changeset_provenance(false, "demo", &spec, &[], &stored).is_ok());
        assert!(check_changeset_provenance(true, "demo", &["src/lib.rs"], &[], &stored).is_ok());
        // A loop edit of the spec tree needs a named, approved proposal of the same project.
        let refused = "PROPOSAL_DIRECT_WRITE_REFUSED";
        assert_eq!(
            code(check_changeset_provenance(
                true,
                "demo",
                &spec,
                &[],
                &stored
            )),
            refused
        );
        assert_eq!(
            code(check_changeset_provenance(
                true,
                "demo",
                &spec,
                &named(&["proposal:b"]),
                &stored
            )),
            refused
        );
        assert_eq!(
            code(check_changeset_provenance(
                true,
                "demo",
                &spec,
                &named(&["proposal:c"]),
                &stored
            )),
            refused
        );
        assert_eq!(
            code(check_changeset_provenance(
                true,
                "demo",
                &spec,
                &named(&["proposal:absent"]),
                &stored
            )),
            refused
        );
        assert!(
            check_changeset_provenance(true, "demo", &spec, &named(&["proposal:a"]), &stored)
                .is_ok()
        );
    }

    // spec: MEDULLA-SDLC-R017.4
    #[test]
    fn proposal_trailers_read_the_last_paragraph() {
        use medulla_source_types::lifecycle::proposal_trailers;
        let message = "feat: edit\n\nProposal: proposal:a, proposal:b\nSpec: MEDULLA-X-R001";
        assert_eq!(proposal_trailers(message), ["proposal:a", "proposal:b"]);
        assert!(proposal_trailers("feat: edit\n\nProposal: p\n\nbody only").is_empty());
        assert!(is_spec_file_path("specs/a/spec.md"));
        assert!(!is_spec_file_path("src/specs/a.md"));
    }

    // spec: MEDULLA-SDLC-R015.5
    #[test]
    fn repeated_failures_of_one_task_raise_a_split_proposal() {
        let failed: Vec<String> = ["t1", "t2", "t1", "t1", "t2"]
            .iter()
            .map(|id| id.to_string())
            .collect();
        assert_eq!(failures_of_task("t1", &failed), 3);
        assert_eq!(failures_of_task("t2", &failed), 2);
        assert_eq!(failures_of_task("none", &failed), 0);
        assert!(split_proposal_due(failures_of_task("t1", &failed)));
        assert!(!split_proposal_due(failures_of_task("t2", &failed)));
    }
}
