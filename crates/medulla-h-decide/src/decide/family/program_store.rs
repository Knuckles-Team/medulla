//! The `program_lookup` question family (MEDULLA-DECISION-ENGINE-R135.1,
//! R135.3, R135.4, R135.5).
//!
//! Which stored compiled program answers one task signature. The model is
//! `medulla_decision_types::decision::program_store`. The family reads the key
//! `DecisionLog` wrote (`run_record::program_key`) from the caller's tenant
//! and writes nothing. A signature with no stored program abstains with
//! `NO_PROGRAM`.

use medulla_decision_types::decision::program_store::preplan::{pattern_key, ReusablePattern};
use medulla_decision_types::decision::program_store::semantic::{
    bind_request, bind_steps, match_key, Binding, BindingSource, select_match, MatchKind, MatchRefusal, ProgramCandidates,
    ProgramHit, ProgramMatch, ProgramMatchIndex, Selection, MATCH_ANSWER_PREFIX,
    MATCH_SCHEMA_VERSION, MAX_REQUEST_TEXT_BYTES, PARAM_PROGRAM_CANDIDATES, PARAM_PROGRAM_GRAPH,
    PARAM_REQUEST_TEXT, PHRASING_NODE_CLASS,
};
use medulla_decision_types::decision::program_store::replay::{
    replay_prefix, replay_verdict, ProgramDemoted, ReplayKnobs, StoredReplay, MAX_REPLAY_ROWS,
};
use medulla_decision_types::decision::program_store::{
    CompiledProgram, NoProgram, ProgramKnobs,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef, ResolutionKind};
use medulla_types::evidence_class::EvidenceClass;
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::decision_jobs::decode_artifact;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::support::{abstained, advisory, invalid, single_option, text_param};
use super::DecisionFamily;
use crate::decide::program_embedding::{embed_texts, SpacedEmbedder};
use crate::decide::stat_support::resolve_policy;
use crate::decide::stat_vectors::graph_vectors;

/// The one option a program lookup question declares.
pub const PROGRAM_OPTION: &str = "program";
/// The request parameter naming the task signature.
pub const PARAM_TASK_SIGNATURE: &str = "task_signature";
/// The prefix of the answer option that carries the stored program as JSON.
pub const ANSWER_PREFIX: &str = "program:";
/// The skill a program was compiled from (MEDULLA-DECISION-ENGINE-R712.3).
pub const PARAM_SKILL_REF: &str = "skill_ref";
/// The model a program was compiled for (MEDULLA-DECISION-ENGINE-R712.3).
pub const PARAM_MODEL_REF: &str = "model_ref";

/// The family of the `program_lookup` question kind.
pub struct ProgramLookupFamily;

/// The key a program is stored and looked up under. A request that names the pair `skill_ref`
/// and `model_ref` keys the program by that pair as well as by its task signature, so one task
/// compiled by two skills, or for two models, keeps two programs (MEDULLA-DECISION-ENGINE-R712.3).
fn signature(request: &DecideRequest) -> Result<String, String> {
    let task = match text_param(request, PARAM_TASK_SIGNATURE)? {
        Some(text) if !text.trim().is_empty() => text,
        _ => {
            return Err(invalid(
                "a program_lookup question requires a non-empty task_signature",
            ))
        }
    };
    match (
        text_param(request, PARAM_SKILL_REF)?,
        text_param(request, PARAM_MODEL_REF)?,
    ) {
        (None, None) => Ok(task.to_string()),
        (Some(skill), Some(model)) if !skill.trim().is_empty() && !model.trim().is_empty() => {
            Ok(medulla_decision_types::decision::program_store::pair_signature(task, skill, model))
        }
        _ => Err(invalid(
            "skill_ref and model_ref are given together, and neither is empty",
        )),
    }
}

/// The stored program for `task_signature` in the caller's tenant, if any.
fn stored(ctx: &ExecutionContext, task_signature: &str) -> Result<Option<CompiledProgram>, String> {
    let key = medulla_decision_types::decision::program_store::program_key(task_signature);
    let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? else {
        return Ok(None);
    };
    let program: CompiledProgram = decode_artifact(&bytes, "compiled program")?;
    program
        .validate()
        .map_err(|error| invalid(format!("the stored program is unreadable: {error}")))?;
    Ok(Some(program))
}

/// The pattern promoted for `task_signature` in the caller's tenant, if any
/// (MEDULLA-DECISION-ENGINE-R135.7). It is indexed here, beside the program.
fn promoted(
    ctx: &ExecutionContext,
    task_signature: &str,
) -> Result<Option<ReusablePattern>, String> {
    let Some(bytes) = ctx
        .store
        .decision_artifact(ctx.tenant_id, &pattern_key(task_signature))?
    else {
        return Ok(None);
    };
    decode_artifact(&bytes, "reusable pattern").map(Some)
}

fn claim(fact: String, field: &'static str) -> FamilyPremise {
    FamilyPremise {
        subject: "program_lookup".into(),
        fact,
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request(field),
    }
}

/// The match index stored for `task_signature`, if any.
fn stored_index(
    ctx: &ExecutionContext,
    task_signature: &str,
) -> Result<Option<ProgramMatchIndex>, String> {
    let Some(bytes) = ctx
        .store
        .decision_artifact(ctx.tenant_id, &match_key(task_signature))?
    else {
        return Ok(None);
    };
    decode_artifact(&bytes, "program match index").map(Some)
}

/// The demotion of `program` by its recent replays (MEDULLA-DECISION-ENGINE-R135.17.2): the
/// abstention it answers when its newest replays fell back too often, else the two replay
/// counts to state as premises. Rows are read from the caller's tenant under the task signature.
fn demotion(
    ctx: &ExecutionContext,
    policy: &DecisionPolicy,
    task_signature: &str,
    program: &CompiledProgram,
) -> Result<(Option<Executed>, Vec<FamilyPremise>), String> {
    let rows: Vec<StoredReplay> = ctx
        .store
        .decision_artifacts_with_prefix(
            ctx.tenant_id,
            &replay_prefix(task_signature),
            MAX_REPLAY_ROWS,
        )?
        .into_iter()
        .map(|(_, bytes)| decode_artifact(&bytes, "program replay"))
        .collect::<Result<_, _>>()?;
    let verdict = replay_verdict(&rows, &program.program_digest, &ReplayKnobs::from_policy(policy));
    let premises = vec![
        claim(format!("program.replays={}", verdict.replays), PARAM_TASK_SIGNATURE),
        claim(
            format!("program.replay_fallbacks={}", verdict.fallbacks),
            PARAM_TASK_SIGNATURE,
        ),
    ];
    let abstention = verdict.demoted.then(|| {
        abstained(
            QuestionKind::ProgramLookup,
            &ProgramDemoted {
                replays: verdict.replays,
                fallbacks: verdict.fallbacks,
            },
        )
    });
    Ok((abstention, premises))
}

/// The request-text premises of a match answer or abstention (R135.16.5): the match kind,
/// score, threshold and where it came from, the matched node, the embedding calls, the space and
/// one premise per binding.
fn match_claims(
    kind: &str,
    score: Option<i64>,
    threshold: i64,
    source: &str,
    node: Option<&str>,
    candidates: &ProgramCandidates,
    bindings: &[Binding],
) -> Vec<FamilyPremise> {
    let or_none = |value: Option<String>| value.unwrap_or_else(|| "none".to_string());
    let mut claims = vec![
        claim(format!("program.match_kind={kind}"), PARAM_REQUEST_TEXT),
        claim(
            format!("program.match_score_micros={}", or_none(score.map(|n| n.to_string()))),
            PARAM_REQUEST_TEXT,
        ),
        claim(format!("program.match_threshold_micros={threshold}"), PARAM_REQUEST_TEXT),
        claim(format!("program.match_threshold_source={source}"), PARAM_REQUEST_TEXT),
        claim(
            format!("program.matched_node={}", or_none(node.map(str::to_string))),
            PARAM_REQUEST_TEXT,
        ),
        claim(
            format!("program.embedding_calls={}", candidates.embedding_calls),
            PARAM_REQUEST_TEXT,
        ),
        claim(
            format!("program.space_digest={}", or_none(candidates.space_digest.clone())),
            PARAM_REQUEST_TEXT,
        ),
    ];
    for binding in bindings {
        let from = match binding.source {
            BindingSource::Slot => "slot",
            BindingSource::Fixed => "fixed",
        };
        claims.push(claim(
            format!(
                "program.binding.{}={from}:{}",
                binding.parameter,
                serde_json::to_string(&binding.value).unwrap_or_default()
            ),
            PARAM_REQUEST_TEXT,
        ));
    }
    claims
}

/// The semantic answer of a lookup that carries `request_text` (R135.16.3): the exact program,
/// else the nearest candidate; its parameters are bound from the text.
fn semantic(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
    task_signature: &str,
) -> Result<Executed, String> {
    let request_text = text_param(request, PARAM_REQUEST_TEXT)?.unwrap_or_default();
    let candidates: ProgramCandidates = match text_param(request, PARAM_PROGRAM_CANDIDATES)? {
        Some(json) => serde_json::from_str(json)
            .map_err(|error| invalid(format!("program_candidates is unreadable: {error}")))?,
        None => return Err(invalid("a request_text lookup carries recorded candidates")),
    };
    let default_micros = {
        let resolved = resolve_policy(ctx.store, ctx.tenant_id, &request.policy)?;
        ProgramKnobs::from_policy(&resolved.policy).match_threshold_micros
    };
    // An abstention states the same score and threshold premises as an answer.
    let abstain = |refusal: &dyn DecisionRefusal,
                   score: Option<i64>,
                   threshold: i64,
                   source: &str|
     -> Executed {
        let mut executed = abstained(QuestionKind::ProgramLookup, refusal);
        executed.family_premises =
            match_claims("none", score, threshold, source, None, &candidates, &[]);
        executed
    };
    let (chosen, kind, score, threshold, node) = if stored(ctx, task_signature)?.is_some() {
        let own = stored_index(ctx, task_signature)?.and_then(|index| index.threshold_micros);
        (
            task_signature.to_string(),
            MatchKind::Exact,
            None,
            own.unwrap_or(default_micros),
            None,
        )
    } else {
        if let Some(cause) = &candidates.unavailable {
            return Ok(abstain(
                &MatchRefusal::EmbeddingUnavailable {
                    cause: cause.clone(),
                },
                None,
                default_micros,
                "knob",
            ));
        }
        let threshold_of = |signature: &str| -> Option<Option<i64>> {
            stored_index(ctx, signature)
                .ok()
                .flatten()
                .map(|index| index.threshold_micros)
        };
        match select_match(&candidates, threshold_of, default_micros) {
            Selection::Selected {
                task_signature,
                node_id,
                score_micros,
                threshold_micros,
            } => (
                task_signature,
                MatchKind::Semantic,
                Some(score_micros),
                threshold_micros,
                Some(node_id),
            ),
            Selection::BelowThreshold {
                score_micros,
                threshold_micros,
            } => {
                return Ok(abstain(
                    &MatchRefusal::BelowThreshold {
                        score_micros,
                        threshold_micros,
                    },
                    Some(score_micros),
                    threshold_micros,
                    "knob",
                ))
            }
            Selection::NoProgram => {
                return Ok(abstain(
                    &NoProgram {
                        task_signature: task_signature.to_string(),
                    },
                    None,
                    default_micros,
                    "knob",
                ))
            }
        }
    };
    let index = stored_index(ctx, &chosen)?;
    let source = if index.as_ref().is_some_and(|i| i.threshold_micros.is_some()) {
        "program"
    } else {
        "knob"
    };
    let Some(program) = stored(ctx, &chosen)? else {
        return Ok(abstain(
            &NoProgram {
                task_signature: chosen,
            },
            score,
            threshold,
            source,
        ));
    };
    let (demoted, replay_premises) = demotion(ctx, policy, &chosen, &program)?;
    if let Some(mut executed) = demoted {
        executed.family_premises = match_claims("none", score, threshold, source, None, &candidates, &[]);
        executed.family_premises.extend(replay_premises);
        return Ok(executed);
    }
    // A program with no index matches by exact signature only: it has no slots to bind.
    let (bindings, steps) = match index {
        None => (Vec::new(), Vec::new()),
        Some(index) => {
            let bindings = match bind_request(&index, request_text) {
                Ok(bindings) => bindings,
                Err(unbound) => {
                    return Ok(abstain(
                        &MatchRefusal::SlotsUnbound {
                            parameters: unbound.parameters,
                        },
                        score,
                        threshold,
                        source,
                    ))
                }
            };
            if !index.executable {
                return Ok(abstain(
                    &MatchRefusal::NotExecutable {
                        task_signature: chosen,
                    },
                    score,
                    threshold,
                    source,
                ));
            }
            let steps = bind_steps(&index, &bindings);
            (bindings, steps)
        }
    };
    let kind_text = match kind {
        MatchKind::Exact => "exact",
        MatchKind::Semantic => "semantic",
    };
    let matched = ProgramMatch {
        schema_version: MATCH_SCHEMA_VERSION,
        task_signature: chosen.clone(),
        program_digest: program.program_digest.clone(),
        match_kind: kind,
        score_micros: score,
        threshold_micros: threshold,
        matched_node: node.clone(),
        bindings: BoundedVec::new(bindings.clone()).map_err(invalid)?,
        steps: BoundedVec::new(steps).map_err(invalid)?,
    };
    let body = serde_json::to_string(&matched)
        .map_err(|error| invalid(format!("program match encoding failed: {error}")))?;
    let mut executed = advisory(vec![format!("{MATCH_ANSWER_PREFIX}{body}")])?;
    executed.family_premises = vec![claim(
        format!("program.task_signature={chosen}"),
        PARAM_TASK_SIGNATURE,
    )];
    executed.family_premises.extend(match_claims(
        kind_text,
        score,
        threshold,
        source,
        node.as_deref(),
        &candidates,
        &bindings,
    ));
    executed.family_premises.extend(replay_premises);
    Ok(executed)
}

/// Record the nearest program candidates of `request_text` in the request's own parameter
/// (MEDULLA-DECISION-ENGINE-R135.16.3). A request without `request_text` passes unchanged.
pub(in crate::decide) async fn with_program_candidates(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    verified: &VerifiedRequestContext,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    let Some(request_text) = text_param(&request, PARAM_REQUEST_TEXT)?.map(str::to_string) else {
        return Ok(request);
    };
    if request
        .params
        .as_slice()
        .iter()
        .any(|param| param.name == PARAM_PROGRAM_CANDIDATES)
    {
        return Err(invalid(
            "PROGRAM_CANDIDATES_RESERVED: program_candidates is recorded by the engine",
        ));
    }
    if request_text.len() > MAX_REQUEST_TEXT_BYTES {
        return Err(invalid(format!(
            "REQUEST_TEXT_TOO_LONG: request_text is over {MAX_REQUEST_TEXT_BYTES} bytes"
        )));
    }
    let graph = text_param(&request, PARAM_PROGRAM_GRAPH)?
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| invalid("PROGRAM_GRAPH_REQUIRED: request_text needs program_graph"))?;
    let task_signature = signature(&request)?;
    let exact = store
        .decision_artifact(
            &request.tenant_id,
            &medulla_decision_types::decision::program_store::program_key(&task_signature),
        )?
        .is_some();
    let mut candidates = ProgramCandidates {
        space_digest: None,
        embedding_calls: 0,
        hits: BoundedVec::default(),
        unavailable: None,
    };
    if !exact {
        let resolved = resolve_policy(store, &request.tenant_id, &request.policy)?;
        let k = ProgramKnobs::from_policy(&resolved.policy).match_candidates;
        let view = graph_vectors(state, verified.agent_id(), &graph).await?;
        let embedder = SpacedEmbedder::configured()?;
        candidates.space_digest = Some(embedder.space.digest.clone());
        if !view.admits_space(&embedder.space.digest) {
            candidates.unavailable =
                Some("PROGRAM_EMBEDDING_SPACE_MISMATCH: the graph holds another space".to_string());
        } else {
            let tenant = request.tenant_id.clone();
            let policy_digest = resolved.digest.clone();
            let embedded = tokio::task::spawn_blocking(move || {
                embed_texts(&embedder, &tenant, &policy_digest, &[request_text.as_str()])
            })
            .await
            .map_err(|error| invalid(format!("embedding task failed: {error}")))?;
            match embedded {
                Err(cause) => candidates.unavailable = Some(cause),
                Ok((mut vectors, calls)) => {
                    candidates.embedding_calls = u32::try_from(calls).unwrap_or(u32::MAX);
                    let query = vectors.remove(0);
                    let hits: Vec<ProgramHit> = view
                        .probe_filtered(&query, k, |node| {
                            matches!(
                                node.get("type").and_then(|t| t.as_str()),
                                Some(PROGRAM_CLASS) | Some(PHRASING_NODE_CLASS)
                            )
                        })
                        .into_iter()
                        .filter_map(|(node_id, cosine)| {
                            let node = view.visible_properties(&node_id)?;
                            Some(ProgramHit {
                                task_signature: node.get("task_signature")?.as_str()?.to_string(),
                                node_id,
                                score_micros: (f64::from(cosine) * 1_000_000.0).round() as i64,
                            })
                        })
                        .collect();
                    candidates.hits = BoundedVec::new(hits).map_err(invalid)?;
                }
            }
        }
    }
    let mut params = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: PARAM_PROGRAM_CANDIDATES.to_string(),
        value: TypedValue::Text(serde_json::to_string(&candidates).map_err(invalid)?),
    });
    request.params = BoundedVec::new(params).map_err(invalid)?;
    Ok(request)
}

/// The class of a stored program node.
const PROGRAM_CLASS: &str = medulla_decision_types::decision::program_store::PROGRAM_NODE_CLASS;

impl DecisionFamily for ProgramLookupFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, PROGRAM_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        signature(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let task_signature = signature(request)?;
            let task_signature = task_signature.as_str();
            if text_param(request, PARAM_REQUEST_TEXT)?.is_some() {
                return semantic(ctx, request, policy, task_signature);
            }
            let mut executed = match stored(ctx, task_signature)? {
                None => abstained(
                    QuestionKind::ProgramLookup,
                    &NoProgram {
                        task_signature: task_signature.to_string(),
                    },
                ),
                Some(program) => {
                    let (demoted, replay_premises) =
                        demotion(ctx, policy, task_signature, &program)?;
                    if let Some(mut executed) = demoted {
                        executed.family_premises = replay_premises;
                        executed.family_premises.insert(
                            0,
                            claim(
                                format!("program.task_signature={task_signature}"),
                                PARAM_TASK_SIGNATURE,
                            ),
                        );
                        return Ok(executed);
                    }
                    let body = serde_json::to_string(&program)
                        .map_err(|error| invalid(format!("program encoding failed: {error}")))?;
                    let mut executed = advisory(vec![format!("{ANSWER_PREFIX}{body}")])?;
                    executed.family_premises = vec![
                        claim(
                            format!("program.step_count={}", program.steps.len()),
                            PARAM_TASK_SIGNATURE,
                        ),
                        claim(
                            format!("program.parameter_count={}", program.parameters.len()),
                            PARAM_TASK_SIGNATURE,
                        ),
                        claim(
                            format!(
                                "program.source_run_id={}",
                                program.source_run_id.as_deref().unwrap_or_default()
                            ),
                            PARAM_TASK_SIGNATURE,
                        ),
                    ];
                    if let Some(pattern) = promoted(ctx, task_signature)? {
                        for fact in [
                            "program.reusable=true".to_string(),
                            format!(
                                "program.preplan_command_digest={}",
                                pattern.receipt.command_digest
                            ),
                            format!(
                                "program.preplan_result_digest={}",
                                pattern.receipt.result_digest
                            ),
                            format!(
                                "program.preplan_manifest_digest={}",
                                pattern.receipt.artifact_manifest_digest
                            ),
                        ] {
                            executed
                                .family_premises
                                .push(claim(fact, PARAM_TASK_SIGNATURE));
                        }
                    }
                    executed.family_premises.extend(replay_premises);
                    executed
                }
            };
            executed.family_premises.insert(
                0,
                claim(
                    format!("program.task_signature={task_signature}"),
                    PARAM_TASK_SIGNATURE,
                ),
            );
            Ok(executed)
        })())
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

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Constraint)
    }

    /// A semantic match scores by a model embedding, so it is a `Claim`; an exact answer, an
    /// abstention and a plain lookup stay `Observation` (R135.16.5).
    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        let semantic = match outcome {
            StatisticalOutcome::Advisory { scores, .. } => scores
                .as_slice()
                .first()
                .and_then(|score| score.option_id.strip_prefix(MATCH_ANSWER_PREFIX))
                .and_then(|body| serde_json::from_str::<ProgramMatch>(body).ok())
                .is_some_and(|matched| matched.match_kind == MatchKind::Semantic),
            _ => false,
        };
        Some(if semantic {
            EvidenceClass::Claim
        } else {
            EvidenceClass::Observation
        })
    }
}
