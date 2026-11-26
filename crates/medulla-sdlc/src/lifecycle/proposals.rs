//! Proposals follow the project mode (MEDULLA-SDLC-R017.5).
//!
//! A `reference` codebase takes no proposal. A `development` codebase takes the
//! proposals a person requested. An `evolving` codebase takes those and the proposals the
//! loop raised unprompted. The gate reads the mode and never writes it: the loop cannot
//! change the mode through a proposal.

use medulla_source_types::lifecycle::{LifecycleRefusal, ProjectMode, ProposalOrigin};

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
        return Err(LifecycleRefusal::ModeReferenceNoProposals(project.to_owned()));
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

/// How many failed results of one task raise a split proposal (MEDULLA-SDLC-R015.5). The task
/// is one the loop cannot finish in one piece when it fails this often.
pub const SPLIT_PROPOSAL_FAILURES: u32 = 3;

/// The failures recorded for `task`: one per entry of `failed_tasks`, the task ids of the
/// failed gate verdicts and failed runs, in any order.
pub fn failures_of_task(task: &str, failed_tasks: &[String]) -> u32 {
    u32::try_from(failed_tasks.iter().filter(|failed| failed.as_str() == task).count())
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
