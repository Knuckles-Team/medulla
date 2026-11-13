//! Fresh data through the search connector's virtual-graph source
//! (MEDULLA-DECISION-ENGINE-R174.9): `DecisionLog.learn` refreshes a logical
//! graph through the registered search source, deduplicates by canonical URL
//! and content digest, marks changed regions and writes a receipt.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::retrieval_tests::learn;
use super::*;
use crate::decide::stat_executor::ExecutionContext;
use crate::decide::stat_freshness::{
    clear_search_source_port, held_rows, region_revision, register_search_source_port,
    stored_receipt, SearchHit, SearchHits, SearchSourceBinding, SearchSourcePort,
};
use medulla_decision_stats::decision::drift::ChangedRevisions;
use medulla_decision_types::decision::statistical::retrieval::{LearningRecorded, LearningWrite};
use medulla_decision_types::decision::statistical::retrieval_drift::{
    DriftKind, RefreshPlan, RefreshPriority, RegionRef,
};
use medulla_decision_types::decision::statistical::retrieval_refresh::{
    FreshnessReceipt, FreshnessRefresh, RefreshBudget, RefreshTrigger,
};

const SOURCE: &str = "https://knuckles-team.github.io/ontology/virtual-graph#searxng-connection";
const GRAPH: &str = "kg-web";

/// One test at a time owns the process-wide port.
static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The recorded search source: a fixed table of hits per query, and a count of
/// the reads it served.
struct RecordedSource {
    hits: Mutex<Vec<SearchHit>>,
    reads: AtomicUsize,
    cost: u64,
}

impl RecordedSource {
    fn new(hits: Vec<SearchHit>, cost: u64) -> Arc<Self> {
        Arc::new(Self {
            hits: Mutex::new(hits),
            reads: AtomicUsize::new(0),
            cost,
        })
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn set_snippet(&self, index: usize, snippet: &str) {
        self.hits.lock().unwrap()[index].snippet = snippet.to_string();
    }
}

impl SearchSourcePort for RecordedSource {
    fn binding(&self, _tenant: &str, graph: &str) -> Option<SearchSourceBinding> {
        (graph == GRAPH).then(|| SearchSourceBinding {
            connection_iri: SOURCE.to_string(),
            cost_units_per_request: self.cost,
        })
    }

    fn read(
        &self,
        _binding: &SearchSourceBinding,
        _query: &str,
        _now_ms: u64,
    ) -> Result<SearchHits, String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(SearchHits {
            query_digest: "a".repeat(64),
            hits: self.hits.lock().unwrap().clone(),
        })
    }
}

fn hit(path: &str) -> SearchHit {
    SearchHit {
        url: format!("HTTPS://Example.ORG:443/{path}#top"),
        title: format!("title {path}"),
        snippet: format!("snippet {path}"),
        engine: "searxng".to_string(),
        retrieved_at_s: 1_700_000_000,
    }
}

fn three_hits() -> Vec<SearchHit> {
    vec![hit("a"), hit("b"), hit("c")]
}

fn refresh(graph: &str, queries: usize, budget: (u32, u64)) -> FreshnessRefresh {
    FreshnessRefresh {
        schema_version: 1,
        logical_graph: graph.to_string(),
        queries: BoundedVec::new(vec!["fresh topic".to_string(); queries]).unwrap(),
        trigger: RefreshTrigger::Staleness,
        priority: 2,
        acl_policy_ref: "policy:acl".to_string(),
        privacy_policy_ref: "policy:privacy".to_string(),
        budget: RefreshBudget {
            max_requests: budget.0,
            max_cost_units: budget.1,
        },
    }
}

async fn serve(h: &Harness, request: FreshnessRefresh) -> Result<FreshnessReceipt, String> {
    match learn(
        h,
        "decider",
        LearningWrite::RefreshFreshness {
            request: Box::new(request),
        },
    )
    .await?
    {
        LearningRecorded::Refreshed(receipt) => Ok(*receipt),
        other => panic!("refresh receipt: {other:?}"),
    }
}

fn ctx<'a>(h: &'a Harness, tenant: &'a str) -> ExecutionContext<'a> {
    ExecutionContext {
        store: &h.store,
        tenant_id: tenant,
        now_ms: 1,
        server_secret: b"",
        matched: &[],
        designation: None,
    }
}

fn nothing_written(h: &Harness) -> bool {
    h.store
        .decision_artifacts_with_prefix(TENANT, "freshness:", 10)
        .unwrap()
        .is_empty()
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.9.3: Wiring: one virtual-graph read, one region mark.
// spec: MEDULLA-DECISION-ENGINE-R174.9.3
#[tokio::test]
async fn refresh_reads_the_virtual_graph_and_marks_one_region() {
    let _gate = GATE.lock().await;
    let h = Harness::new().await;
    let source = RecordedSource::new(vec![hit("a")], 1);
    register_search_source_port(source.clone());

    let receipt = serve(&h, refresh(GRAPH, 1, (4, 10))).await.unwrap();
    assert_eq!(
        source.reads(),
        1,
        "one read through the virtual-graph source"
    );
    assert_eq!(receipt.regions.len(), 1, "one region mark");
    assert_eq!(receipt.new_count, 1);
    assert_eq!(receipt.source_connection_iri, SOURCE);
    assert_eq!(region_revision(&ctx(&h, TENANT), GRAPH).unwrap(), 1);

    // Cross tenant: another tenant sees none of these rows, and its own
    // refresh of the same URL is new for it.
    assert!(held_rows(&ctx(&h, "tenant-other"), GRAPH)
        .unwrap()
        .is_empty());
    assert_eq!(region_revision(&ctx(&h, "tenant-other"), GRAPH).unwrap(), 0);
    let other = super::super::stat_freshness::refresh_with(
        &ctx(&h, "tenant-other"),
        source.as_ref(),
        &refresh(GRAPH, 1, (4, 10)),
    )
    .unwrap();
    assert_eq!(other.new_count, 1);
    assert_eq!(held_rows(&ctx(&h, TENANT), GRAPH).unwrap().len(), 1);
    clear_search_source_port();
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.9.4: Refusal: each case, no row and no mark.
// spec: MEDULLA-DECISION-ENGINE-R174.9.4
#[tokio::test]
async fn freshness_refusals() {
    let _gate = GATE.lock().await;
    let h = Harness::new().await;
    let source = RecordedSource::new(three_hits(), 5);
    register_search_source_port(source.clone());

    let mut no_acl = refresh(GRAPH, 1, (4, 100));
    no_acl.acl_policy_ref = " ".to_string();
    let mut no_privacy = refresh(GRAPH, 1, (4, 100));
    no_privacy.privacy_policy_ref = String::new();
    let cases = [
        (no_acl, "REFRESH_ACL_POLICY_MISSING"),
        (no_privacy, "REFRESH_PRIVACY_POLICY_MISSING"),
        (
            refresh(GRAPH, 3, (2, 1_000)),
            "REFRESH_RATE_BUDGET_EXCEEDED",
        ),
        (refresh(GRAPH, 2, (4, 9)), "REFRESH_COST_BUDGET_EXCEEDED"),
        (
            refresh("kg-without-source", 1, (4, 100)),
            "SEARCH_SOURCE_UNREGISTERED",
        ),
    ];
    for (request, code) in cases {
        let refused = serve(&h, request).await.unwrap_err();
        assert!(refused.starts_with(code), "{code}: {refused}");
        assert_eq!(source.reads(), 0, "{code} read nothing");
        assert!(
            nothing_written(&h),
            "{code} wrote no row and no region mark"
        );
    }
    clear_search_source_port();
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.9.5: Evidence: provenance and the receipt.
// spec: MEDULLA-DECISION-ENGINE-R174.9.5
#[tokio::test]
async fn result_provenance_and_receipt() {
    let _gate = GATE.lock().await;
    let h = Harness::new().await;
    let source = RecordedSource::new(three_hits(), 2);
    register_search_source_port(source.clone());

    let first = serve(&h, refresh(GRAPH, 2, (8, 100))).await.unwrap();
    // Two queries return the same three URLs: three rows, three repeats.
    assert_eq!(
        (
            first.new_count,
            first.unchanged_count,
            first.superseded_count
        ),
        (3, 3, 0)
    );
    assert_eq!(first.requests_spent, 2);
    assert_eq!(first.cost_units_spent, 4);
    assert_eq!(first.regions.len(), 3);
    assert_eq!(
        stored_receipt(&ctx(&h, TENANT), &first.refresh_id).unwrap(),
        Some(first.clone())
    );

    let rows = held_rows(&ctx(&h, TENANT), GRAPH).unwrap();
    assert_eq!(rows.len(), 3);
    let row = &rows["https://example.org/a"];
    assert_eq!(row.result.source_connection_iri, SOURCE);
    assert_eq!(row.result.retrieved_at, 1_700_000_000_000);
    assert_eq!(row.result.query_text, "fresh topic");
    assert_eq!(row.result.query_digest, "a".repeat(64));
    assert_eq!(row.result.refresh_id, first.refresh_id);

    source.set_snippet(1, "a changed snippet");
    let second = serve(&h, refresh(GRAPH, 1, (8, 100))).await.unwrap();
    assert_eq!(
        (
            second.new_count,
            second.unchanged_count,
            second.superseded_count
        ),
        (0, 2, 1)
    );
    assert_eq!(second.stored.len(), 1);
    assert_eq!(second.regions.len(), 1);
    let rows = held_rows(&ctx(&h, TENANT), GRAPH).unwrap();
    assert!(rows["https://example.org/b"].supersedes.is_some());
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.9: the journey: dedupe, supersede, and feed the drift refresh.
// spec: MEDULLA-DECISION-ENGINE-R174.9.1, MEDULLA-DECISION-ENGINE-R174.9.2
#[tokio::test]
async fn search_refresh_dedupes_and_feeds_drift_refresh() {
    let _gate = GATE.lock().await;
    let h = Harness::new().await;
    let source = RecordedSource::new(three_hits(), 1);
    register_search_source_port(source.clone());

    let first = serve(&h, refresh(GRAPH, 1, (50, 500))).await.unwrap();
    assert_eq!(first.new_count, 3);
    let repeated = serve(&h, refresh(GRAPH, 1, (50, 500))).await.unwrap();
    assert_eq!((repeated.new_count, repeated.unchanged_count), (0, 3));
    assert!(repeated.regions.is_empty(), "a repeat marks no region");
    assert_eq!(region_revision(&ctx(&h, TENANT), GRAPH).unwrap(), 3);

    source.set_snippet(2, "the page changed");
    let changed = serve(&h, refresh(GRAPH, 1, (50, 500))).await.unwrap();
    assert_eq!((changed.superseded_count, changed.unchanged_count), (1, 2));
    let mark = &changed.regions[0];

    // The marks feed the drift refresh: the bitmap holds the changed revision
    // and the plan names exactly the marked region.
    let mut bitmap = ChangedRevisions::default();
    for region in first.regions.iter().chain(changed.regions.iter()) {
        bitmap.mark(region.revision);
    }
    assert!(bitmap.contains(mark.revision));
    let plan = RefreshPlan::new(
        GRAPH.to_string(),
        vec![RegionRef {
            shard_id: mark.shard_id,
            node_type: mark.node_type.clone(),
            revision_from: mark.revision,
            revision_to: mark.revision,
        }],
        DriftKind::Staleness,
        RefreshPriority::Downtime,
        "gen-2".to_string(),
    )
    .unwrap();
    assert_eq!(plan.regions.as_slice().len(), 1);
    assert_eq!(plan.regions.as_slice()[0].shard_id, mark.shard_id);
    clear_search_source_port();
}
