//! Pure knowledge-curation kernels (MEDULLA-DECISION-ENGINE-R709.2, R709.7).
//!
//! No I/O and no clock. [`check_contradictions`] answers every reason a proposal is refused;
//! [`is_due`] says whether a curation cycle is due; [`loop_metrics`] measures the closed loop
//! over the stored flags. The handler (R709.3) feeds them the graph and the store.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::knowledge_curation::{
    ContradictionFinding, CurationEditOp, CurationProposal, LoopMetrics,
    CONTRADICTS_ACTIVE_DEFINITION, CONTRADICTS_INVARIANT, REFERENCE_UNRESOLVED, UPDATE_BASE_STALE,
};
use medulla_decision_types::knowledge_flag::FlagState;

/// What the graph and the store hold about existing knowledge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnowledgeIndex {
    /// The digest of the adopted (champion) definition of each target.
    pub current: BTreeMap<String, String>,
    /// The digest of the evaluated proposal that is waiting to be adopted, for each target.
    pub pending: BTreeMap<String, String>,
    /// Every source column and term the graph holds.
    pub known_refs: BTreeSet<String>,
}

/// One invariant of the invariant gate (R129): it forbids some edit operations on every target
/// that starts with `target_prefix`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invariant {
    /// The invariant id, named in the finding.
    pub invariant_id: String,
    /// The targets it protects.
    pub target_prefix: String,
    /// The edit operations it forbids on them.
    pub forbidden_ops: Vec<CurationEditOp>,
}

fn finding(
    code: medulla_types::refusal_code::RefusalCode,
    other: Option<String>,
) -> ContradictionFinding {
    ContradictionFinding {
        code: code.as_str().to_string(),
        other_ref: other,
    }
}

/// Every reason `proposal` is refused, in a fixed order: active definition, invariants (by
/// id), unresolved references (in the edit's order), stale base. An empty answer admits it.
pub fn check_contradictions(
    proposal: &CurationProposal,
    index: &KnowledgeIndex,
    invariants: &[Invariant],
) -> Vec<ContradictionFinding> {
    let edit = &proposal.edit;
    let target = &proposal.target_ref;
    let mut findings = Vec::new();

    // A different definition is already current or waiting, and the edit does not supersede it.
    if !edit.supersedes {
        let incoming = edit.after_digest.as_deref();
        let pending = index.pending.get(target);
        let current = index.current.get(target);
        let clash_pending = pending.filter(|digest| Some(digest.as_str()) != incoming);
        let clash_current = (edit.op == CurationEditOp::Create)
            .then_some(current)
            .flatten()
            .filter(|digest| Some(digest.as_str()) != incoming);
        if let Some(other) = clash_pending.or(clash_current) {
            findings.push(finding(
                CONTRADICTS_ACTIVE_DEFINITION,
                Some(format!("{target}#{other}")),
            ));
        }
    }

    let mut gates: Vec<&Invariant> = invariants
        .iter()
        .filter(|invariant| {
            target.starts_with(&invariant.target_prefix)
                && invariant.forbidden_ops.contains(&edit.op)
        })
        .collect();
    gates.sort_by(|a, b| a.invariant_id.cmp(&b.invariant_id));
    for invariant in gates {
        findings.push(finding(
            CONTRADICTS_INVARIANT,
            Some(invariant.invariant_id.clone()),
        ));
    }

    for reference in &edit.references {
        if !index.known_refs.contains(reference) {
            findings.push(finding(REFERENCE_UNRESOLVED, Some(reference.clone())));
        }
    }

    if edit.op != CurationEditOp::Create || edit.before_digest.is_some() {
        let current = index.current.get(target);
        if edit.before_digest.as_ref() != current {
            findings.push(finding(UPDATE_BASE_STALE, current.cloned()));
        }
    }
    findings
}

/// Whether a curation cycle is due: no cycle ran yet, or the cadence has elapsed since the last.
pub fn is_due(last_cycle_ms: Option<u64>, now_ms: u64, cadence_seconds: u64) -> bool {
    last_cycle_ms
        .is_none_or(|last| now_ms >= last.saturating_add(cadence_seconds.saturating_mul(1000)))
}

/// What the measures read from one stored flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagMeasure {
    /// When the flag was first raised.
    pub raised_at_ms: u64,
    /// The flag's state.
    pub state: FlagState,
    /// Whether the flag came back to `Open`.
    pub reopened: bool,
    /// How many times a curation fixed the flag.
    pub curated_times: u64,
    /// When the flag was last resolved.
    pub resolved_at_ms: Option<u64>,
}

/// The closed-loop measures of the flags raised in `[from_ms, to_ms]`.
pub fn loop_metrics(flags: &[FlagMeasure], from_ms: u64, to_ms: u64) -> LoopMetrics {
    let window: Vec<&FlagMeasure> = flags
        .iter()
        .filter(|flag| flag.raised_at_ms >= from_ms && flag.raised_at_ms <= to_ms)
        .collect();
    let curated: Vec<&&FlagMeasure> = window
        .iter()
        .filter(|flag| flag.state == FlagState::Curated)
        .collect();
    let mut spans: Vec<u64> = curated
        .iter()
        .filter_map(|flag| {
            flag.resolved_at_ms
                .map(|resolved| resolved.saturating_sub(flag.raised_at_ms))
        })
        .collect();
    spans.sort_unstable();
    let median = match spans.len() {
        0 => 0,
        n if n % 2 == 1 => spans[n / 2],
        n => {
            (spans[n / 2 - 1] / 2)
                + (spans[n / 2] / 2)
                + ((spans[n / 2 - 1] % 2 + spans[n / 2] % 2) / 2)
        }
    };
    let ever_curated = window.iter().filter(|flag| flag.curated_times > 0).count() as u64;
    let came_back = window
        .iter()
        .filter(|flag| flag.curated_times > 0 && flag.reopened)
        .count() as u64;
    LoopMetrics {
        flags_raised: window.len() as u64,
        flags_curated: curated.len() as u64,
        flags_dismissed: window
            .iter()
            .filter(|flag| flag.state == FlagState::Dismissed)
            .count() as u64,
        median_flag_to_fix_ms: median,
        recurrence_ppm: if ever_curated == 0 {
            0
        } else {
            came_back.saturating_mul(1_000_000) / ever_curated
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::knowledge_curation::CurationEdit;
    use medulla_decision_types::knowledge_flag::KnowledgeKind;

    fn proposal(op: CurationEditOp, before: Option<&str>, after: &str) -> CurationProposal {
        CurationProposal {
            proposal_id: "p1".into(),
            flag_ids: vec!["flag:a".into()],
            target_ref: "source:orders".into(),
            knowledge_kind: KnowledgeKind::DataAccess,
            edit: CurationEdit {
                op,
                before_digest: before.map(str::to_string),
                after_digest: Some(after.into()),
                body_digest: "body".into(),
                supersedes: false,
                references: Vec::new(),
            },
            rationale_trace_ids: Vec::new(),
            evidence_digest: "e".into(),
        }
    }

    fn index() -> KnowledgeIndex {
        KnowledgeIndex {
            current: [("source:orders".to_string(), "v1".to_string())].into(),
            pending: BTreeMap::new(),
            known_refs: ["orders.amount".to_string()].into(),
        }
    }

    fn codes(findings: &[ContradictionFinding]) -> Vec<&str> {
        findings.iter().map(|f| f.code.as_str()).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R709.2
    #[test]
    fn each_contradiction_code_fires_once() {
        // A clean update from the current digest is admitted.
        let clean = proposal(CurationEditOp::Update, Some("v1"), "v2");
        assert!(check_contradictions(&clean, &index(), &[]).is_empty());

        // A stale base fires only UPDATE_BASE_STALE.
        let stale = proposal(CurationEditOp::Update, Some("v0"), "v2");
        assert_eq!(
            codes(&check_contradictions(&stale, &index(), &[])),
            [UPDATE_BASE_STALE.as_str()]
        );

        // A waiting proposal with another definition contradicts, unless the edit supersedes.
        let mut waiting = index();
        waiting.pending.insert("source:orders".into(), "v9".into());
        assert_eq!(
            codes(&check_contradictions(&clean, &waiting, &[])),
            [CONTRADICTS_ACTIVE_DEFINITION.as_str()]
        );
        let mut supersede = clean.clone();
        supersede.edit.supersedes = true;
        assert!(check_contradictions(&supersede, &waiting, &[]).is_empty());
        // A create over a current definition contradicts too.
        let create = proposal(CurationEditOp::Create, None, "v2");
        assert_eq!(
            codes(&check_contradictions(&create, &index(), &[])),
            [CONTRADICTS_ACTIVE_DEFINITION.as_str()]
        );

        // An invariant that forbids retiring a protected target fires once.
        let invariant = Invariant {
            invariant_id: "no-retire-orders".into(),
            target_prefix: "source:".into(),
            forbidden_ops: vec![CurationEditOp::Retire],
        };
        let retire = proposal(CurationEditOp::Retire, Some("v1"), "v1");
        let found = check_contradictions(&retire, &index(), &[invariant.clone()]);
        assert_eq!(codes(&found), [CONTRADICTS_INVARIANT.as_str()]);
        assert_eq!(found[0].other_ref.as_deref(), Some("no-retire-orders"));
        assert!(check_contradictions(&clean, &index(), &[invariant]).is_empty());

        // An unknown reference fires once per reference; a known one does not.
        let mut refs = clean.clone();
        refs.edit.references = vec!["orders.amount".into(), "orders.total".into()];
        let found = check_contradictions(&refs, &index(), &[]);
        assert_eq!(codes(&found), [REFERENCE_UNRESOLVED.as_str()]);
        assert_eq!(found[0].other_ref.as_deref(), Some("orders.total"));
    }

    // spec: MEDULLA-DECISION-ENGINE-R709.7
    #[test]
    fn cadence_and_metrics_are_total() {
        assert!(is_due(None, 5, 300));
        assert!(!is_due(Some(1_000), 200_000, 300));
        assert!(is_due(Some(1_000), 301_000, 300));
        let measure = |raised, state, reopened, times, resolved| FlagMeasure {
            raised_at_ms: raised,
            state,
            reopened,
            curated_times: times,
            resolved_at_ms: resolved,
        };
        let flags = [
            measure(100, FlagState::Curated, false, 1, Some(300)),
            measure(200, FlagState::Curated, false, 1, Some(800)),
            measure(300, FlagState::Open, true, 1, Some(500)),
            measure(400, FlagState::Dismissed, false, 0, Some(450)),
            measure(9_000, FlagState::Open, false, 0, None),
        ];
        let metrics = loop_metrics(&flags, 0, 1_000);
        assert_eq!(metrics.flags_raised, 4);
        assert_eq!(metrics.flags_curated, 2);
        assert_eq!(metrics.flags_dismissed, 1);
        // Spans are 200 and 600: the median of two is their mean.
        assert_eq!(metrics.median_flag_to_fix_ms, 400);
        // Three flags were ever curated and one came back.
        assert_eq!(metrics.recurrence_ppm, 333_333);
        assert_eq!(loop_metrics(&[], 0, 10), LoopMetrics::default());
    }
}
