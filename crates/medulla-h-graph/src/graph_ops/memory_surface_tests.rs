//! Served memory surface proofs (MEDULLA-DECISION-ENGINE-R651.1 .. R651.3).
//! They run the admission, recall and export rules over one graph holding two
//! tenants, and check how the three methods are routed.

use super::memory_surface::{admit_remember, export, recall, Admitted};
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use medulla_server_mutation::mutation::{apply_coalescable_write, is_gateway_routed};
use medulla_types::contract::{ResourceId, TenantId};
use medulla_work_types::memory::{
    MemoryExportRequest, MemoryKind, MemoryRecallRequest, MemoryRecord, MemoryRememberRequest,
    MemoryTier, MEMORY_RECORD_SCHEMA_VERSION,
};

const NOW: i64 = 1_000_000;

fn record(tenant: &str, id: &str, tier: MemoryTier, text: &str) -> MemoryRecord {
    MemoryRecord {
        memory_id: ResourceId::new(id).unwrap(),
        tenant: TenantId::new(tenant).unwrap(),
        kind: MemoryKind::Fact,
        tier,
        text: text.into(),
        provenance_key: None,
        session_id: None,
        timestamp_ms: 10,
        topics: Vec::new(),
        pattern_key: None,
        importance: 1.0,
        access_count: 0,
        last_access_ms: 0,
        last_decay_ms: 0,
        forgotten: false,
        legal_hold: false,
        expires_at_ms: None,
        support_count: 0,
        token_estimate: 10,
        attachments: Vec::new(),
        schema_version: MEMORY_RECORD_SCHEMA_VERSION,
    }
}

fn remember(record: MemoryRecord, embedding: Vec<f32>) -> MemoryRememberRequest {
    MemoryRememberRequest { record, embedding }
}

fn store(core: &GraphCore, tenant: &str, request: &MemoryRememberRequest) {
    let Admitted::Write(write, answer) = admit_remember(core, tenant, request).unwrap() else {
        panic!("a first remember writes");
    };
    assert!(answer.stored);
    apply_coalescable_write(core, &write).unwrap();
}

fn recall_request(embedding: Vec<f32>, keywords: &[&str], budget: u32) -> MemoryRecallRequest {
    MemoryRecallRequest {
        query_embedding: embedding,
        keywords: keywords.iter().map(|k| k.to_string()).collect(),
        tiers: Vec::new(),
        scope: None,
        as_of_ms: None,
        budget,
        prime_session: None,
        include_plan: false,
    }
}

fn ids(hits: &[medulla_work_types::memory::MemoryHit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.record.memory_id.as_str()).collect()
}

// spec: MEDULLA-DECISION-ENGINE-R651.3
#[test]
fn memory_methods_are_served_and_classified() {
    let write = Method::MemoryRemember {
        request: Box::new(remember(record("tenant-a", "m:1", MemoryTier::Semantic, "x"), vec![])),
    };
    let read = Method::MemoryRecall {
        request: Box::new(recall_request(vec![1.0], &[], 10)),
    };
    let export_method = Method::MemoryExport {
        request: MemoryExportRequest::default(),
    };
    // A remember commits through the gateway; the two reads never do.
    assert!(is_gateway_routed(&write));
    assert!(!is_gateway_routed(&read));
    assert!(!is_gateway_routed(&export_method));

    let core = GraphCore::new();
    // A record naming another tenant is refused and writes nothing.
    let foreign = remember(record("tenant-a", "m:1", MemoryTier::Semantic, "x"), vec![]);
    let refusal = admit_remember(&core, "tenant-b", &foreign).err().unwrap();
    assert!(refusal.starts_with("ACCESS_DENIED"), "{refusal}");
    assert_eq!(core.node_count(), 0);

    // Another tenant's record reads as absent to recall and to export.
    store(&core, "tenant-a", &foreign);
    let request = recall_request(vec![], &["x"], 100);
    assert_eq!(recall(&core, "tenant-a", NOW, &request).unwrap().hits.len(), 1);
    assert!(recall(&core, "tenant-b", NOW, &request).unwrap().hits.is_empty());
    let exported = export(&core, "tenant-b", &MemoryExportRequest::default()).unwrap();
    assert!(exported.records.is_empty());
    // A recall with nothing to search by is refused.
    let empty = recall_request(vec![], &[], 100);
    assert!(recall(&core, "tenant-a", NOW, &empty).is_err());
}

// spec: MEDULLA-DECISION-ENGINE-R651.1, MEDULLA-DECISION-ENGINE-R651.2,
// MEDULLA-DECISION-ENGINE-R651.2.1, MEDULLA-DECISION-ENGINE-R651.3
#[test]
fn remember_recall_consolidate_and_export_round_trip() {
    let core = GraphCore::new();
    let mut sky = record("tenant-a", "m:sky", MemoryTier::Semantic, "the sky is blue");
    sky.importance = 2.0;
    sky.session_id = Some(ResourceId::new("sess:1").unwrap());
    let mut sky_copy = record("tenant-a", "m:sky2", MemoryTier::Semantic, "sky blue colour");
    sky_copy.session_id = Some(ResourceId::new("sess:1").unwrap());
    let grass = record("tenant-a", "m:grass", MemoryTier::Procedural, "grass is green");
    let mut run1 = record("tenant-a", "m:run1", MemoryTier::Episodic, "ran the build");
    run1.pattern_key = Some("build".into());
    let mut run2 = record("tenant-a", "m:run2", MemoryTier::Episodic, "ran the build again");
    run2.pattern_key = Some("build".into());
    let mut expired = record("tenant-a", "m:old", MemoryTier::Semantic, "sky blue stale");
    expired.expires_at_ms = Some(500);
    for (record, embedding) in [
        (sky.clone(), vec![1.0, 0.0]),
        (sky_copy, vec![0.99, 0.01]),
        (grass, vec![0.0, 1.0]),
        (run1, vec![0.5, 0.5]),
        (run2, vec![0.4, 0.6]),
        (expired, vec![1.0, 0.0]),
    ] {
        store(&core, "tenant-a", &remember(record, embedding));
    }
    store(
        &core,
        "tenant-b",
        &remember(record("tenant-b", "m:sky", MemoryTier::Semantic, "the sky is blue"), vec![1.0, 0.0]),
    );

    // A repeat of the same record is a replay; a different body is refused.
    let same = remember(sky.clone(), vec![1.0, 0.0]);
    assert!(matches!(admit_remember(&core, "tenant-a", &same), Ok(Admitted::Same(_))));
    let mut changed = sky.clone();
    changed.text = "the sky is green".into();
    assert!(admit_remember(&core, "tenant-a", &remember(changed, vec![1.0, 0.0])).is_err());

    // Recall ranks by cosine, drops the expired record and respects the budget.
    let found = recall(&core, "tenant-a", NOW, &recall_request(vec![1.0, 0.0], &[], 25)).unwrap();
    assert_eq!(ids(&found.hits), ["m:sky", "m:sky2"]);
    assert!(found.tokens_used <= 25);
    assert!(found.hits[0].score_q16 >= found.hits[1].score_q16);
    let by_keyword = recall(&core, "tenant-a", NOW, &recall_request(vec![], &["grass"], 100)).unwrap();
    assert_eq!(ids(&by_keyword.hits), ["m:grass"]);
    // As of an earlier instant the expired record is still visible.
    let mut earlier = recall_request(vec![1.0, 0.0], &[], 100);
    earlier.as_of_ms = Some(100);
    let past = recall(&core, "tenant-a", NOW, &earlier).unwrap();
    assert!(ids(&past.hits).contains(&"m:old"));

    // Priming a session returns its mementos; a plan proposes the merge and the promotion.
    let mut full = recall_request(vec![1.0, 0.0], &[], 100);
    full.prime_session = Some(ResourceId::new("sess:1").unwrap());
    full.include_plan = true;
    let answer = recall(&core, "tenant-a", NOW, &full).unwrap();
    assert_eq!(answer.mementos.len(), 1);
    assert_eq!(answer.mementos[0].source_ids.len(), 2);
    let plan = answer.plan.expect("a plan was asked for");
    let keeps: Vec<&str> = plan.merges.iter().map(|m| m.keep.as_str()).collect();
    assert_eq!(keeps, ["m:run1", "m:sky"]);
    assert_eq!(plan.promotions.len(), 1);
    assert_eq!(plan.promotions[0].support_count, 2);

    // Export returns only the procedural and semantic tiers of the tenant.
    let exported = export(&core, "tenant-a", &MemoryExportRequest::default()).unwrap();
    assert!(exported.records.iter().all(|r| matches!(r.tier, MemoryTier::Procedural | MemoryTier::Semantic)));
    assert_eq!(exported.manifest.record_count as usize, exported.records.len());
    assert_eq!(exported.manifest.ids_digest.len(), 64);
    assert!(!exported.manifest.truncated);
    let limited = export(&core, "tenant-a", &MemoryExportRequest { limit: Some(1) }).unwrap();
    assert_eq!(limited.records.len(), 100.min(exported.records.len()));
}
