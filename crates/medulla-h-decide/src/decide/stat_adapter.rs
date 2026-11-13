//! `DecisionLog.retrieval` adapter operations (EH-396): fit and evaluate a
//! query-side adapter, activate it, roll it back, read its state.
//!
//! A fit reads ONLY independently judged successful runs whose query was
//! ranked in the requested space, scores their cited units and hard negatives
//! by the graph's stored vectors -- through the caller's graph ACL and
//! row-level security -- and holds a deterministic share of them out. The
//! receipt is a paired sign test on that held-out share, run on the
//! QUANTISED body, i.e. exactly what would be served. Activation requires the
//! passing receipt that qualified exactly this body; every pointer move is an
//! audited event, and rollback returns to the adapter active before.

use medulla_decision_stats::decision::adapter::{
    fit_scoped, is_held_out, monitor_adapter, wilson_upper, MonitorVerdict, ScopedFitError,
    ScopedFitParams, ScopedItem,
};
use medulla_decision_stats::{
    decision::adapter::directions_of, decision::adapter::evaluate, decision::adapter::fit,
    decision::adapter::to_body, decision::adapter::to_q16, decision::adapter::JudgedItem,
};
use medulla_decision_stats::{decision::retrieval::hard_negatives, decision::retrieval::Verdict};
use medulla_decision_types::decision::statistical::retrieval::RetrievalOutcome;
use medulla_decision_types::decision::statistical::retrieval_pointer::PointerState;
use medulla_decision_types::decision::statistical::run_credit::RunCreditReceipt;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::{
    decision::statistical::body::canonical_body_bytes,
    decision::statistical::body::content_digest_of,
};
use medulla_decision_types::{
    decision::statistical::retrieval_adapter::AdapterEvalReceipt,
    decision::statistical::retrieval_adapter::AdapterFitRequest,
    decision::statistical::retrieval_adapter::AdapterFitted,
    decision::statistical::retrieval_adapter::QueryAdapterBody,
    decision::statistical::retrieval_adapter::MAX_ADAPTER_GAIN_Q16,
    decision::statistical::retrieval_adapter::MAX_ADAPTER_RANK,
    decision::statistical::retrieval_adapter::QUERY_ADAPTER_SCHEMA_VERSION,
    decision::statistical::retrieval_adapter::{
        AdapterMonitorRequest, AdapterMonitored, AdapterScope, AdapterSide, MonitorState,
        OutcomeSource, SourceCount,
    },
};
use medulla_query_types::solve_scalar::digest_text;
use std::collections::{BTreeMap, BTreeSet};

use super::served_adapter::{adapter_key, read_fitted};
use super::stat_executor::ExecutionContext;
use super::stat_log::LogReader;
use super::stat_pointer::{read_pointer, rollback, PointerMove, Qualified};
use super::stat_retrieval::{judged_outcomes, Scope};
use super::stat_support::{invalid_parameter as invalid, refusal};
use super::stat_vectors::GraphVectors;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};

const TRAINING_DOMAIN: &str = "eg/adapter-training/v1";
const HOLDOUT_DOMAIN: &str = "eg/adapter-holdout/v1";
const Q16: f64 = 65_536.0;

fn check_fit_request(request: &AdapterFitRequest) -> Result<(), String> {
    let rank_ok = (1..=MAX_ADAPTER_RANK).contains(&usize::from(request.rank));
    let gain_ok = (1..=MAX_ADAPTER_GAIN_Q16).contains(&request.max_gain_q16);
    let holdout_ok = (100..=500).contains(&request.holdout_per_mille);
    if !(rank_ok && gain_ok && holdout_ok && request.min_eval_items > 0) {
        return Err(invalid(
            "an adapter fit needs rank 1..=8, 0 < max gain <= 1/2, a 100..=500 per-mille \
             holdout and at least one evaluation item",
        ));
    }
    if request.sources.is_empty() {
        return Err(invalid("an adapter fit names at least one outcome source"));
    }
    if request.side == AdapterSide::Index {
        return Err(invalid(
            "an index-side adapter is materialized by the generation cycle \
             (MEDULLA-DECISION-ENGINE-R174.7.3.2), not fitted here",
        ));
    }
    if request.sources.contains(&OutcomeSource::CapabilityOutcome) {
        return Err(invalid(
            "capability outcomes train an adapter once MEDULLA-DECISION-ENGINE-R172.5 records them",
        ));
    }
    Ok(())
}

fn query_of(outcome: &RetrievalOutcome, space: &str) -> Option<Vec<f64>> {
    let query = outcome.query.as_ref()?;
    (query.space_digest == space).then(|| query.q16.iter().map(|v| f64::from(*v) / Q16).collect())
}

/// The judged item of one successful run, when both sides have vectors.
fn judged_item(
    outcome: &RetrievalOutcome,
    space: &str,
    vectors: &GraphVectors,
) -> Option<JudgedItem> {
    let item = JudgedItem {
        query: query_of(outcome, space)?,
        positives: vectors.vectors(outcome.cited.iter().cloned()),
        negatives: vectors.vectors(hard_negatives(outcome).into_iter().map(|n| n.evidence_id)),
    };
    (!item.positives.is_empty() && !item.negatives.is_empty()).then_some(item)
}

fn held_out(record_id: &str, per_mille: u16) -> bool {
    let digest = digest_text(HOLDOUT_DOMAIN, &record_id);
    let hex = digest.trim_start_matches("sha256:");
    let bucket = u32::from_str_radix(hex.get(..4).unwrap_or("0"), 16).unwrap_or(0) % 1000;
    bucket < u32::from(per_mille)
}

/// `(training ids, training items, held-out items)` in log key order.
type Split = (Vec<String>, Vec<JudgedItem>, Vec<JudgedItem>);

fn split_items(
    store: &AgentLibraryStore,
    reader: &LogReader,
    request: &AdapterFitRequest,
    vectors: &GraphVectors,
) -> Result<Split, String> {
    let scope = Scope {
        question_id: request.question_id.as_deref(),
        window: request.window,
    };
    let mut split: Split = (Vec::new(), Vec::new(), Vec::new());
    for judged in judged_outcomes(store, reader, scope)? {
        if judged.verdict != Verdict::Success {
            continue;
        }
        let outcome = &judged.stored.outcome;
        let Some(item) = judged_item(outcome, &request.space_digest, vectors) else {
            continue;
        };
        if held_out(&outcome.record_id, request.holdout_per_mille) {
            split.2.push(item);
        } else {
            split.0.push(outcome.record_id.clone());
            split.1.push(item);
        }
    }
    Ok(split)
}

/// What a receipt records beyond the fit's evaluation.
struct Provenance {
    sources: Vec<SourceCount>,
    training_digest: String,
    revision: u32,
    parent_digest: Option<String>,
}

fn receipt_of(
    adapter_digest: &str,
    request: &AdapterFitRequest,
    n_training: u64,
    held: &[JudgedItem],
    body: &QueryAdapterBody,
    provenance: Provenance,
) -> AdapterEvalReceipt {
    let eval = evaluate(held, &directions_of(body));
    let lower = to_q16(eval.win_rate_lower);
    AdapterEvalReceipt {
        schema_version: QUERY_ADAPTER_SCHEMA_VERSION,
        adapter_digest: adapter_digest.to_string(),
        graph: request.graph.clone(),
        space_digest: request.space_digest.clone(),
        n_training,
        n_eval: eval.n,
        wins: eval.wins,
        losses: eval.losses,
        ties: eval.ties,
        base_mrr_q16: to_q16(eval.base_mrr),
        adapted_mrr_q16: to_q16(eval.adapted_mrr),
        win_rate_lower_q16: lower,
        win_rate_upper_q16: to_q16(wilson_upper(eval.wins, eval.wins + eval.losses)),
        scope: request.scope.clone(),
        side: request.side,
        sources: provenance.sources,
        training_digest: provenance.training_digest,
        revision: provenance.revision,
        parent_digest: provenance.parent_digest,
        passed: eval.n >= u64::from(request.min_eval_items) && lower > to_q16(0.5),
    }
}

/// Most run receipts one fit reads.
const MAX_RUN_RECEIPTS: usize = 100_000;

/// Whether a judged retrieval belongs to `scope`: the graph scope covers every
/// retrieval, a corpus scope the retrievals whose plan anchored on that corpus
/// class, a capability scope those made for that task class.
fn in_scope(outcome: &RetrievalOutcome, scope: &AdapterScope) -> bool {
    let path = outcome.path.as_ref();
    match scope {
        AdapterScope::Graph => true,
        AdapterScope::Corpus { corpus_iri } => path.is_some_and(|p| &p.anchor_class == corpus_iri),
        AdapterScope::Capability { capability_iri } => {
            path.is_some_and(|p| &p.task_class == capability_iri)
        }
    }
}

/// The decisions a stored run receipt credits with at least one positive.
fn credited_decisions(store: &AgentLibraryStore, tenant: &str) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for (_, bytes) in
        store.decision_artifacts_with_prefix(tenant, "run_credit/run/", MAX_RUN_RECEIPTS)?
    {
        let receipt: RunCreditReceipt = decode_artifact(&bytes, "run credit receipt")?;
        if receipt.positive_count > 0 {
            out.extend(receipt.decision_ids.iter().cloned());
        }
    }
    Ok(out)
}

/// The adapter active in `scope` of `graph`, and the revision a refit of it
/// takes.
fn parent_of(
    store: &AgentLibraryStore,
    tenant: &str,
    graph: &str,
    scope: &AdapterScope,
) -> Result<(Option<String>, u32), String> {
    let key = scoped_adapter_pointer(graph, scope);
    let (_, pointer) = read_pointer(store, tenant, &key)?;
    let Some(digest) = pointer.target() else {
        return Ok((None, 1));
    };
    let revision = read_fitted(store, tenant, digest)?
        .map_or(1, |parent| parent.body.revision.saturating_add(1));
    Ok((Some(digest.to_string()), revision))
}

/// The scoped fit: the judged retrievals of the requested sources inside the
/// requested scope, fitted with [`fit_scoped`] under the registry knobs.
fn scoped_split(
    ctx: &ExecutionContext,
    reader: &LogReader,
    request: &AdapterFitRequest,
    vectors: &GraphVectors,
) -> Result<(Vec<ScopedItem>, Vec<SourceCount>), String> {
    let scope = Scope {
        question_id: request.question_id.as_deref(),
        window: request.window,
    };
    let judged = judged_outcomes(ctx.store, reader, scope)?;
    if request.scope != AdapterScope::Graph
        && !judged
            .iter()
            .any(|j| in_scope(&j.stored.outcome, &request.scope))
    {
        return Err(refusal(
            StatisticalErrorCode::AdapterScopeUnknown,
            format!(
                "no judged retrieval belongs to scope {}",
                request.scope.key()
            ),
        ));
    }
    let from_retrieval = request.sources.contains(&OutcomeSource::RetrievalOutcome);
    let runs = if request.sources.contains(&OutcomeSource::RunOutcome) {
        credited_decisions(ctx.store, &reader.tenant_id)?
    } else {
        BTreeSet::new()
    };
    let mut counts: BTreeMap<OutcomeSource, u64> = BTreeMap::new();
    let mut items = Vec::new();
    for judged in judged.iter().filter(|j| j.verdict == Verdict::Success) {
        let outcome = &judged.stored.outcome;
        let from_run = runs.contains(&outcome.record_id);
        if !in_scope(outcome, &request.scope) || !(from_retrieval || from_run) {
            continue;
        }
        let Some(item) = judged_item(outcome, &request.space_digest, vectors) else {
            continue;
        };
        if from_retrieval {
            *counts.entry(OutcomeSource::RetrievalOutcome).or_default() += 1;
        }
        if from_run {
            *counts.entry(OutcomeSource::RunOutcome).or_default() += 1;
        }
        items.push(ScopedItem {
            decision_id: outcome.record_id.clone(),
            item,
            weight: 1,
        });
    }
    let sources = request
        .sources
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|source| SourceCount {
            source: *source,
            count: counts.get(source).copied().unwrap_or(0),
        })
        .collect();
    Ok((items, sources))
}

fn scoped_refusal(error: ScopedFitError) -> String {
    match error {
        ScopedFitError::InsufficientOutcomes { held_out, needed } => refusal(
            StatisticalErrorCode::AdapterInsufficientOutcomes,
            format!("{held_out} held-out outcomes, {needed} needed"),
        ),
        ScopedFitError::MemoryBudget { bytes, max } => refusal(
            StatisticalErrorCode::AdapterMemoryBudget,
            format!("{bytes} resident bytes exceed adapter.max_resident_bytes {max}"),
        ),
        ScopedFitError::NothingToLearn => refusal(
            StatisticalErrorCode::NoAdmissibleLabels,
            "no judged run in this scope has both a cited unit and a hard negative to learn from",
        ),
    }
}

/// Whether the request is the original graph-scope retrieval-outcome fit.
fn is_graph_retrieval_fit(request: &AdapterFitRequest) -> bool {
    request.scope == AdapterScope::Graph && request.sources == [OutcomeSource::RetrievalOutcome]
}

/// Fit, evaluate and store one adapter under the served tenant `tenant`.
pub(super) fn fit_adapter(
    ctx: &ExecutionContext,
    reader: &LogReader,
    request: &AdapterFitRequest,
    vectors: &GraphVectors,
    tenant: &str,
) -> Result<AdapterFitted, String> {
    check_fit_request(request)?;
    if !vectors.admits_space(&request.space_digest) {
        return Err(refusal(
            StatisticalErrorCode::AdapterSpaceMismatch,
            "the graph's store declares another embedding space",
        ));
    }
    let dim = vectors.dimensions();
    let max_gain = f64::from(request.max_gain_q16) / Q16;
    let (parent_digest, revision) = parent_of(ctx.store, tenant, &request.graph, &request.scope)?;
    let knobs = medulla_server_api::knob_config::knobs();
    let (ids, directions, held, sources) = if is_graph_retrieval_fit(request) {
        let (ids, train, held) = split_items(ctx.store, reader, request, vectors)?;
        let directions = fit(&train, dim, usize::from(request.rank), max_gain);
        let count = (ids.len() + held.len()) as u64;
        let sources = vec![SourceCount {
            source: OutcomeSource::RetrievalOutcome,
            count,
        }];
        (ids, directions, held, sources)
    } else {
        let (items, sources) = scoped_split(ctx, reader, request, vectors)?;
        let params = ScopedFitParams {
            dim,
            rank: usize::from(request.rank).min(knobs.adapter_rank as usize),
            max_gain,
            holdout_pct: u8::try_from(request.holdout_per_mille / 10).unwrap_or(50),
            min_eval_items: request.min_eval_items,
            max_resident_bytes: knobs.adapter_max_resident_bytes,
        };
        let fitted = fit_scoped(&items, &params).map_err(scoped_refusal)?;
        let held = held_items(&items, params.holdout_pct);
        (fitted.training_ids, fitted.directions, held, sources)
    };
    if directions.is_empty() {
        return Err(refusal(
            StatisticalErrorCode::NoAdmissibleLabels,
            "no judged run in this space has both a cited unit and a hard negative to learn from",
        ));
    }
    let training_digest = digest_text(TRAINING_DOMAIN, &ids);
    let training = (training_digest.clone(), ids.len() as u64);
    let mut body = to_body(&directions, &request.space_digest, dim, training)?;
    body.revision = revision;
    body.parent_digest = parent_digest.clone();
    let adapter_digest = content_digest_of(&canonical_body_bytes(&body)?);
    let provenance = Provenance {
        sources,
        training_digest,
        revision,
        parent_digest,
    };
    let receipt = receipt_of(
        &adapter_digest,
        request,
        ids.len() as u64,
        &held,
        &body,
        provenance,
    );
    let fitted = AdapterFitted {
        receipt_digest: content_digest_of(&canonical_body_bytes(&receipt)?),
        adapter_digest,
        body,
        receipt,
    };
    let key = adapter_key(&fitted.adapter_digest);
    ctx.store
        .put_decision_artifacts(tenant, &[(key, encode_artifact(&fitted)?)])?;
    Ok(fitted)
}

/// The held-out items of a scoped fit, by the same decision-id split
/// [`fit_scoped`] used.
fn held_items(items: &[ScopedItem], holdout_pct: u8) -> Vec<JudgedItem> {
    items
        .iter()
        .filter(|s| s.weight > 0 && is_held_out(&s.decision_id, holdout_pct))
        .map(|s| s.item.clone())
        .collect()
}

/// The pointer key of a graph's adapter.
pub(super) fn adapter_pointer(graph: &str) -> String {
    format!("adapter:{graph}")
}

/// The pointer key of one scope of a graph's adapter. The graph scope is the
/// graph's own pointer.
pub(super) fn scoped_adapter_pointer(graph: &str, scope: &AdapterScope) -> String {
    match scope {
        AdapterScope::Graph => adapter_pointer(graph),
        other => format!("adapter:{graph}:{}", other.key()),
    }
}

/// A fitted adapter whose passing receipt is the named one, fitted on `graph`.
pub(super) fn qualified(
    store: &AgentLibraryStore,
    tenant: &str,
    (graph, scope): (&str, &AdapterScope),
    adapter_digest: &str,
    receipt_digest: &str,
) -> Result<Qualified, String> {
    let fitted = read_fitted(store, tenant, adapter_digest)?
        .ok_or_else(|| invalid("no fitted adapter with that digest"))?;
    if fitted.receipt_digest != receipt_digest || !fitted.receipt.passed {
        return Err(refusal(
            StatisticalErrorCode::EvaluationReceiptMismatch,
            "the adapter's receipt is not the named one, or did not pass",
        ));
    }
    if fitted.receipt.graph != graph {
        return Err(invalid("the adapter was fitted on another graph"));
    }
    if &fitted.receipt.scope != scope {
        return Err(invalid("the adapter was fitted for another scope"));
    }
    Ok(Qualified {
        target: fitted.adapter_digest,
        receipt_digest: fitted.receipt_digest,
    })
}

/// Most adapter pointers one tenant's activation check reads.
const MAX_ADAPTER_POINTERS: usize = 4_096;

/// Refuse an activation that would leave more active adapters than
/// `adapter.max_per_tenant`. Re-activating a pointer already active does not
/// add one.
pub(super) fn ensure_room(
    store: &AgentLibraryStore,
    tenant: &str,
    pointer_key: &str,
) -> Result<(), String> {
    let mut active: u64 = 0;
    for (_, bytes) in
        store.decision_artifacts_with_prefix(tenant, "pointer:adapter:", MAX_ADAPTER_POINTERS)?
    {
        let state: PointerState = decode_artifact(&bytes, "pointer state")?;
        if state.active.is_some() && state.key != pointer_key {
            active += 1;
        }
    }
    let max = medulla_server_api::knob_config::knobs().adapter_max_per_tenant;
    if active >= max {
        return Err(refusal(
            StatisticalErrorCode::AdapterTooMany,
            format!("{active} adapters are active; adapter.max_per_tenant is {max}"),
        ));
    }
    Ok(())
}

/// Run the post-activation monitor of one scope's active adapter over the
/// outcomes judged in the request's window, and move the pointer back when it
/// hurts. The state it answers with is the only record of the verdict, written
/// beside the adapter; a rollback receipt holds the window and the bound that
/// decided it.
pub(super) fn monitor(
    ctx: &ExecutionContext,
    reader: &LogReader,
    request: &AdapterMonitorRequest,
    vectors: &GraphVectors,
    tenant: &str,
) -> Result<AdapterMonitored, String> {
    let key = scoped_adapter_pointer(&request.graph, &request.scope);
    let (_, pointer) = read_pointer(ctx.store, tenant, &key)?;
    let digest = pointer
        .target()
        .ok_or_else(|| invalid("no adapter is active in this scope"))?
        .to_string();
    let fitted = read_fitted(ctx.store, tenant, &digest)?
        .ok_or_else(|| invalid("the active adapter has no stored body"))?;
    let scope = Scope {
        question_id: None,
        window: request.window,
    };
    let items: Vec<JudgedItem> = judged_outcomes(ctx.store, reader, scope)?
        .iter()
        .filter(|j| j.verdict == Verdict::Success && in_scope(&j.stored.outcome, &request.scope))
        .filter_map(|j| judged_item(&j.stored.outcome, &fitted.body.space_digest, vectors))
        .collect();
    let directions = directions_of(&fitted.body);
    let min_outcomes = medulla_server_api::knob_config::knobs().adapter_monitor_min_outcomes;
    let verdict = monitor_adapter(&items, &directions, min_outcomes);
    let evaluation = evaluate(&items, &directions);
    let state = match verdict {
        MonitorVerdict::Insufficient { .. } => MonitorState::Watching,
        MonitorVerdict::Healthy { .. } => MonitorState::Healthy,
        MonitorVerdict::Rollback { .. } => MonitorState::RolledBack,
    };
    let monitored = AdapterMonitored {
        adapter_digest: digest.clone(),
        scope: request.scope.clone(),
        state,
        window: request.window,
        outcomes: evaluation.n,
        wins: evaluation.wins,
        losses: evaluation.losses,
        win_rate_upper_q16: to_q16(wilson_upper(
            evaluation.wins,
            evaluation.wins + evaluation.losses,
        )),
        min_outcomes,
    };
    if state == MonitorState::RolledBack {
        let at = PointerMove {
            tenant,
            key,
            principal: &reader.principal,
            now_ms: ctx.now_ms,
        };
        rollback(ctx.store, &at)?;
        let receipt_key = format!("adapter-monitor:{digest}:{}", ctx.now_ms);
        ctx.store
            .put_decision_artifacts(tenant, &[(receipt_key, encode_artifact(&monitored)?)])?;
    }
    Ok(monitored)
}
