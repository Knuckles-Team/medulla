//! EH-525: the learned `reputation` relation and the `SOURCE RELIABILITY` stage that
//! reads it. Only independent outcomes of records the caller may read count; a named
//! evaluator's grant (EH-395) lets it evaluate, never read; below `min_support` a row
//! carries no numbers.

use super::retrieval_tests::{
    clone_executed, clone_record, evaluate_by, executed_template, grant, principal_of, sql,
};
use super::*;
use serde_json::{json, Value};

/// `n` executed `plan-hyde` records, `successes` of them independently judged good.
async fn labelled(h: &Harness, n: usize, successes: usize) {
    let template = executed_template(h).await;
    for i in 0..n {
        let id = format!("rec-{i}");
        clone_executed(h, &template, &id);
        evaluate_by(h, "evaluator", &id, i < successes).await;
    }
    // A self-report by the decider must never count.
    clone_record(h, &template, "rec-self");
    evaluate_by(h, "decider", "rec-self", false).await;
}

const OPTION_ROW: &str =
    "SELECT subject_kind, status, trials, successes FROM reputation WHERE subject = 'plan-hyde'";

#[tokio::test]
async fn reputations_count_only_independent_outcomes_the_caller_may_read() {
    let h = Harness::new().await;
    labelled(&h, 12, 9).await;
    assert_eq!(
        sql(&h, "decider", OPTION_ROW).await,
        vec![vec![
            json!("option"),
            json!("estimated"),
            json!(12),
            json!(9)
        ]],
        "twelve independent labels; the self-report is not one"
    );
    let agent = "SELECT trials FROM reputation WHERE subject_kind = 'agent' AND subject = 'retriever-agent'";
    assert_eq!(sql(&h, "decider", agent).await, vec![vec![json!(12)]]);
    let bounds = sql(
        &h,
        "decider",
        "SELECT lower, mean, upper FROM reputation WHERE subject = 'plan-hyde'",
    )
    .await;
    let [lower, mean, upper] = [0, 1, 2].map(|i| bounds[0][i].as_f64().unwrap());
    assert!(lower < mean && mean < upper && mean > 0.5, "{bounds:?}");
    for who in ["evaluator", "stranger"] {
        let count = sql(&h, who, "SELECT count(*) FROM reputation").await;
        assert_eq!(
            count,
            vec![vec![json!(0)]],
            "{who} reads no record, so no reputation"
        );
    }
}

#[tokio::test]
async fn below_the_support_floor_a_reputation_has_no_numbers() {
    let h = Harness::new().await;
    labelled(&h, 3, 3).await;
    assert_eq!(
        sql(&h, "decider", OPTION_ROW).await,
        vec![vec![
            json!("option"),
            json!("insufficient_history"),
            Value::Null,
            Value::Null
        ]]
    );
}

/// `SOURCE RELIABILITY 'plan-hyde'` over one row, as `who` runs it: (belief, lo, hi).
#[cfg(feature = "epistemic")]
fn reliability(h: &Harness, who: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    use medulla_query_types::wire::UqlResult;
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, TENANT);
    let authority =
        medulla_server_api::carrier::CarrierAuthority::from_verified(&verified).unwrap();
    let views = super::super::stat_view::DecisionViews::of(h.store.clone(), &authority);
    let core = medulla_graph_core::graph::GraphCore::new();
    core.add_node(
        "d1".into(),
        rmp_serde::to_vec_named(&json!({"type": "Doc"})).unwrap(),
    );
    let graph = core.analysis_snapshot();
    let semantic = medulla_semantic_index::semantic::SemanticStore::new();
    let ctx = medulla_exec::exec::PlanCtx::new(&graph, &semantic).with_decisions(&views);
    let text = "MATCH () |> SOURCE RELIABILITY 'plan-hyde' |> RETURN belief, reliability_lo, reliability_hi";
    let stmt = medulla_uql::parse_statement(text, &medulla_uql::Params::new()).unwrap();
    let UqlResult::Rows { rows, .. } =
        medulla_plan::uql::serve::run_statement(&stmt, &ctx).unwrap()
    else {
        panic!("rows")
    };
    (
        rows[0].channels[0],
        rows[0].channels[1],
        rows[0].channels[2],
    )
}

#[cfg(feature = "epistemic")]
#[tokio::test]
async fn source_reliability_is_learned_from_the_visible_log() {
    let h = Harness::new().await;
    labelled(&h, 12, 9).await;
    let (learned, lo, hi) = reliability(&h, "decider");
    let (learned, lo, hi) = (learned.unwrap(), lo.unwrap(), hi.unwrap());
    assert!(lo < learned && learned < hi, "{lo} < {learned} < {hi}");
    let (prior, none_lo, none_hi) = reliability(&h, "stranger");
    assert_eq!(
        (none_lo, none_hi),
        (None, None),
        "no visible outcome: the prior stands"
    );
    assert_ne!(prior, Some(learned));
}

/// The tenant whose dense history must never reach `TENANT`'s readers.
#[cfg(feature = "epistemic")]
const FOREIGN: &str = "tenant-foreign";
#[cfg(feature = "epistemic")]
const SPARSE: &str = "sparse-opt";
#[cfg(feature = "epistemic")]
const DENSE: &str = "dense-opt";

/// The template record under `record_id`, executed as `option`, stored for `tenant`.
#[cfg(feature = "epistemic")]
fn put_option_record(
    h: &Harness,
    template: &medulla_decision_types::decision::statistical::log::DecisionLogEntry,
    tenant: &str,
    record_id: &str,
    option: &str,
) {
    let mut entry = template.clone();
    entry.record.record_id = record_id.to_string();
    entry.record.outcome =
        medulla_decision_types::decision::statistical::StatisticalOutcome::Explored {
            option_id: option.to_string(),
            propensity: medulla_decision_types::decision::UnitRationalWire::new(1, 1).unwrap(),
        };
    let key = medulla_persistence_records::decision_jobs::record_key(record_id);
    let bytes = medulla_persistence_records::decision_jobs::encode_artifact(&entry).unwrap();
    h.store
        .put_decision_artifacts(tenant, &[(key, bytes)])
        .unwrap();
}

/// `n` executed records of `option` in `TENANT`, `successes` of them independently judged good.
#[cfg(feature = "epistemic")]
async fn labelled_option(
    h: &Harness,
    template: &medulla_decision_types::decision::statistical::log::DecisionLogEntry,
    option: &str,
    n: usize,
    successes: usize,
) {
    for i in 0..n {
        let id = format!("{option}-{i}");
        put_option_record(h, template, TENANT, &id, option);
        let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        grant(h, &id, "evaluator", (now, now + 3_600_000));
        evaluate_by(h, "evaluator", &id, i < successes).await;
    }
}

/// `n` independently judged successes of `option` in the foreign tenant, written straight
/// to that tenant's own rows.
#[cfg(feature = "epistemic")]
fn foreign_dense_history(
    h: &Harness,
    template: &medulla_decision_types::decision::statistical::log::DecisionLogEntry,
    option: &str,
    n: usize,
) {
    use medulla_decision_types::decision::statistical::dataset::OutcomeFidelity;
    use medulla_decision_types::decision::statistical::log::{
        DecisionOutcomeEvaluation, StoredEvaluation,
    };
    use medulla_decision_types::decision::EvidenceClass;
    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    for i in 0..n {
        let id = format!("foreign-{option}-{i}");
        put_option_record(h, template, FOREIGN, &id, option);
        let stored = StoredEvaluation {
            evaluation: DecisionOutcomeEvaluation {
                record_id: id.clone(),
                evaluation_id: "eval-evaluator".to_string(),
                class: EvidenceClass::Observation,
                selected_agent: "retriever-agent".to_string(),
                lease_holder: "retriever-worker".to_string(),
                fidelity: OutcomeFidelity::ToolCalls,
                success: Some(true),
                backbone: None,
            },
            producer: principal_of("evaluator"),
            recorded_at_ms: now,
        };
        let key = medulla_persistence_records::decision_jobs::evaluation_key(&id, "eval-evaluator");
        let bytes = medulla_persistence_records::decision_jobs::encode_artifact(&stored).unwrap();
        h.store
            .put_decision_artifacts(FOREIGN, &[(key, bytes)])
            .unwrap();
        let record_key = medulla_persistence_records::decision_jobs::record_key(&id);
        assert!(
            h.store
                .decision_artifact(FOREIGN, &record_key)
                .unwrap()
                .is_some(),
            "the foreign tenant really holds the history"
        );
    }
}

/// What every reader of the pool answers for the two subjects.
#[cfg(feature = "epistemic")]
#[derive(Debug, Clone, PartialEq)]
struct PoolReads {
    /// The `reputation` relation (the SQL reader): subject, status, trials, mean.
    relation: Vec<Vec<Value>>,
    /// `SOURCE RELIABILITY <subject>`: (reliability, abstained) for the sparse and dense subject.
    uql: [(Option<f64>, Option<f64>); 2],
    /// `EpistemicStatus` over the subject node: the authority's governed read.
    status: [medulla_decision_types::decision::reputation::SourceReliability; 2],
    /// `RankByProvenance` over a candidate of each subject: the candidate's governed read.
    ranked: [medulla_decision_types::decision::reputation::SourceReliability; 2],
}

/// `SOURCE RELIABILITY <subject>` over a graph whose subject nodes carry `prior`, as the
/// served `DecisionViews` of `decider` run it: (reliability, abstained).
#[cfg(feature = "epistemic")]
fn uql_read(h: &Harness, subject: &str, prior: f64) -> (Option<f64>, Option<f64>) {
    use medulla_types::wire::UqlResult;
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT);
    let authority =
        medulla_server_api::carrier::CarrierAuthority::from_verified(&verified).unwrap();
    let views = super::super::stat_view::DecisionViews::of(h.store.clone(), &authority);
    let core = pool_graph(prior);
    let graph = core.analysis_snapshot();
    let semantic = medulla_semantic_index::semantic::SemanticStore::new();
    let ctx = medulla_exec::exec::PlanCtx::new(&graph, &semantic).with_decisions(&views);
    let text = format!(
        "MATCH () |> SOURCE RELIABILITY '{subject}' |> RETURN reliability, reliability_abstained"
    );
    let stmt = medulla_uql::parse_statement(&text, &medulla_uql::Params::new()).unwrap();
    let UqlResult::Rows { rows, .. } =
        medulla_plan::uql::serve::run_statement(&stmt, &ctx).unwrap()
    else {
        panic!("rows")
    };
    (rows[0].channels[0], rows[0].channels[1])
}

/// A graph holding the document node and both subject nodes, each at belief `prior`.
#[cfg(feature = "epistemic")]
fn pool_graph(prior: f64) -> medulla_graph_core::graph::GraphCore {
    let core = medulla_graph_core::graph::GraphCore::new();
    core.add_node(
        "d1".into(),
        rmp_serde::to_vec_named(&json!({"type": "Doc"})).unwrap(),
    );
    for subject in [SPARSE, DENSE] {
        core.add_node(
            subject.into(),
            rmp_serde::to_vec_named(&json!({"type": "Doc", "confidence": prior})).unwrap(),
        );
    }
    core
}

/// Run one served `Query` method through the real handler as `decider`.
#[cfg(feature = "epistemic")]
async fn served_query(
    h: &Harness,
    prior: f64,
    method: medulla_protocol_types::protocol::Method,
) -> medulla_protocol_types::protocol::Response {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT);
    let rls = std::sync::Arc::new(ServerState::test_isolation("decider"));
    let read_authority =
        medulla_server_api::carrier::GraphReadAuthority::from_verified(&verified, &rls).unwrap();
    medulla_h_query::query::try_handle(
        &h.state,
        medulla_server_api::api::reentry::TryHandleContext {
            req_id: 11,
            graph_name: "pool-graph",
            read_authority: Some(&read_authority),
            caller: "decider",
        },
        std::sync::Arc::new(pool_graph(prior)),
        method,
        &rls,
    )
    .await
    .expect("a served Query method is handled")
}

/// Every reader's answer for both subjects, under graph belief `prior` and the caller's
/// retrieval `weights`.
#[cfg(feature = "epistemic")]
async fn read_pool(
    h: &Harness,
    prior: f64,
    weights: medulla_protocol_types::protocol::RankWeightsWire,
) -> PoolReads {
    use medulla_protocol_types::protocol::{
        EpistemicStatusResult, Method, RankByProvenanceResult, RetrievalCandidateWire,
    };
    let relation = sql(
        h,
        "decider",
        "SELECT subject, status, trials, mean FROM reputation \
         WHERE subject_kind = 'option' ORDER BY subject",
    )
    .await;
    // Rows come back in subject order; the reads below list the sparse subject first.
    let mut relation = relation;
    relation.sort_by_key(|row| row[0] != json!(SPARSE));
    let uql = [uql_read(h, SPARSE, prior), uql_read(h, DENSE, prior)];
    let mut status = Vec::new();
    for subject in [SPARSE, DENSE] {
        let response = served_query(
            h,
            prior,
            Method::EpistemicStatus {
                node_id: subject.to_string(),
            },
        )
        .await;
        let result: EpistemicStatusResult = decode(response).unwrap();
        status.push(result.status.authority.source_reliability);
    }
    let candidates = [SPARSE, DENSE]
        .iter()
        .map(|subject| RetrievalCandidateWire {
            id: format!("from-{subject}"),
            similarity: 0.6,
            source_id: subject.to_string(),
            freshness: 0.5,
            calibration: None,
        })
        .collect();
    let response = served_query(
        h,
        prior,
        Method::RankByProvenance {
            candidates,
            weights,
        },
    )
    .await;
    let ranked: RankByProvenanceResult = decode(response).unwrap();
    let ranked_read = |id: &str| {
        ranked
            .ranked
            .iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("{id} is ranked"))
            .source_reliability
    };
    PoolReads {
        relation,
        uql,
        status: [status[0], status[1]],
        ranked: [
            ranked_read(&format!("from-{SPARSE}")),
            ranked_read(&format!("from-{DENSE}")),
        ],
    }
}

/// One visible sparse subject and one dense peer share one pool. The SQL `reputation`
/// relation, the UQL `SOURCE RELIABILITY` stage, `EpistemicStatus` and `RankByProvenance`
/// all abstain on the sparse subject with `REPUTATION_INSUFFICIENT_HISTORY` and report one
/// dense value for its peer; neither the graph prior nor the caller's retrieval weight moves
/// any of them, and another tenant's dense history of the sparse subject changes nothing and
/// discloses no count.
// spec: MEDULLA-DECISION-ENGINE-R118.1.2.4
#[cfg(feature = "epistemic")]
#[tokio::test]
async fn three_readers_agree_on_sparse_abstention_and_dense_estimate() {
    use medulla_decision_types::decision::reputation::{
        SourceReliability, REPUTATION_INSUFFICIENT_HISTORY,
    };
    use medulla_protocol_types::protocol::RankWeightsWire;
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    assert!(medulla_server_ops::api::admin::install_decide_seam(
        &h.state.read().await.seams,
        std::sync::Arc::new(super::super::DecideSeam),
    ));
    let template = executed_template(&h).await;
    labelled_option(&h, &template, DENSE, 12, 9).await;
    labelled_option(&h, &template, SPARSE, 3, 3).await;

    let weights = RankWeightsWire {
        similarity: 0.2,
        evidence_quality: 0.8,
    };
    let base = read_pool(&h, 0.1, weights).await;

    // The sparse subject abstains in every reader, with the one typed code.
    let sparse_read = SourceReliability::InsufficientHistory {
        trials: 3,
        min_support: 10,
    };
    assert_eq!(base.status[0], sparse_read);
    assert_eq!(base.ranked[0], sparse_read);
    for read in [base.status[0], base.ranked[0]] {
        assert_eq!(read.refusal(), Some(REPUTATION_INSUFFICIENT_HISTORY));
    }
    assert_eq!(base.uql[0], (None, Some(1.0)), "UQL: no number, abstained");
    assert_eq!(
        base.relation[0],
        vec![
            json!(SPARSE),
            json!("insufficient_history"),
            Value::Null,
            Value::Null
        ],
        "the relation reports no count and no estimate"
    );

    // The dense peer has one value, the same in every reader.
    let SourceReliability::Estimated {
        mean,
        lower,
        upper,
        trials,
    } = base.status[1]
    else {
        panic!("dense peer is estimated: {:?}", base.status[1])
    };
    assert_eq!(trials, 12);
    assert!(lower < mean && mean < upper && mean > 0.5);
    assert_eq!(base.ranked[1], base.status[1]);
    assert_eq!(base.relation[1][1], json!("estimated"));
    assert_eq!(base.relation[1][2], json!(12));
    assert_eq!(base.relation[1][3].as_f64(), Some(mean));
    let (uql_value, uql_abstained) = base.uql[1];
    assert!((uql_value.unwrap() - mean).abs() < 1e-6, "{uql_value:?}");
    assert_eq!(uql_abstained, Some(0.0));

    // Neither the graph prior nor the caller's retrieval weight changes any reader.
    let other_weights = RankWeightsWire {
        similarity: 0.9,
        evidence_quality: 0.1,
    };
    assert_eq!(read_pool(&h, 0.9, other_weights).await, base);

    // Another tenant's dense history of the sparse subject changes nothing and discloses
    // no count: the sparse subject still reports this tenant's three trials.
    foreign_dense_history(&h, &template, SPARSE, 12);
    let after = read_pool(&h, 0.1, weights).await;
    assert_eq!(after, base);
    assert_eq!(
        after.status[0],
        SourceReliability::InsufficientHistory {
            trials: 3,
            min_support: 10
        }
    );
}
