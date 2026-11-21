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
}
