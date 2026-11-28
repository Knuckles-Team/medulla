//! Findings are cited entries in `research.md` (MEDULLA-SDLC-R016.2, R016.4).
//!
//! A research entry is a proposal of kind `edit_plan_section` whose target starts with
//! `research:` followed by the unknown id. Its statement is the entry text, and each citation is
//! one evidence of kind `research_citation`. The rule: the statement is not blank, at least one
//! evidence is a `research_citation`, and for every such evidence the statement holds a line
//! `Source: <record_id> (read at <digits>)`, the digits being the read time in epoch
//! milliseconds. The entry lands only as a pull request through the gates (R017.4).

use medulla_source_types::lifecycle::{
    sha256_hex, EvidenceKind, LifecycleRefusal, Proposal, ProposalKind,
};
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredNumber,
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption,
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredText,
};

/// The target prefix of a research entry.
pub const RESEARCH_TARGET_PREFIX: &str = "research:";

/// The task the research loop's tool-use contracts name (`sc:forTask`). The loop's operation
/// set is not a list in code: it is the tools of the `sc:ToolUseContract` individuals that name
/// this task, which a connector pack imports into the graph (MEDULLA-SDLC-R016.4). The loop
/// calls no connector directly: it goes through arbora `fleet.call`.
pub const RESEARCH_TASK_IRI: &str = "urn:medulla:task:research-unknown";

/// The unit cost of researching one unknown, in micro-units (`research_next` `cost`).
pub const UNKNOWN_COST_MICRO: i64 = 250_000;

/// The unit gain of researching one unknown, in micro-units (`research_next` `gain`).
pub const UNKNOWN_GAIN_MICRO: i64 = 1_000_000;

/// One tool a research contract names: the connector (the segment after `urn:scion:` of the
/// contract) and the tool's name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResearchTool {
    /// The connector the research contract names.
    pub connector: String,
    /// The name of the tool.
    pub tool: String,
}

/// Check that `tool` is one the research contracts of the graph name. A tool the contracts do
/// not hold refuses `RESEARCH_TOOL_UNKNOWN`; the registry is never consulted for a name the
/// graph does not hold.
pub fn check_research_tool(known: &[ResearchTool], tool: &str) -> Result<(), LifecycleRefusal> {
    if known.iter().any(|held| held.tool == tool) {
        Ok(())
    } else {
        Err(LifecycleRefusal::ResearchToolUnknown(tool.to_owned()))
    }
}

/// The frontier option of one unknown for `Decide` `ResearchNext` (R016.1): the option id is the
/// unknown id; `cost`, `depth`, `gain` and `p_useful` are Q32 numbers (sorted by key) and the
/// text `subtask` is the unknown id. The text of the unknown is not part of the option: it is
/// what [`research_item_key`] hashes.
pub fn unknown_frontier(unknown_id: &str, text: &str) -> DeclaredOption {
    let _ = text;
    let q32 = |micro: i64| micro << 32;
    let numbers = vec![
        DeclaredNumber {
            key: "cost".to_owned(),
            q32: q32(UNKNOWN_COST_MICRO),
        },
        DeclaredNumber {
            key: "depth".to_owned(),
            q32: 0,
        },
        DeclaredNumber {
            key: "gain".to_owned(),
            q32: q32(UNKNOWN_GAIN_MICRO),
        },
        DeclaredNumber {
            key: "p_useful".to_owned(),
            q32: 1 << 32,
        },
    ];
    DeclaredOption {
        option_id: unknown_id.to_owned(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::new(numbers).unwrap_or_default(),
        texts: BoundedVec::new(vec![DeclaredText {
            key: "subtask".to_owned(),
            text: unknown_id.to_owned(),
        }])
        .unwrap_or_default(),
    }
}

/// The idempotency key of the `research_web` work item of one unknown. A re-ingest of the same
/// unknown text submits the same key, so nothing new is submitted.
pub fn research_item_key(unknown_id: &str, text: &str) -> String {
    format!(
        "research-unknown:{unknown_id}:{}",
        sha256_hex(text.as_bytes())
    )
}

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
    fn the_loop_calls_only_tools_its_contracts_name() {
        let known = vec![
            ResearchTool {
                connector: "searxng".into(),
                tool: "web_search".into(),
            },
            ResearchTool {
                connector: "scholarx".into(),
                tool: "search_papers".into(),
            },
        ];
        assert_eq!(check_research_tool(&known, "web_search"), Ok(()));
        let refused = check_research_tool(&known, "list_snapshots").unwrap_err();
        assert_eq!(refused.code(), "RESEARCH_TOOL_UNKNOWN");
        assert_eq!(
            check_research_tool(&[], "web_search").unwrap_err().code(),
            "RESEARCH_TOOL_UNKNOWN"
        );
    }

    // spec: MEDULLA-SDLC-R016.1
    #[test]
    fn an_unknown_is_one_frontier_option_and_one_stable_item_key() {
        let option = unknown_frontier("planunknown:demo:plan.md#L4", "Which parser?");
        assert_eq!(option.option_id, "planunknown:demo:plan.md#L4");
        let numbers: Vec<(&str, i64)> = option
            .numbers
            .iter()
            .map(|n| (n.key.as_str(), n.q32))
            .collect();
        assert_eq!(
            numbers,
            [
                ("cost", 250_000 << 32),
                ("depth", 0),
                ("gain", 1_000_000 << 32),
                ("p_useful", 1 << 32)
            ]
        );
        let texts: Vec<(&str, &str)> = option
            .texts
            .iter()
            .map(|t| (t.key.as_str(), t.text.as_str()))
            .collect();
        assert_eq!(texts, [("subtask", "planunknown:demo:plan.md#L4")]);
        let key = research_item_key("planunknown:demo:plan.md#L4", "Which parser?");
        assert_eq!(
            key,
            research_item_key("planunknown:demo:plan.md#L4", "Which parser?")
        );
        assert_ne!(
            key,
            research_item_key("planunknown:demo:plan.md#L4", "Which lexer?")
        );
        assert!(key.starts_with("research-unknown:planunknown:demo:plan.md#L4:"));
    }
}
