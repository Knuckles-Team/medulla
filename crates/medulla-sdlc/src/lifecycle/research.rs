//! Findings are cited entries in `research.md` (MEDULLA-SDLC-R016.2, R016.4).
//!
//! A research entry is a proposal of kind `edit_plan_section` whose target starts with
//! `research:` followed by the unknown id. Its statement is the entry text, and each citation is
//! one evidence of kind `research_citation`. The rule: the statement is not blank, at least one
//! evidence is a `research_citation`, and for every such evidence the statement holds a line
//! `Source: <record_id> (read at <digits>)`, the digits being the read time in epoch
//! milliseconds. The entry lands only as a pull request through the gates (R017.4).

use medulla_source_types::lifecycle::{
    EvidenceKind, LifecycleRefusal, Proposal, ProposalKind,
};

/// The target prefix of a research entry.
pub const RESEARCH_TARGET_PREFIX: &str = "research:";

/// The loop's operation set, one tool name per connector kind: web search, scholarly search and
/// the archived-web read. Each is the name the arbora registry lists; the L3 test reads the
/// registry through `find` and fails on any name it does not hold. The loop calls no connector
/// directly: it goes through arbora `fleet.call`.
pub const RESEARCH_TOOLS: &[&str] = &["web_search", "search_papers", "list_snapshots"];

/// Whether `proposal` is a research entry: an `edit_plan_section` proposal that targets
/// `research:<unknown id>`.
pub fn is_research_entry(proposal: &Proposal) -> bool {
    proposal.kind == ProposalKind::EditPlanSection
        && proposal.target.starts_with(RESEARCH_TARGET_PREFIX)
}

/// Whether `statement` holds a line `Source: <record_id> (read at <digits>)` for `record_id`.
fn cites(statement: &str, record_id: &str) -> bool {
    let head = format!("Source: {record_id} (read at ");
    statement.lines().any(|line| {
        line.strip_prefix(&head)
            .and_then(|rest| rest.strip_suffix(')'))
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Check one research entry. A blank statement, no `research_citation` evidence, or a citation
/// the statement does not name in a `Source:` line refuses `RESEARCH_ENTRY_UNCITED`.
pub fn check_research_entry(proposal: &Proposal) -> Result<(), LifecycleRefusal> {
    let uncited = |detail: &str| {
        Err(LifecycleRefusal::ResearchEntryUncited(format!(
            "{}: {detail}",
            proposal.target
        )))
    };
    if proposal.statement.trim().is_empty() {
        return uncited("the entry text is blank");
    }
    let citations: Vec<&str> = proposal
        .evidence
        .iter()
        .filter(|evidence| evidence.kind == EvidenceKind::ResearchCitation)
        .map(|evidence| evidence.record_id.as_str())
        .collect();
    if citations.is_empty() {
        return uncited("the entry cites no source");
    }
    for record_id in citations {
        if !cites(&proposal.statement, record_id) {
            return uncited(&format!(
                "the text holds no `Source: {record_id} (read at <ms>)` line"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::lifecycle::{ProposalEvidence, ProposalOrigin};

    fn entry(statement: &str, evidence: Vec<ProposalEvidence>) -> Proposal {
        Proposal {
            project: "demo".into(),
            kind: ProposalKind::EditPlanSection,
            target: "research:planunknown:demo:plan.md#L4".into(),
            statement: statement.into(),
            marker: None,
            evidence,
            origin: ProposalOrigin::Requested,
        }
    }

    fn citation(record: &str) -> ProposalEvidence {
        ProposalEvidence {
            kind: EvidenceKind::ResearchCitation,
            record_id: record.into(),
            digest: "d".repeat(64),
        }
    }

    // spec: MEDULLA-SDLC-R016.2
    #[test]
    fn uncited_entries_refuse() {
        let cited = entry(
            "The finding.\nSource: source:a (read at 1700000000000)",
            vec![citation("source:a")],
        );
        assert!(is_research_entry(&cited));
        assert_eq!(check_research_entry(&cited), Ok(()));
        let no_evidence = entry("The finding.", Vec::new());
        assert_eq!(
            check_research_entry(&no_evidence).unwrap_err().code(),
            "RESEARCH_ENTRY_UNCITED"
        );
        let no_line = entry("The finding.", vec![citation("source:a")]);
        assert_eq!(
            check_research_entry(&no_line).unwrap_err().code(),
            "RESEARCH_ENTRY_UNCITED"
        );
        let bad_time = entry(
            "Source: source:a (read at soon)",
            vec![citation("source:a")],
        );
        assert!(check_research_entry(&bad_time).is_err());
        let second_missing = entry(
            "Source: source:a (read at 1)",
            vec![citation("source:a"), citation("source:b")],
        );
        assert!(check_research_entry(&second_missing).is_err());
        assert!(check_research_entry(&entry("  ", vec![citation("source:a")])).is_err());
    }

    // spec: MEDULLA-SDLC-R016.4
    #[test]
    fn the_loop_names_one_tool_per_connector_kind() {
        assert_eq!(RESEARCH_TOOLS.len(), 3);
        let mut sorted = RESEARCH_TOOLS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);
    }
}
