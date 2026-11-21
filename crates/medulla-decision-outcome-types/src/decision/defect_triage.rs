//! Defect triage as a decision (MEDULLA-SDLC-R004): the record the engine decides
//! for one defect, and the pure ranking that feeds it.
//!
//! The decision names severity, suspected symbols, an owner and a duplicate
//! link, or abstains with a typed reason. Nothing here reads a clock or a float;
//! equal inputs produce equal records.

use serde::{Deserialize, Serialize};

use medulla_decision_types::decision::codes;
use medulla_decision_types::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_decision_types::decision::record::outcome::RefusalCode;
use medulla_decision_types::decision::DecisionPolicy;
use medulla_source_types::lifecycle::Severity;

/// A triage that names a defect the engine does not hold.
pub const DEFECT_UNKNOWN: RefusalCode = RefusalCode::new("DEFECT_UNKNOWN");
/// A triage of a defect that is already closed.
pub const DEFECT_CLOSED: RefusalCode = RefusalCode::new("DEFECT_CLOSED");

/// The refusal codes of the `DefectTriage` family (MEDULLA-SDLC-R004.7), in
/// declaration order. The three `TRIAGE_*` codes map one to one onto
/// [`TriageAbstainReason`].
pub const REFUSALS: &[RefusalCode] = &[
    DEFECT_UNKNOWN,
    DEFECT_CLOSED,
    codes::TRIAGE_NO_SYMBOL_MATCH,
    codes::TRIAGE_NO_OWNER,
    codes::TRIAGE_INSUFFICIENT_EVIDENCE,
];

/// The `triage.*` knobs of the `DefectTriage` family (MEDULLA-SDLC-R004.7):
/// `max_frames`, `max_suspects`, `blame_window_days`, `escalate_symptoms` and
/// `duplicate_min_overlap` (a fixed-point fraction, the value times 1,000,000).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "triage.max_frames",
        unit: "frames",
        default: KnobValue::Integer(50),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(500),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "triage.max_suspects",
        unit: "symbols",
        default: KnobValue::Integer(10),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(100),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "triage.blame_window_days",
        unit: "days",
        default: KnobValue::Integer(90),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(3650),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "triage.escalate_symptoms",
        unit: "symptoms",
        default: KnobValue::Integer(10),
        min: KnobValue::Integer(2),
        max: KnobValue::Integer(10_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "triage.duplicate_min_overlap",
        unit: "fraction",
        default: KnobValue::DecimalMicros(800_000),
        min: KnobValue::DecimalMicros(500_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
];

/// The triage limits the pinned policy sets, each knob or its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriageKnobs {
    pub max_frames: u32,
    pub max_suspects: u32,
    pub blame_window_days: u32,
    pub escalate_symptoms: u32,
    /// The least frame overlap that makes two defects duplicates, in millionths.
    pub duplicate_min_overlap_micros: i64,
}

impl TriageKnobs {
    /// Read every knob through [`DecisionPolicy::knob`].
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let count = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(0),
            _ => 0,
        };
        let overlap = match policy.knob(&KNOBS[4]) {
            KnobValue::DecimalMicros(value) => value,
            _ => 0,
        };
        Self {
            max_frames: count(0),
            max_suspects: count(1),
            blame_window_days: count(2),
            escalate_symptoms: count(3),
            duplicate_min_overlap_micros: overlap,
        }
    }
}

/// Why a triage abstained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageAbstainReason {
    NoSymbolMatch,
    NoOwner,
    InsufficientEvidence,
}

impl medulla_decision_types::decision::record::outcome::DecisionRefusal for TriageAbstainReason {
    fn code(&self) -> medulla_decision_types::decision::record::outcome::RefusalCode {
        match self {
            Self::NoSymbolMatch => medulla_decision_types::decision::codes::TRIAGE_NO_SYMBOL_MATCH,
            Self::NoOwner => medulla_decision_types::decision::codes::TRIAGE_NO_OWNER,
            Self::InsufficientEvidence => {
                medulla_decision_types::decision::codes::TRIAGE_INSUFFICIENT_EVIDENCE
            }
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::NoSymbolMatch => "no symbol matches the symptom",
            Self::NoOwner => "no owner for the suspected symbols",
            Self::InsufficientEvidence => "not enough evidence to triage",
        }
        .to_string()
    }
}

/// The triage of one defect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriageDecision {
    pub defect_id: String,
    pub severity: Option<Severity>,
    pub suspected_symbols: Vec<String>,
    pub owner_principal: Option<String>,
    pub duplicate_of: Option<String>,
    pub abstain_reason: Option<TriageAbstainReason>,
    pub evidence_refs: Vec<String>,
}

/// A code symbol named by a symptom frame, with its blame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuspectInput {
    pub symbol_id: String,
    /// The changeset that last touched the symbol.
    pub blame_changeset: String,
    /// Blame recency: the time of that changeset.
    pub blame_at: u64,
}

/// An open defect of a repository, with its ranked suspect symbols.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenDefect {
    pub defect_id: String,
    pub repository: String,
    pub suspects: Vec<String>,
}

const DAY_MS: u64 = 86_400_000;

/// Keep the first `knobs.max_frames` symptom frames, in order.
pub fn cap_frames<T: Clone>(frames: &[T], knobs: &TriageKnobs) -> Vec<T> {
    frames
        .iter()
        .take(knobs.max_frames as usize)
        .cloned()
        .collect()
}

/// Rank suspects: drop blame older than the window before `as_of_ms`, then
/// symbols last changed by the bisect culprit first, then the most recently
/// changed, then by symbol id so equal inputs rank equally. Repeated symbols
/// keep their first place and the list is cut to `knobs.max_suspects`.
pub fn rank_suspects(
    inputs: &[SuspectInput],
    culprit: Option<&str>,
    as_of_ms: u64,
    knobs: &TriageKnobs,
) -> Vec<String> {
    let window_ms = u64::from(knobs.blame_window_days) * DAY_MS;
    let mut ranked: Vec<&SuspectInput> = inputs
        .iter()
        .filter(|input| input.blame_at.saturating_add(window_ms) >= as_of_ms)
        .collect();
    ranked.sort_by(|a, b| {
        let culprit_a = culprit == Some(a.blame_changeset.as_str());
        let culprit_b = culprit == Some(b.blame_changeset.as_str());
        culprit_b
            .cmp(&culprit_a)
            .then(b.blame_at.cmp(&a.blame_at))
            .then_with(|| a.symbol_id.cmp(&b.symbol_id))
    });
    let mut seen: Vec<String> = Vec::new();
    for suspect in ranked {
        if !seen.contains(&suspect.symbol_id) {
            seen.push(suspect.symbol_id.clone());
        }
    }
    seen.truncate(knobs.max_suspects as usize);
    seen
}

/// Raise the severity one level when the defect has at least
/// `knobs.escalate_symptoms` symptoms; `Critical` stays.
pub fn escalate_severity(
    severity: Severity,
    symptom_count: u32,
    knobs: &TriageKnobs,
) -> Severity {
    if symptom_count < knobs.escalate_symptoms {
        return severity;
    }
    match severity {
        Severity::Low => Severity::Medium,
        Severity::Medium => Severity::High,
        Severity::High | Severity::Critical => Severity::Critical,
    }
}

/// The open defect of the same repository whose suspects overlap these the
/// most, at or above `knobs.duplicate_min_overlap_micros`. The overlap is
/// `intersection * 1_000_000 / union` over distinct symbols; the lowest defect
/// id wins a tie.
pub fn find_duplicate(
    repository: &str,
    suspects: &[String],
    open: &[OpenDefect],
    knobs: &TriageKnobs,
) -> Option<String> {
    let mut mine: Vec<&str> = suspects.iter().map(String::as_str).collect();
    mine.sort_unstable();
    mine.dedup();
    let mut best: Option<(i64, &str)> = None;
    for defect in open.iter().filter(|defect| defect.repository == repository) {
        let mut theirs: Vec<&str> = defect.suspects.iter().map(String::as_str).collect();
        theirs.sort_unstable();
        theirs.dedup();
        let intersection = mine.iter().filter(|symbol| theirs.contains(symbol)).count() as i64;
        let union = mine.len() as i64 + theirs.len() as i64 - intersection;
        let overlap = if union == 0 {
            0
        } else {
            intersection * 1_000_000 / union
        };
        if overlap < knobs.duplicate_min_overlap_micros {
            continue;
        }
        let better = match best {
            None => true,
            Some((top, id)) => {
                overlap > top || (overlap == top && defect.defect_id.as_str() < id)
            }
        };
        if better {
            best = Some((overlap, defect.defect_id.as_str()));
        }
    }
    best.map(|(_, id)| id.to_owned())
}

/// Pick the owner from path ownership records first, then recent authors.
pub fn pick_owner(path_owners: &[String], recent_authors: &[String]) -> Option<String> {
    path_owners
        .first()
        .or_else(|| recent_authors.first())
        .cloned()
}

/// Decide a triage from ranked evidence. It abstains, with the typed reason,
/// when the evidence cannot support a decision.
pub fn decide_triage(
    defect_id: &str,
    severity: Severity,
    symptom_count: u32,
    suspects: Vec<String>,
    owner: Option<String>,
    duplicate_of: Option<String>,
    evidence_refs: Vec<String>,
    knobs: &TriageKnobs,
) -> TriageDecision {
    let abstain = if evidence_refs.is_empty() {
        Some(TriageAbstainReason::InsufficientEvidence)
    } else if suspects.is_empty() {
        Some(TriageAbstainReason::NoSymbolMatch)
    } else if owner.is_none() {
        Some(TriageAbstainReason::NoOwner)
    } else {
        None
    };
    TriageDecision {
        defect_id: defect_id.to_owned(),
        severity: abstain
            .is_none()
            .then(|| escalate_severity(severity, symptom_count, knobs)),
        suspected_symbols: suspects,
        owner_principal: owner,
        duplicate_of,
        abstain_reason: abstain,
        evidence_refs,
    }
}

impl TriageDecision {
    /// The `affects` edges (defect to symbol) this decision sets. The
    /// `introducedBy` edge comes from the `introduced.bisect` derivation rule.
    pub fn affects_edges(&self) -> Vec<(String, String)> {
        self.suspected_symbols
            .iter()
            .map(|symbol| (self.defect_id.clone(), symbol.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knobs() -> TriageKnobs {
        TriageKnobs::from_policy(&DecisionPolicy::engine_default())
    }

    fn suspect(symbol: &str, changeset: &str, at: u64) -> SuspectInput {
        SuspectInput {
            symbol_id: symbol.into(),
            blame_changeset: changeset.into(),
            blame_at: at,
        }
    }

    // spec: MEDULLA-SDLC-R004.1
    #[test]
    fn decision_round_trips() {
        let decision = decide_triage(
            "d1",
            Severity::High,
            1,
            vec!["sym::a".into()],
            Some("alice".into()),
            Some("d0".into()),
            vec!["gv-1".into()],
            &knobs(),
        );
        let json = serde_json::to_string(&decision).unwrap();
        assert_eq!(
            serde_json::from_str::<TriageDecision>(&json).unwrap(),
            decision
        );

        let abstained = decide_triage(
            "d2",
            Severity::Low,
            1,
            vec![],
            None,
            None,
            vec!["x".into()],
            &knobs(),
        );
        assert_eq!(
            abstained.abstain_reason,
            Some(TriageAbstainReason::NoSymbolMatch)
        );
        assert_eq!(abstained.severity, None);
        let json = serde_json::to_string(&abstained).unwrap();
        assert!(json.contains("no_symbol_match"));
        assert!(
            serde_json::from_str::<TriageDecision>(&json.replace("no_symbol_match", "shrug"))
                .is_err()
        );

        let ownerless = decide_triage(
            "d3",
            Severity::Low,
            1,
            vec!["s".into()],
            None,
            None,
            vec!["x".into()],
            &knobs(),
        );
        assert_eq!(ownerless.abstain_reason, Some(TriageAbstainReason::NoOwner));
        let thin = decide_triage(
            "d4",
            Severity::Low,
            1,
            vec!["s".into()],
            Some("o".into()),
            None,
            vec![],
            &knobs(),
        );
        assert_eq!(
            thin.abstain_reason,
            Some(TriageAbstainReason::InsufficientEvidence)
        );
    }

    // spec: MEDULLA-SDLC-R004.2
    #[test]
    fn culprit_outranks_blame() {
        let inputs = [
            suspect("recent", "c9", 900),
            suspect("older", "c5", 500),
            suspect("culprit_sym", "c2", 200),
            suspect("recent", "c9", 900),
        ];
        assert_eq!(
            rank_suspects(&inputs, Some("c2"), 1_000, &knobs()),
            ["culprit_sym", "recent", "older"]
        );
        assert_eq!(
            rank_suspects(&inputs, None, 1_000, &knobs()),
            ["recent", "older", "culprit_sym"]
        );
        assert_eq!(
            rank_suspects(&inputs, Some("c2"), 1_000, &knobs()),
            rank_suspects(&inputs, Some("c2"), 1_000, &knobs())
        );

        let owners = ["team".to_string()];
        let authors = ["bob".to_string()];
        assert_eq!(pick_owner(&owners, &authors).as_deref(), Some("team"));
        assert_eq!(pick_owner(&[], &authors).as_deref(), Some("bob"));
        assert_eq!(pick_owner(&[], &[]), None);

        let decision = decide_triage(
            "d1",
            Severity::Medium,
            1,
            rank_suspects(&inputs, Some("c2"), 1_000, &knobs()),
            pick_owner(&owners, &authors),
            None,
            vec!["culprit:c2".into()],
            &knobs(),
        );
        assert_eq!(
            decision.affects_edges()[0],
            ("d1".to_string(), "culprit_sym".to_string())
        );
    }

    // spec: MEDULLA-SDLC-R004.8
    #[test]
    fn window_cap_escalation_and_duplicate_follow_the_knobs() {
        let knobs = TriageKnobs {
            max_frames: 2,
            max_suspects: 2,
            blame_window_days: 1,
            escalate_symptoms: 3,
            duplicate_min_overlap_micros: 500_000,
        };
        let as_of = 10 * DAY_MS;
        let edge = as_of - DAY_MS;
        let inputs = [
            suspect("edge", "c1", edge),
            suspect("past", "c2", edge - 1),
            suspect("b", "c3", as_of),
            suspect("a", "c4", as_of),
        ];
        assert_eq!(rank_suspects(&inputs, None, as_of, &knobs), ["a", "b"]);
        let wide = TriageKnobs {
            max_suspects: 10,
            ..knobs
        };
        assert_eq!(rank_suspects(&inputs, None, as_of, &wide), ["a", "b", "edge"]);
        assert_eq!(cap_frames(&[1, 2, 3], &knobs), [1, 2]);

        assert_eq!(escalate_severity(Severity::Low, 2, &knobs), Severity::Low);
        assert_eq!(escalate_severity(Severity::Low, 3, &knobs), Severity::Medium);
        assert_eq!(escalate_severity(Severity::Medium, 3, &knobs), Severity::High);
        assert_eq!(escalate_severity(Severity::High, 3, &knobs), Severity::Critical);
        assert_eq!(
            escalate_severity(Severity::Critical, 9, &knobs),
            Severity::Critical
        );

        let open = |id: &str, repo: &str, symbols: &[&str]| OpenDefect {
            defect_id: id.into(),
            repository: repo.into(),
            suspects: symbols.iter().map(|s| (*s).to_owned()).collect(),
        };
        let mine = vec!["a".to_owned(), "b".to_owned()];
        // overlap exactly 500_000: {a} of {a,b,c} is 1/3 (no), {a,b} of {a,b,c,d} is 1/2.
        let defects = [
            open("d9", "r", &["a", "b", "c", "d"]),
            open("d1", "r", &["a", "b", "c", "d"]),
            open("d0", "other", &["a", "b"]),
        ];
        assert_eq!(find_duplicate("r", &mine, &defects, &knobs).as_deref(), Some("d1"));
        assert_eq!(find_duplicate("none", &mine, &defects, &knobs), None);
        assert_eq!(
            find_duplicate("r", &mine, &[open("d2", "r", &["a", "c", "d"])], &knobs),
            None
        );
        assert_eq!(find_duplicate("r", &[], &[open("d3", "r", &[])], &knobs), None);
    }

    // spec: MEDULLA-SDLC-R004.7
    #[test]
    fn knob_specs_lie_within_bounds_and_codes_map_abstain_reasons() {
        use medulla_decision_types::decision::record::outcome::DecisionRefusal;

        assert_eq!(KNOBS.len(), 5);
        for spec in KNOBS {
            assert!(spec.name.starts_with("triage."), "{}", spec.name);
            assert_eq!(spec.safety, KnobSafety::Tunable, "{}", spec.name);
            assert!(spec.admits(&spec.default), "{}", spec.name);
            assert!(spec.admits(&spec.min) && spec.admits(&spec.max));
        }
        let knobs = TriageKnobs::from_policy(&DecisionPolicy::engine_default());
        assert_eq!(
            knobs,
            TriageKnobs {
                max_frames: 50,
                max_suspects: 10,
                blame_window_days: 90,
                escalate_symptoms: 10,
                duplicate_min_overlap_micros: 800_000,
            }
        );

        let codes: Vec<&str> = REFUSALS.iter().map(|code| code.as_str()).collect();
        assert_eq!(
            codes,
            [
                "DEFECT_UNKNOWN",
                "DEFECT_CLOSED",
                "TRIAGE_NO_SYMBOL_MATCH",
                "TRIAGE_NO_OWNER",
                "TRIAGE_INSUFFICIENT_EVIDENCE"
            ]
        );
        for (reason, code) in [
            (TriageAbstainReason::NoSymbolMatch, REFUSALS[2]),
            (TriageAbstainReason::NoOwner, REFUSALS[3]),
            (TriageAbstainReason::InsufficientEvidence, REFUSALS[4]),
        ] {
            assert_eq!(reason.code(), code);
        }
    }
}
