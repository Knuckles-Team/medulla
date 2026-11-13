//! The `instruction_disclosure` question family (MEDULLA-DECISION-ENGINE-R137.100).
//!
//! Which versions of the named instruction sets a run discloses now. The
//! `initial` phase discloses the latest initial version of each set; the
//! `runtime` phase discloses the latest runtime version whose trigger has
//! occurred, and a runtime version stays withheld until then. The versions are
//! tenant graph nodes read from the graph the `graph` param names, under the
//! caller's row-level filter. `Decide` is evaluate-only and writes nothing.

use std::collections::BTreeSet;

use medulla_decision_types::decision::instruction_disclosure::{
    instruction_version_prefix, InstructionPhase, InstructionVersion,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalErrorCode, TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::{filtered_view, refusal};

use super::support::{
    advisory, invalid, json_param, param, single_option, text_param, unsigned_param,
};
use super::DecisionFamily;

/// The one option an instruction disclosure question declares.
pub const DISCLOSURE_OPTION: &str = "disclosure";

/// The most instruction sets one question names.
const MAX_SETS: usize = 64;

/// The graph the versions are read from.
const PARAM_GRAPH: &str = "graph";

/// The family of the `instruction_disclosure` question kind.
pub struct InstructionDisclosureFamily;

/// What the request asks.
struct Asked {
    phase: InstructionPhase,
    run_id: String,
    sequence: u64,
    occurred: BTreeSet<String>,
    sets: Vec<String>,
    pinned: std::collections::BTreeMap<String, String>,
}

fn asked(request: &DecideRequest) -> Result<Asked, String> {
    let word =
        text_param(request, "phase")?.ok_or_else(|| invalid("parameter phase is required"))?;
    let phase = InstructionPhase::parse(word)
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
    if phase == InstructionPhase::Initial && !occurred.is_empty() {
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
        phase,
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
) -> Result<GraphView, String> {
    let graph = text_param(request, PARAM_GRAPH)?
        .filter(|graph| !graph.trim().is_empty())
        .ok_or_else(|| invalid("an instruction_disclosure question requires a graph param"))?;
    filtered_view(state, agent_id, graph).await
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
        .filter(|version| match asked.phase {
            InstructionPhase::Initial => version.phase == InstructionPhase::Initial,
            InstructionPhase::Runtime => {
                version.phase == InstructionPhase::Runtime
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
        subject: "instruction_disclosure",
        fact,
        class,
        source,
    }
}

fn premises_of(chosen: &[InstructionVersion], asked: &Asked) -> Vec<FamilyPremise> {
    let mut out = vec![
        premise(
            format!("instruction_disclosure.phase={}", asked.phase.as_str()),
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
        single_option(options, DISCLOSURE_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        asked(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
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
