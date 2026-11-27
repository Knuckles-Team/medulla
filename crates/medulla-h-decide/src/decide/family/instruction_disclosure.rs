//! The `instruction_disclosure` question family (MEDULLA-DECISION-ENGINE-R137.100).
//!
//! Which versions of the named instruction sets a run discloses now. The
//! `initial` phase discloses the latest initial version of each set; the
//! `runtime` phase discloses the latest runtime version whose trigger has
//! occurred, and a runtime version stays withheld until then. The versions are
//! tenant graph nodes read from the graph the `graph` param names, under the
//! caller's row-level filter. `Decide` is evaluate-only and writes nothing.
//!
//! The same kind also decides the next disclosure EVENT of one set
//! (MEDULLA-DECISION-ENGINE-R143.3, DECISIONS DE-ONE-INSTRUCTION-DISCLOSURE):
//! a request with a `trigger` param and no `phase` carries one declared option
//! (the set id, its `instructions` text, classification `initial` or
//! `runtime_refinement`). The prior event is read from the caller's own
//! decision log through `ExecutionContext.reader`; the answer is the typed
//! event, sealed as the record's `FamilyAnswer::DisclosureEvent`.

use std::collections::BTreeSet;

use medulla_decision_stats::decision::instruction_disclosure::{
    next_disclosure_event, DisclosureInput,
};
use medulla_decision_types::decision::instruction_disclosure::{
    instruction_version_prefix, instructions_hash, scan_limit, timeout_s, DisclosureEvent,
    DisclosureTiming, DisclosureTrigger, InstructionVersion, DISCLOSURE_SET_UNDEFINED,
};
use medulla_decision_types::decision::record::outcome::AbstainReason;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, StatisticalErrorCode, StatisticalOutcome,
    TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef, ResolutionKind};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyExecuted, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::{filtered_view, refusal};

use super::support::{
    advisory, invalid, json_param, param, single_option, text_param, unsigned_param,
};
use super::DecisionFamily;

/// The one option an instruction disclosure question declares.
pub const DISCLOSURE_OPTION: &str = "disclosure";

/// An event option's classification for the `initial` trigger.
const EVENT_INITIAL: &str = "initial";

/// An event option's classification for every runtime refinement.
const EVENT_REFINEMENT: &str = "runtime_refinement";

/// The most instruction sets one question names.
const MAX_SETS: usize = 64;

/// The graph the versions are read from.
const PARAM_GRAPH: &str = "graph";

/// The family of the `instruction_disclosure` question kind.
pub struct InstructionDisclosureFamily;

/// What the request asks.
struct Asked {
    timing: DisclosureTiming,
    run_id: String,
    sequence: u64,
    occurred: BTreeSet<String>,
    sets: Vec<String>,
    pinned: std::collections::BTreeMap<String, String>,
}

fn asked(request: &DecideRequest) -> Result<Asked, String> {
    let word =
        text_param(request, "phase")?.ok_or_else(|| invalid("parameter phase is required"))?;
    let timing = DisclosureTiming::parse(word)
        .ok_or_else(|| invalid("parameter phase is `initial` or `runtime`"))?;
    let run_id = text_param(request, "run_id")?
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| invalid("parameter run_id is required"))?
        .to_string();
    let sequence = unsigned_param(request, "sequence")?
        .ok_or_else(|| invalid("parameter sequence is required"))?;
    let occurred: BTreeSet<String> = match param(request, "occurred_triggers")? {
        None => BTreeSet::new(),
        Some(TypedValue::IriList(iris)) => iris.iter().cloned().collect(),
        Some(_) => return Err(invalid("parameter occurred_triggers is not an IRI list")),
    };
    if timing == DisclosureTiming::Initial && !occurred.is_empty() {
        return Err(invalid("occurred_triggers apply to the runtime phase only"));
    }
    let mut sets: Vec<String> = json_param(request, "instruction_set_ids")?;
    sets.sort();
    sets.dedup();
    if sets.is_empty() || sets.len() > MAX_SETS || sets.iter().any(|set| set.trim().is_empty()) {
        return Err(invalid(format!(
            "parameter instruction_set_ids names between 1 and {MAX_SETS} non-empty sets"
        )));
    }
    let pinned = match param(request, "pinned_digests")? {
        None => std::collections::BTreeMap::new(),
        Some(_) => json_param(request, "pinned_digests")?,
    };
    Ok(Asked {
        timing,
        run_id,
        sequence,
        occurred,
        sets,
        pinned,
    })
}

/// Open the graph a disclosure question names before any read, through the
/// caller's row-level filter.
pub(in crate::decide) async fn disclosure_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<GraphView>, String> {
    if event_mode(request)? {
        return Ok(None);
    }
    let graph = text_param(request, PARAM_GRAPH)?
        .filter(|graph| !graph.trim().is_empty())
        .ok_or_else(|| invalid("an instruction_disclosure question requires a graph param"))?;
    filtered_view(state, agent_id, graph).await.map(Some)
}

/// Whether the request asks for a disclosure event (a `trigger`, no `phase`)
/// rather than a graph disclosure (a `phase`, no `trigger`). A request with
/// neither is an event request without its trigger.
fn event_mode(request: &DecideRequest) -> Result<bool, String> {
    let trigger = param(request, "trigger")?.is_some();
    let phase = param(request, "phase")?.is_some();
    if trigger && phase {
        return Err(invalid(
            "parameters trigger and phase are exclusive: a request asks for an event or a graph disclosure",
        ));
    }
    Ok(!phase)
}

/// What an event request asks.
struct AskedEvent<'a> {
    trigger: DisclosureTrigger,
    set_id: &'a str,
    instructions: Option<&'a str>,
    prior_version: Option<u64>,
    elapsed_s: u64,
    expected_hash: Option<&'a str>,
}

/// The one declared option of an event request.
fn event_option(
    request: &DecideRequest,
) -> Result<&medulla_decision_types::decision::statistical::declared::DeclaredOption, String> {
    match &request.candidates {
        CandidateSource::Declared { options } if options.len() == 1 => Ok(&options.as_slice()[0]),
        _ => Err(invalid(
            "a disclosure event question declares exactly one option: the instruction set",
        )),
    }
}

fn asked_event(request: &DecideRequest) -> Result<AskedEvent<'_>, String> {
    let word = text_param(request, "trigger")?.ok_or_else(|| {
        refusal(
            StatisticalErrorCode::DisclosureTriggerMissing,
            "parameter trigger is required",
        )
    })?;
    let trigger = DisclosureTrigger::parse(word).ok_or_else(|| {
        refusal(
            StatisticalErrorCode::DisclosureTriggerMissing,
            format!(
                "trigger `{word}` is not initial, relevance_update, constraint_change or timeout"
            ),
        )
    })?;
    let option = event_option(request)?;
    let class = if trigger.is_runtime_refinement() {
        EVENT_REFINEMENT
    } else {
        EVENT_INITIAL
    };
    if option.classification.as_slice().first().map(String::as_str) != Some(class) {
        return Err(invalid(format!(
            "the option's classification is `{class}` for trigger `{word}`"
        )));
    }
    let prior_version = unsigned_param(request, "prior_version")?;
    if trigger == DisclosureTrigger::Initial && prior_version.is_some() {
        return Err(invalid("the initial disclosure carries no prior_version"));
    }
    Ok(AskedEvent {
        trigger,
        set_id: option.option_id.as_str(),
        instructions: option
            .texts
            .as_slice()
            .iter()
            .find(|text| text.key == "instructions")
            .map(|text| text.text.as_str()),
        prior_version,
        elapsed_s: unsigned_param(request, "elapsed_s")?.unwrap_or(0),
        expected_hash: text_param(request, "expected_hash")?,
    })
}

/// The registered refusal a `next_disclosure_event` refusal code names.
fn event_refusal(code: &str, detail: String) -> String {
    let known = StatisticalErrorCode::ALL
        .iter()
        .find(|known| known.as_str() == code)
        .copied()
        .unwrap_or(StatisticalErrorCode::ParameterInvalid);
    refusal(known, detail)
}

/// Conclude one disclosure-event question: version and hash the submitted
/// trigger against the set's latest committed event.
fn conclude_event(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let ask = asked_event(request)?;
    let Some(instructions) = ask.instructions else {
        let mut executed = advisory(Vec::new())?;
        executed.ladder.outcome = StatisticalOutcome::Abstained {
            reasons: medulla_types::contract::BoundedVec::new(vec![AbstainReason::Family {
                family: QuestionKind::InstructionDisclosure,
                code: DISCLOSURE_SET_UNDEFINED,
                detail: format!(
                    "instruction set {} carries no instructions text",
                    ask.set_id
                ),
            }])
            .map_err(|detail| refusal(StatisticalErrorCode::ParameterInvalid, detail))?,
        };
        return Ok(executed);
    };
    let reader = ctx
        .reader
        .ok_or_else(|| invalid("a disclosure event question reads the caller's decision log"))?;
    let prior = super::super::stat_log::latest_disclosure(
        ctx.store,
        reader,
        ask.set_id,
        scan_limit(policy),
    )?;
    if let (Some(seen), Some(latest)) = (ask.prior_version, prior.as_ref()) {
        if seen < u64::from(latest.version) {
            return Err(refusal(
                StatisticalErrorCode::DisclosureVersionStale,
                format!(
                    "prior_version {seen} of set {} is below the latest committed version {}",
                    ask.set_id, latest.version
                ),
            ));
        }
    }
    if let Some(expected) = ask.expected_hash {
        let computed = instructions_hash(instructions);
        if expected != computed {
            return Err(refusal(
                StatisticalErrorCode::DisclosureHashMismatch,
                format!("expected_hash differs from the computed hash {computed}"),
            ));
        }
    }
    let event = next_disclosure_event(
        &DisclosureInput {
            set_id: ask.set_id,
            instructions,
            trigger: ask.trigger,
            now_ms: ctx.now_ms,
            elapsed_s: ask.elapsed_s,
            timeout_s: timeout_s(policy),
        },
        prior.as_ref(),
    )
    .map_err(|refused| event_refusal(refused.code.as_str(), refused.detail))?;
    let mut executed = advisory(vec![format!("{}@v{}", event.set_id, event.version)])?;
    executed.family_premises = vec![event_premise(&event)];
    executed.family = Some(FamilyExecuted {
        answer: Some(FamilyAnswer::DisclosureEvent(Box::new(event))),
        premises: Vec::new(),
        resolution: Some(ResolutionKind::Constraint),
        evidence: None,
    });
    Ok(executed)
}

/// The fact of the Claim premise one sealed event states.
pub(in crate::decide) fn event_premise_fact(event: &DisclosureEvent) -> String {
    format!(
        "disclosure_event:{}@v{} trigger={} at_ms={} hash={}",
        event.set_id,
        event.version,
        event.trigger.as_str(),
        event.at_ms,
        event.instructions_hash
    )
}

/// The Claim premise one sealed event states.
fn event_premise(event: &DisclosureEvent) -> FamilyPremise {
    premise(
        event_premise_fact(event),
        PremiseClass::Claim,
        FamilyPremiseSource::Request("candidates"),
    )
}

/// The versions of `set` the caller's tenant holds in `view`.
fn versions_of(view: &GraphView, tenant: &str, set: &str) -> Vec<InstructionVersion> {
    let prefix = instruction_version_prefix(set);
    view.node_properties
        .keys()
        .filter(|id| id.starts_with(&prefix))
        .filter_map(|id| view.node_row_object(id))
        .filter_map(|props| InstructionVersion::from_node_properties(&props))
        .filter(|version| version.tenant_id == tenant && version.instruction_set_id == set)
        .collect()
}

/// The version of one set the run discloses now, if any.
fn selected(asked: &Asked, versions: &[InstructionVersion]) -> Option<InstructionVersion> {
    versions
        .iter()
        .filter(|version| match asked.timing {
            DisclosureTiming::Initial => version.timing == DisclosureTiming::Initial,
            DisclosureTiming::Runtime => {
                version.timing == DisclosureTiming::Runtime
                    && version
                        .trigger_iri
                        .as_ref()
                        .is_some_and(|trigger| asked.occurred.contains(trigger))
            }
        })
        .max_by_key(|version| version.version)
        .cloned()
}

fn disclose(
    request: &DecideRequest,
    view: Option<&GraphView>,
) -> Result<(Vec<InstructionVersion>, Asked), String> {
    let asked = asked(request)?;
    let view = view.ok_or_else(|| invalid("an instruction_disclosure question reads a graph"))?;
    let mut chosen = Vec::new();
    for set in &asked.sets {
        let versions = versions_of(view, &request.tenant_id, set);
        if versions.is_empty() {
            return Err(refusal(
                StatisticalErrorCode::DisclosureUnauthorized,
                format!("instruction set {set} is not held by this tenant"),
            ));
        }
        let Some(version) = selected(&asked, &versions) else {
            continue;
        };
        if let Some(pin) = asked.pinned.get(set) {
            if pin != &version.digest {
                return Err(refusal(
                    StatisticalErrorCode::DisclosureVersionStale,
                    format!(
                        "instruction set {set} is pinned to {pin} but its latest version is {}",
                        version.digest
                    ),
                ));
            }
        }
        chosen.push(version);
    }
    Ok((chosen, asked))
}

fn premise(fact: String, class: PremiseClass, source: FamilyPremiseSource) -> FamilyPremise {
    FamilyPremise {
        subject: "instruction_disclosure".into(),
        fact,
        class,
        source,
    }
}

fn premises_of(chosen: &[InstructionVersion], asked: &Asked) -> Vec<FamilyPremise> {
    let mut out = vec![
        premise(
            format!("instruction_disclosure.phase={}", asked.timing.as_str()),
            PremiseClass::Claim,
            FamilyPremiseSource::Request("params.phase"),
        ),
        premise(
            format!(
                "instruction_disclosure.run={}@{}",
                asked.run_id, asked.sequence
            ),
            PremiseClass::Claim,
            FamilyPremiseSource::Request("params.run_id"),
        ),
    ];
    let released: BTreeSet<&str> = chosen
        .iter()
        .filter_map(|version| version.trigger_iri.as_deref())
        .collect();
    if released.is_empty() {
        out.push(premise(
            "instruction_disclosure.trigger=none".to_string(),
            PremiseClass::Claim,
            FamilyPremiseSource::Request("params.occurred_triggers"),
        ));
    }
    for trigger in released {
        out.push(premise(
            format!("instruction_disclosure.trigger={trigger}"),
            PremiseClass::Claim,
            FamilyPremiseSource::Request("params.occurred_triggers"),
        ));
    }
    for version in chosen {
        out.push(premise(
            format!(
                "instruction_disclosure.version:{}={}:{}",
                version.instruction_set_id, version.version, version.digest
            ),
            PremiseClass::Observation,
            FamilyPremiseSource::Observation(version.node_id()),
        ));
    }
    out
}

impl DecisionFamily for InstructionDisclosureFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        let event_option = options.len() == 1
            && matches!(
                options[0]
                    .classification
                    .as_slice()
                    .first()
                    .map(String::as_str),
                Some(EVENT_INITIAL | EVENT_REFINEMENT)
            );
        if event_option {
            return Ok(());
        }
        single_option(options, DISCLOSURE_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        if event_mode(request)? {
            asked_event(request).map(|_| ())
        } else {
            asked(request).map(|_| ())
        }
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        match event_mode(request) {
            Ok(true) => return Some(conclude_event(ctx, request, policy)),
            Ok(false) => {}
            Err(error) => return Some(Err(error)),
        }
        Some(disclose(request, graph_view).and_then(|(chosen, asked)| {
            let mut executed = advisory(
                chosen
                    .iter()
                    .map(|version| {
                        format!(
                            "{}:{}:{}",
                            version.instruction_set_id, version.version, version.digest
                        )
                    })
                    .collect(),
            )?;
            executed.family_premises = premises_of(&chosen, &asked);
            Ok(executed)
        }))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }
}
