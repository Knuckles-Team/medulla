//! The team read of the `team_coordination` family
//! (MEDULLA-DECISION-ENGINE-R170.2).
//!
//! The question names a graph (`graph`), a `Team` node (`team`) and carries the
//! caller's `channel_signal`. The family reads the caller's RLS-filtered
//! snapshot of that graph: a `Team` node and its `TeamMember` nodes, each
//! member naming its team in `boundToTeam`. Nodes of another tenant are
//! skipped. `Decide` is evaluate-only: nothing is written.

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::codes::{
    TEAM_MEMBER_UNBOUND, TEAM_NO_FALLBACK_AVAILABLE, TEAM_SIGNAL_MISSING, TEAM_SIGNAL_STALE,
};
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::team_coordination::ChannelSignal;
use medulla_decision_types::decision::DecisionPolicy;
use medulla_decision_types::decision::{
    AbstainReason, PremiseClass, PremiseProvenance, PremiseRef,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::{BoundedVec, Digest256};

use super::stat_executor::{AnsweredFamily, Executed};
use super::stat_support::{filtered_view, invalid_parameter};

const PARAM_GRAPH: &str = "graph";
const PARAM_TEAM: &str = "team";
const PARAM_SIGNAL: &str = "channel_signal";
/// The option taken while the channel is available and its signal fresh.
pub(super) const PRIMARY_CHANNEL: &str = "primary_channel";
/// The option taken while the channel is unavailable and a member is bound.
pub(super) const TEAM_COORDINATION: &str = "team_coordination";
/// Marker property of a `Team` node.
const TEAM_MARKER: &str = "_team";
/// Marker property of a `TeamMember` node.
const MEMBER_MARKER: &str = "_team_member";
/// The property a `TeamMember` names its team with.
const BOUND_TO_TEAM: &str = "boundToTeam";

fn text_param<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    request
        .params
        .iter()
        .find(|param| param.name == name)
        .and_then(|param| match &param.value {
            TypedValue::Iri(value) | TypedValue::Text(value) if !value.trim().is_empty() => {
                Some(value.as_str())
            }
            _ => None,
        })
        .ok_or_else(|| {
            invalid_parameter(format!(
                "a team_coordination question requires a {name} param"
            ))
        })
}

fn signal_param(request: &DecideRequest) -> Result<&ChannelSignal, String> {
    request
        .params
        .iter()
        .find(|param| param.name == PARAM_SIGNAL)
        .and_then(|param| match &param.value {
            TypedValue::ChannelSignal(signal) => Some(signal),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "{}: a team_coordination question requires a typed channel_signal param",
                TEAM_SIGNAL_MISSING.as_str()
            )
        })
}

/// The Constraints stage: the question declares exactly the two options and
/// carries its three params.
pub(super) fn admit(request: &DecideRequest) -> Result<(), String> {
    text_param(request, PARAM_GRAPH)?;
    text_param(request, PARAM_TEAM)?;
    signal_param(request).map(|_| ())
}

/// The Candidates stage: exactly `primary_channel` and `team_coordination`.
pub(super) fn validate_options(options: &[DeclaredOption]) -> Result<(), String> {
    let mut ids: Vec<&str> = options
        .iter()
        .map(|option| option.option_id.as_str())
        .collect();
    ids.sort_unstable();
    if ids == [PRIMARY_CHANNEL, TEAM_COORDINATION] {
        Ok(())
    } else {
        Err(invalid_parameter(
            "a team_coordination question declares exactly primary_channel and team_coordination",
        ))
    }
}

/// Open the graph a `team_coordination` question names, under the same
/// RLS-filtered snapshot a graph-sourced candidate plan gets.
pub(super) async fn team_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<GraphView, String> {
    filtered_view(state, agent_id, text_param(request, PARAM_GRAPH)?).await
}

/// Whether a node belongs to the asking tenant. A node that carries no tenant
/// belongs to the graph and counts.
fn visible(row: &serde_json::Map<String, serde_json::Value>, tenant: &str) -> bool {
    row.get("tenant_id")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|owner| owner == tenant)
}

fn marked(row: &serde_json::Map<String, serde_json::Value>, marker: &str) -> bool {
    row.get(marker) == Some(&serde_json::Value::Bool(true))
}

/// What one team question reads from the caller's graph view: the visible
/// members that explicitly name the team in `boundToTeam`. `None` when the
/// team node itself is not visible.
fn bound_members(view: &GraphView, team: &str, tenant: &str) -> Option<Vec<String>> {
    view.node_row_object(team)
        .filter(|row| marked(row, TEAM_MARKER) && visible(row, tenant))?;
    let mut members: Vec<String> = view
        .node_properties
        .keys()
        .filter(|id| {
            view.node_row_object(id).is_some_and(|row| {
                marked(&row, MEMBER_MARKER)
                    && visible(&row, tenant)
                    && row.get(BOUND_TO_TEAM).and_then(serde_json::Value::as_str) == Some(team)
            })
        })
        .cloned()
        .collect();
    members.sort();
    Some(members)
}

/// The first visible member node that names no team at all. A member is bound
/// only by an explicit `boundToTeam`; an unbound member is never read as
/// belonging to the asked team (MEDULLA-DECISION-ENGINE-R170.4).
fn unbound_member(view: &GraphView, tenant: &str) -> Option<String> {
    let mut ids: Vec<&String> = view.node_properties.keys().collect();
    ids.sort();
    ids.into_iter().find_map(|id| {
        let row = view.node_row_object(id)?;
        (marked(&row, MEMBER_MARKER)
            && visible(&row, tenant)
            && row
                .get(BOUND_TO_TEAM)
                .and_then(serde_json::Value::as_str)
                .is_none_or(|team| team.trim().is_empty()))
        .then(|| id.clone())
    })
}

fn q32(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value: value << 32,
    }
}

fn executed(outcome: StatisticalOutcome) -> Executed {
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        topology: None,
        thought: None,
        context_selection: None,
        family_premises: Vec::new(),
        route: None,
        answered: None,
    }
}

fn abstained(code: medulla_types::refusal_code::RefusalCode, detail: String) -> Executed {
    executed(StatisticalOutcome::Abstained {
        reasons: BoundedVec::new(vec![AbstainReason::Family {
            family: QuestionKind::TeamCoordination,
            code,
            detail,
        }])
        .expect("one reason fits the bound"),
    })
}

/// The freshness limit the pinned policy sets, in seconds (the
/// `team.signal_max_age` decision-policy knob, DECISIONS DE-R059.1-KNOBS).
fn signal_max_age_s(policy: &DecisionPolicy) -> u64 {
    match policy.knob(&medulla_decision_types::decision::team_coordination::KNOBS[0]) {
        KnobValue::Integer(seconds) => u64::try_from(seconds).unwrap_or(0),
        _ => 0,
    }
}

/// Answer one team coordination question at `now_ms` over the caller's
/// filtered graph view. Exactly one legal option concludes; an unreadable
/// request is refused, a well-formed one with no legal option abstains.
pub(super) fn conclude(
    request: &DecideRequest,
    tenant: &str,
    now_ms: u64,
    policy: &DecisionPolicy,
    graph_view: Option<&GraphView>,
) -> Result<Executed, String> {
    let view = graph_view.ok_or_else(|| {
        invalid_parameter("a team_coordination question reads the graph its graph param names")
    })?;
    let signal = signal_param(request)?;
    let team = text_param(request, PARAM_TEAM)?;
    if let Some(member) = unbound_member(view, tenant) {
        return Err(format!(
            "{}: member {member} names no team in boundToTeam",
            TEAM_MEMBER_UNBOUND.as_str()
        ));
    }
    let max_age_ms = signal_max_age_s(policy).saturating_mul(1000);
    let age_ms = now_ms.saturating_sub(signal.observed_at_ms);
    let fresh = age_ms <= max_age_ms;
    let members = bound_members(view, team, tenant).unwrap_or_default();
    let primary_legal = signal.available && fresh;
    let team_legal = !signal.available && !members.is_empty();
    let chosen = match (primary_legal, team_legal) {
        (true, _) => PRIMARY_CHANNEL,
        (_, true) => TEAM_COORDINATION,
        _ if signal.available => {
            return Ok(abstained(
                TEAM_SIGNAL_STALE,
                format!(
                    "the signal for channel {} is {age_ms} ms old; the limit is {max_age_ms} ms",
                    signal.channel_id.as_str()
                ),
            ))
        }
        _ => {
            return Ok(abstained(
                TEAM_NO_FALLBACK_AVAILABLE,
                format!("team {team} has no visible member bound to it"),
            ))
        }
    };
    let score = |id: &str| ScoredOption {
        option_id: id.to_string(),
        score: q32(i64::from(id == chosen)),
        probability: None,
    };
    let mut concluded = executed(StatisticalOutcome::Advisory {
        scores: BoundedVec::new(vec![score(PRIMARY_CHANNEL), score(TEAM_COORDINATION)])
            .expect("two scores fit the bound"),
        calibrated: false,
    });
    concluded.answered = Some(AnsweredFamily {
        answer: None,
        premises: premises(request, tenant, graph_view),
        resolution: None,
        evidence: None,
    });
    Ok(concluded)
}

/// The premises of a team decision (MEDULLA-DECISION-ENGINE-R170.5): the
/// channel signal with its time (`Claim`, the caller observed it) and the digest
/// of the team's bound member ids (`Observation`, read from the graph).
pub(super) fn premises(
    request: &DecideRequest,
    tenant: &str,
    graph_view: Option<&GraphView>,
) -> Vec<PremiseRef> {
    let (Ok(signal), Ok(team), Some(view)) = (
        signal_param(request),
        text_param(request, PARAM_TEAM),
        graph_view,
    ) else {
        return Vec::new();
    };
    let members = bound_members(view, team, tenant).unwrap_or_default();
    let channel = signal.channel_id.as_str();
    vec![
        PremiseRef {
            subject: team.to_string(),
            fact: format!(
                "team:channel:{channel} available={} observed_at_ms={}",
                signal.available, signal.observed_at_ms
            ),
            class: PremiseClass::Claim,
            provenance: PremiseProvenance::Request {
                field: PARAM_SIGNAL.to_string(),
            },
        },
        PremiseRef {
            subject: team.to_string(),
            fact: format!(
                "team:members:{team} digest={}",
                Digest256::sha256(members.join("\n").as_bytes()).to_hex()
            ),
            class: PremiseClass::Observation,
            provenance: PremiseProvenance::Observation {
                evaluation_id: channel.to_string(),
            },
        },
    ]
}
