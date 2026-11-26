//! Served memory surface proofs (MEDULLA-DECISION-ENGINE-R651.1 .. R651.3).
//! They run the admission, recall and export rules over one graph holding two
//! tenants, and check how the three methods are routed.

use super::memory_surface::{absent_record, admit_remember, export, promotion_support_refusal, recall, Admitted};
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use medulla_server_mutation::mutation::is_gateway_routed;
use medulla_types::contract::{ResourceId, TenantId};
use medulla_work_types::memory::{
    MediaAttachment, MemoryExportRequest, MemoryKind, MemoryRecallRequest, MemoryRecord, MemoryRememberRequest,
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

fn admit(
    core: &GraphCore,
    tenant: &str,
    request: &MemoryRememberRequest,
    known: &std::collections::BTreeSet<String>,
) -> Result<Admitted, String> {
    admit_remember(core, tenant, request, known, tenant_max_bytes())
}

/// Commit the one batch an admitted remember writes.
fn apply_batch(core: &GraphCore, write: &Method) {
    let Method::BatchUpdate { operations_msgpack } = write else {
        panic!("an admitted write is one batch");
    };
    medulla_graph_algos::algorithms::batch_update(core, operations_msgpack).unwrap();
}

fn remember(record: MemoryRecord, embedding: Vec<f32>) -> MemoryRememberRequest {
    MemoryRememberRequest { record, embedding, episode: None }
}

fn store(core: &GraphCore, tenant: &str, request: &MemoryRememberRequest) {
    let Admitted::Write(write, embed, answer) = admit(core, tenant, request, &Default::default()).unwrap()
    else {
        panic!("a first remember writes");
    };
    assert!(answer.stored);
    apply_batch(core, &write);
    // The vector lands beside the node through the same write the AddEmbedding route runs.
    if let Some(embed) = embed {
        let Method::AddEmbedding { node_id, embedding } = *embed else {
            panic!("the vector write is an AddEmbedding");
        };
        core.semantic_store.write().add_embedding(node_id, embedding).unwrap();
    }
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
        include_media: false,
        previous_manifest_digest: None,
        document_set_ids: Vec::new(),
        min_relevance: None,
        include_archived: false,
        current_revision: None,
        episode_scope: None,
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
    // A record naming another tenant answers like a record that does not exist for the caller:
    // no ACCESS_DENIED and no tenant-specific code (MEDULLA-DECISION-ENGINE-R651.4), and it
    // writes nothing.
    let foreign = remember(record("tenant-a", "m:1", MemoryTier::Semantic, "x"), vec![]);
    let refusal = admit(&core, "tenant-b", &foreign, &Default::default()).err().unwrap();
    assert!(refusal.starts_with("PARAMETER_INVALID"), "{refusal}");
    assert!(!refusal.contains("ACCESS_DENIED") && !refusal.to_lowercase().contains("tenant"), "{refusal}");
    assert_eq!(core.node_count(), 0);
    // Byte for byte the answer a write naming a nonexistent record receives.
    assert_eq!(refusal, absent_record("m:1"));

    // Another tenant's record reads as absent to recall and to export.
    store(&core, "tenant-a", &foreign);
    let request = recall_request(vec![], &["x"], 100);
    assert_eq!(recall(&core, "tenant-a", NOW, &request, None).unwrap().hits.len(), 1);
    assert!(recall(&core, "tenant-b", NOW, &request, None).unwrap().hits.is_empty());
    let exported = export(&core, "tenant-b", &MemoryExportRequest::default()).unwrap();
    assert!(exported.records.is_empty());
    // A recall with nothing to search by is refused.
    let empty = recall_request(vec![], &[], 100);
    assert!(recall(&core, "tenant-a", NOW, &empty, None).is_err());
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
    assert!(matches!(admit(&core, "tenant-a", &same, &Default::default()), Ok(Admitted::Same(_))));
    let mut changed = sky.clone();
    changed.text = "the sky is green".into();
    assert!(admit(&core, "tenant-a", &remember(changed, vec![1.0, 0.0]), &Default::default()).is_err());

    // Recall ranks by cosine, drops the expired record and respects the budget.
    let found = recall(&core, "tenant-a", NOW, &recall_request(vec![1.0, 0.0], &[], 25), None).unwrap();
    assert_eq!(ids(&found.hits), ["m:sky", "m:sky2"]);
    assert!(found.tokens_used <= 25);
    assert!(found.hits[0].score_q16 >= found.hits[1].score_q16);
    let by_keyword = recall(&core, "tenant-a", NOW, &recall_request(vec![], &["grass"], 100), None).unwrap();
    assert_eq!(ids(&by_keyword.hits), ["m:grass"]);
    // As of an earlier instant the expired record is still visible.
    let mut earlier = recall_request(vec![1.0, 0.0], &[], 100);
    earlier.as_of_ms = Some(100);
    let past = recall(&core, "tenant-a", NOW, &earlier, None).unwrap();
    assert!(ids(&past.hits).contains(&"m:old"));

    // Priming a session returns its mementos; a plan proposes the merge and the promotion.
    let mut full = recall_request(vec![1.0, 0.0], &[], 100);
    full.prime_session = Some(ResourceId::new("sess:1").unwrap());
    full.include_plan = true;
    let answer = recall(&core, "tenant-a", NOW, &full, None).unwrap();
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
    assert_eq!(exported.manifest.manifest_digest.len(), 64);
    assert!(!exported.manifest.truncated);
    let limited = export(&core, "tenant-a", &MemoryExportRequest { limit: Some(1) }).unwrap();
    assert_eq!(limited.records.len(), 100.min(exported.records.len()));
}

// spec: MEDULLA-DECISION-ENGINE-R154.2, MEDULLA-DECISION-ENGINE-R154.6
#[test]
fn recall_stays_stable_as_the_corpus_doubles() {
    // Five needle facts rank first by cosine; every other fact is orthogonal filler, so a
    // larger corpus must return the same top-k for the same queries.
    let corpus = |size: usize| {
        let core = GraphCore::new();
        for n in 0..size {
            let (text, embedding) = if n < 5 {
                (format!("needle fact {n}"), vec![1.0, 0.1 * n as f32])
            } else {
                (format!("filler fact {n}"), vec![0.0, 1.0])
            };
            store(
                &core,
                "tenant-a",
                &remember(record("tenant-a", &format!("m:{n:05}"), MemoryTier::Semantic, &text), embedding),
            );
        }
        core
    };
    let by_vector = recall_request(vec![1.0, 0.0], &[], 1_000);
    let by_keyword = recall_request(vec![], &["needle"], 1_000);
    let mut expected: Option<(Vec<String>, Vec<String>)> = None;
    for size in [1_000, 2_000, 4_000] {
        let core = corpus(size);
        let vector = recall(&core, "tenant-a", NOW, &by_vector, None).unwrap();
        let keyword = recall(&core, "tenant-a", NOW, &by_keyword, None).unwrap();
        let found = (
            ids(&vector.hits).into_iter().map(String::from).collect::<Vec<_>>(),
            ids(&keyword.hits).into_iter().map(String::from).collect::<Vec<_>>(),
        );
        assert_eq!(found.0.len(), 5, "size {size}: {found:?}");
        assert_eq!(found.1.len(), 5, "size {size}: {found:?}");
        match &expected {
            None => expected = Some(found),
            Some(first) => assert_eq!(&found, first, "top-k changed at {size} facts"),
        }
    }
}

// spec: MEDULLA-MEDIA-VOCAB-R650.3
#[test]
fn media_attachment_remember_and_recall() {
    let core = GraphCore::new();
    let digest = "a".repeat(64);
    let derivation = "b".repeat(64);
    let mut image = record("tenant-a", "m:image", MemoryTier::Semantic, "a photo");
    image.attachments = vec![MediaAttachment {
        blob_digest: digest.clone(),
        mime: "image/png".into(),
        byte_len: 12,
        derivation_ids: vec![ResourceId::new(derivation.as_str()).unwrap()],
    }];
    // An attachment the owner scope does not hold refuses before any write.
    let refusal = admit(&core, "tenant-a", &remember(image.clone(), vec![0.0, 1.0]), &Default::default())
        .err()
        .unwrap();
    assert!(refusal.starts_with("MEDIA_BLOB_UNKNOWN"), "{refusal}");
    assert_eq!(core.node_count(), 0);

    let known: std::collections::BTreeSet<String> = [digest].into_iter().collect();
    let request = remember(image, vec![0.0, 1.0]);
    let Admitted::Write(write, embed, _) = admit(&core, "tenant-a", &request, &known).unwrap() else {
        panic!("a first remember writes");
    };
    apply_batch(&core, &write);
    let Method::AddEmbedding { node_id, embedding } = *embed.unwrap() else {
        panic!("the vector write is an AddEmbedding");
    };
    core.semantic_store.write().add_embedding(node_id, embedding).unwrap();
    // The caption embedding sits on its derivation node.
    core.semantic_store.write().add_embedding(derivation, vec![1.0, 0.0]).unwrap();

    // Without `include_media` the caption vector does not find the record.
    let query = recall_request(vec![1.0, 0.0], &[], 100);
    assert!(recall(&core, "tenant-a", NOW, &query, None).unwrap().hits.is_empty());
    let mut media = query.clone();
    media.include_media = true;
    let hits = recall(&core, "tenant-a", NOW, &media, None).unwrap().hits;
    assert_eq!(ids(&hits), ["m:image"]);
    assert_eq!(hits[0].record.attachments.len(), 1);
    // Another tenant sees nothing, and `include_media` needs a query embedding.
    assert!(recall(&core, "tenant-b", NOW, &media, None).unwrap().hits.is_empty());
    let mut keywords_only = recall_request(vec![], &["photo"], 100);
    keywords_only.include_media = true;
    assert!(recall(&core, "tenant-a", NOW, &keywords_only, None)
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
}

// spec: MEDULLA-MEDIA-VOCAB-R650.4
#[test]
fn media_refusals() {
    let core = GraphCore::new();
    let digest = "c".repeat(64);
    let known: std::collections::BTreeSet<String> = [digest.clone()].into_iter().collect();
    let attach = |byte_len: u64| {
        let mut image = record("tenant-a", "m:big", MemoryTier::Semantic, "a photo");
        image.attachments = vec![MediaAttachment {
            blob_digest: digest.clone(),
            mime: "image/png".into(),
            byte_len,
            derivation_ids: Vec::new(),
        }];
        remember(image, vec![])
    };
    // A blob above the default limit is refused, and nothing is stored.
    let limit = medulla_work_types::memory::MEDIA_MAX_BLOB_BYTES_DEFAULT;
    let refusal = admit(&core, "tenant-a", &attach(limit + 1), &known).err().unwrap();
    assert!(refusal.starts_with("MEDIA_TOO_LARGE"), "{refusal}");
    assert_eq!(core.node_count(), 0);
    // A blob at the limit passes the size check.
    assert!(admit(&core, "tenant-a", &attach(limit), &known).is_ok());
    // A digest the owner scope does not hold is refused before the size is read.
    let refusal = admit(&core, "tenant-a", &attach(limit + 1), &Default::default())
        .err()
        .unwrap();
    assert!(refusal.starts_with("MEDIA_BLOB_UNKNOWN"), "{refusal}");
    assert_eq!(core.node_count(), 0);
}

// spec: MEDULLA-MEDIA-VOCAB-R650.5
#[test]
fn media_evidence_reads_back() {
    let core = GraphCore::new();
    let digest = "d".repeat(64);
    let derivation = "e".repeat(64);
    let mut image = record("tenant-a", "m:evidence", MemoryTier::Semantic, "a chart");
    image.attachments = vec![MediaAttachment {
        blob_digest: digest.clone(),
        mime: "image/png".into(),
        byte_len: 4096,
        derivation_ids: vec![ResourceId::new(derivation.as_str()).unwrap()],
    }];
    let known: std::collections::BTreeSet<String> = [digest.clone()].into_iter().collect();
    let request = remember(image, vec![]);
    let Admitted::Write(write, _, _) = admit(&core, "tenant-a", &request, &known).unwrap() else {
        panic!("a first remember writes");
    };
    apply_batch(&core, &write);

    // The attachment keys ride the memory node, whose id is the citing memory id.
    let blob = core.get_node_properties("memory:tenant-a:m:evidence").unwrap();
    let props = medulla_types::msgpack::decode_property_object(&blob).unwrap();
    let attachment = &props["attachments"].as_array().unwrap()[0];
    assert_eq!(attachment["blob_digest"], digest.as_str());
    assert_eq!(attachment["mime"], "image/png");
    assert_eq!(attachment["byte_len"], 4096);
    assert_eq!(attachment["derivation_ids"][0], derivation.as_str());
    // The same record is readable under its own tenant only.
    assert!(recall(&core, "tenant-b", NOW, &recall_request(vec![], &["chart"], 100), None)
        .unwrap()
        .hits
        .is_empty());
    let own = recall(&core, "tenant-a", NOW, &recall_request(vec![], &["chart"], 100), None).unwrap();
    assert_eq!(own.hits[0].record.attachments[0].byte_len, 4096);
}

fn promotion_props() -> serde_json::Map<String, serde_json::Value> {
    let mut props = serde_json::Map::new();
    props.insert("type".into(), "SemanticMemory".into());
    props.insert("pattern_key".into(), "build".into());
    props.insert("support_count".into(), 3.into());
    props
}

fn episode_ids(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("ep:{index}")).collect()
}

// spec: MEDULLA-DECISION-ENGINE-R651.7
#[test]
fn episodes_promote_to_a_linked_semantic_fact() {
    let core = GraphCore::new();
    let episodes = episode_ids(3);
    for id in &episodes {
        core.add_node(
            id.clone(),
            rmp_serde::to_vec_named(&serde_json::json!({"type": "EpisodicMemory"})).unwrap(),
        );
    }
    // Three episodes reach the default threshold of three: nothing refuses, and the route's
    // own `consolidate` writes one semantic node with one CONSOLIDATES edge per episode.
    assert_eq!(promotion_support_refusal(&episodes, &promotion_props()), None);
    let semantic = core.consolidate(&episodes, promotion_props());
    let properties = medulla_types::msgpack::decode_property_object(
        &core.get_node_properties(&semantic).expect("the semantic node exists"),
    )
    .unwrap();
    assert_eq!(properties["pattern_key"], "build");
    assert_eq!(properties["support_count"], 3);
    let edges = core.analysis_snapshot().edge_properties;
    for id in &episodes {
        assert!(
            edges.contains_key(&(semantic.clone(), id.clone())),
            "no CONSOLIDATES edge to {id}"
        );
    }

    // A two-episode group is below the threshold and names its code.
    let refusal = promotion_support_refusal(&episode_ids(2), &promotion_props())
        .expect("two episodes are below the threshold");
    assert!(refusal.starts_with("MEMORY_SUPPORT_BELOW_THRESHOLD"), "{refusal}");
    // The same id sent twice is one episode.
    let repeated = vec!["ep:0".to_string(); 5];
    assert!(promotion_support_refusal(&repeated, &promotion_props()).is_some());
    // A consolidation that names no pattern key is a plain consolidation and is not counted.
    let mut plain = promotion_props();
    plain.remove("pattern_key");
    assert_eq!(promotion_support_refusal(&episode_ids(2), &plain), None);
}

// spec: MEDULLA-DECISION-ENGINE-R651.5
#[test]
fn consolidation_and_export_evidence_reads_back() {
    let core = GraphCore::new();
    for (id, tier, text) in [
        ("m:grass", MemoryTier::Procedural, "grass is green"),
        ("m:sky", MemoryTier::Semantic, "the sky is blue"),
    ] {
        store(&core, "tenant-a", &remember(record("tenant-a", id, tier, text), vec![1.0, 0.0]));
    }
    // The export manifest counts the records, names the tiers and digests the records themselves.
    let exported = export(&core, "tenant-a", &MemoryExportRequest::default()).unwrap();
    assert_eq!(exported.manifest.record_count, 2);
    assert_eq!(exported.manifest.tiers, [MemoryTier::Procedural, MemoryTier::Semantic]);
    let digest = exported.manifest.manifest_digest.clone();
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    assert_eq!(export(&core, "tenant-a", &MemoryExportRequest::default()).unwrap().manifest.manifest_digest, digest);

    // Changing a record's text changes the digest; the ids alone no longer decide it.
    let other = GraphCore::new();
    for (id, tier, text) in [
        ("m:grass", MemoryTier::Procedural, "grass is green"),
        ("m:sky", MemoryTier::Semantic, "the sky is grey"),
    ] {
        store(&other, "tenant-a", &remember(record("tenant-a", id, tier, text), vec![1.0, 0.0]));
    }
    let changed = export(&other, "tenant-a", &MemoryExportRequest::default()).unwrap();
    assert_ne!(changed.manifest.manifest_digest, digest);

    // Recall answers the plan digest of its own plan, and a drift report only when asked.
    let mut asked = recall_request(vec![1.0, 0.0], &[], 100);
    let plain = recall(&core, "tenant-a", NOW, &asked, None).unwrap();
    assert_eq!(plain.plan_digest.len(), 64);
    assert!(plain.drift.is_none());
    assert_eq!(recall(&core, "tenant-a", NOW, &asked, None).unwrap().plan_digest, plain.plan_digest);
    let mut narrower = asked.clone();
    narrower.scope = Some(ResourceId::new("sess:1").unwrap());
    assert_ne!(recall(&core, "tenant-a", NOW, &narrower, None).unwrap().plan_digest, plain.plan_digest);

    asked.previous_manifest_digest = Some(digest.clone());
    let same = recall(&core, "tenant-a", NOW, &asked, None).unwrap();
    assert_eq!(same.plan_digest, plain.plan_digest, "the comparison input is not part of the plan");
    let report = same.drift.expect("a previous digest was supplied");
    assert!(!report.drifted);
    assert_eq!(report.manifest_digest, digest);
    asked.previous_manifest_digest = Some(changed.manifest.manifest_digest.clone());
    let drifted = recall(&core, "tenant-a", NOW, &asked, None).unwrap().drift.expect("a report");
    assert!(drifted.drifted);
    assert_eq!(drifted.previous_manifest_digest, changed.manifest.manifest_digest);
}

// spec: MEDULLA-DECISION-ENGINE-R651.4
#[test]
fn memory_refusals() {
    let core = GraphCore::new();
    // A foreign tenant's record reads exactly as one that is absent: the same code and text
    // shape as a record the caller never stored, and nothing is written.
    let foreign = remember(record("tenant-a", "m:1", MemoryTier::Semantic, "x"), vec![]);
    let refusal = admit(&core, "tenant-b", &foreign, &Default::default()).err().unwrap();
    assert!(refusal.starts_with("PARAMETER_INVALID"), "{refusal}");
    assert_eq!(refusal, absent_record("m:1"), "a foreign tenant's write is an existence oracle");
    assert!(!refusal.to_lowercase().contains("tenant"), "{refusal}");
    assert_eq!(core.node_count(), 0);

    // A record whose stored encoding is above the knob default of 65536 bytes is refused.
    let huge = remember(
        record("tenant-a", "m:big", MemoryTier::Semantic, &"x".repeat(70_000)),
        vec![],
    );
    let refusal = admit(&core, "tenant-a", &huge, &Default::default()).err().unwrap();
    assert!(refusal.starts_with("MEMORY_ENTRY_TOO_LARGE"), "{refusal}");
    assert_eq!(core.node_count(), 0);

    // A hard removal leaves a node a live node still points to, and reports it.
    let props = |importance: f64| {
        rmp_serde::to_vec_named(&serde_json::json!({"importance": importance, "last_access_ms": 0})).unwrap()
    };
    core.add_node("old".into(), props(0.1));
    core.add_node("live".into(), props(9.0));
    core.add_edge("live".into(), "old".into(), rmp_serde::to_vec_named(&serde_json::json!({"relationship": "CITES"})).unwrap()).unwrap();
    let (pruned, live_referenced) = core.evict_below(&["old".to_string()], 0.5, true);
    assert!(pruned.is_empty());
    assert_eq!(live_referenced, ["old"]);
    assert!(core.get_node_properties("old").is_some(), "the node stays with its edges");
    // Once the referrer is forgotten, the removal goes through.
    assert!(core.forget("live", false));
    let (pruned, live_referenced) = core.evict_below(&["old".to_string()], 0.5, true);
    assert_eq!(pruned, ["old"]);
    assert!(live_referenced.is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R154.1
#[test]
fn tenant_total_admission_keeps_a_durable_counter() {
    let core = GraphCore::new();
    let first = remember(record("tenant-a", "m:1", MemoryTier::Semantic, "first"), vec![]);
    store(&core, "tenant-a", &first);
    let stored = |id: &str| core.get_node_properties(id).unwrap().len() as u64;
    // The counter is a graph node written with the record, and equals the stored bytes.
    assert_eq!(read_usage(&core, "tenant-a"), Some(stored("memory:tenant-a:m:1")));
    let second = remember(record("tenant-a", "m:2", MemoryTier::Semantic, "second"), vec![]);
    store(&core, "tenant-a", &second);
    let total = stored("memory:tenant-a:m:1") + stored("memory:tenant-a:m:2");
    assert_eq!(read_usage(&core, "tenant-a"), Some(total));
    // Another tenant's counter is its own.
    assert_eq!(read_usage(&core, "tenant-b"), None);

    // The boundary: a total that lands exactly on the cap is admitted, one byte more is refused.
    let third = remember(record("tenant-a", "m:3", MemoryTier::Semantic, "third"), vec![]);
    let third_len = node_blob("tenant-a", &third).unwrap().len() as u64;
    let known = Default::default();
    assert!(admit_remember(&core, "tenant-a", &third, &known, total + third_len).is_ok());
    let refusal = admit_remember(&core, "tenant-a", &third, &known, total + third_len - 1)
        .err()
        .unwrap();
    assert!(refusal.starts_with("MEMORY_TENANT_QUOTA_EXCEEDED"), "{refusal}");
    assert!(core.get_node_properties("memory:tenant-a:m:3").is_none());
    // Another tenant is not charged for tenant-a's bytes.
    let other = remember(record("tenant-b", "m:1", MemoryTier::Semantic, "first"), vec![]);
    let other_len = node_blob("tenant-b", &other).unwrap().len() as u64;
    assert!(admit_remember(&core, "tenant-b", &other, &known, other_len).is_ok());

    // A hard removal moves the counter in the same transaction, so it is exact without a re-measure.
    assert!(core.forget("memory:tenant-a:m:2", true));
    let after_delete = stored("memory:tenant-a:m:1");
    assert_eq!(read_usage(&core, "tenant-a"), Some(after_delete));
    // Delete then remember at the boundary: the freed bytes are admitted, one byte fewer is not.
    assert!(admit_remember(&core, "tenant-a", &third, &known, after_delete + third_len).is_ok());
    let refusal = admit_remember(&core, "tenant-a", &third, &known, after_delete + third_len - 1)
        .err()
        .unwrap();
    assert!(refusal.starts_with("MEMORY_TENANT_QUOTA_EXCEEDED"), "{refusal}");
    store(&core, "tenant-a", &third);
    assert_eq!(read_usage(&core, "tenant-a"), Some(after_delete + third_len));
    // The marking path of a forget rewrites the node, and the counter follows its new size; the
    // delete path then removes it and the counter drops by exactly the stored bytes.
    assert!(core.forget("memory:tenant-a:m:3", false));
    let marked = stored("memory:tenant-a:m:3");
    assert_eq!(read_usage(&core, "tenant-a"), Some(stored("memory:tenant-a:m:1") + marked));
    assert!(core.forget("memory:tenant-a:m:3", true));
    assert_eq!(read_usage(&core, "tenant-a"), Some(stored("memory:tenant-a:m:1")));
    // A removal through the node route moves it too; the last removal leaves zero.
    core.remove_node("memory:tenant-a:m:1".to_string());
    assert_eq!(read_usage(&core, "tenant-a"), Some(0));
}

// spec: MEDULLA-DECISION-ENGINE-R154.4
#[test]
fn remember_refuses_over_tenant_capacity() {
    let core = GraphCore::new();
    let first = remember(record("tenant-a", "m:1", MemoryTier::Semantic, "first"), vec![]);
    let second = remember(record("tenant-a", "m:2", MemoryTier::Semantic, "second"), vec![]);
    store(&core, "tenant-a", &first);
    store(&core, "tenant-a", &second);
    let used = read_usage(&core, "tenant-a").unwrap();
    let known = Default::default();

    // A write that lifts the total above the cap is refused with the registered code.
    let third = remember(record("tenant-a", "m:3", MemoryTier::Semantic, "third"), vec![]);
    let refusal = admit_remember(&core, "tenant-a", &third, &known, used)
        .err()
        .expect("the tenant is at its cap");
    assert!(refusal.starts_with("MEMORY_TENANT_QUOTA_EXCEEDED"), "{refusal}");

    // The refusal leaves the store unchanged: no node, no counter movement.
    assert!(core.get_node_properties("memory:tenant-a:m:3").is_none());
    assert_eq!(read_usage(&core, "tenant-a"), Some(used));

    // A second tenant, under the same cap, still writes.
    let other = remember(record("tenant-b", "m:1", MemoryTier::Semantic, "first"), vec![]);
    let other_len = node_blob("tenant-b", &other).unwrap().len() as u64;
    assert!(admit_remember(&core, "tenant-b", &other, &known, used.max(other_len)).is_ok());
}

// ---- MEDULLA-DECISION-ENGINE-R155: episodes and cross-session recall ----

use medulla_work_types::memory::{
    ConversationMessage, ConversationRecord, Episode, EpisodeScope, EpisodeSpec, MemoryRememberResult, MessageRole,
};

fn episodic(tenant: &str, id: &str, session: &str, timestamp_ms: i64) -> MemoryRecord {
    let mut item = record(tenant, id, MemoryTier::Episodic, "note of the session");
    item.session_id = Some(ResourceId::new(session).unwrap());
    item.timestamp_ms = timestamp_ms;
    item
}

fn episode_spec(episode_id: &str, session: Option<&str>, preceding: &[&str]) -> medulla_work_types::memory::EpisodeSpec {
    EpisodeSpec {
        episode_id: ResourceId::new(episode_id).unwrap(),
        session_id: session.map(|session| ResourceId::new(session).unwrap()),
        preceding: preceding.iter().map(|id| ResourceId::new(*id).unwrap()).collect(),
    }
}

/// Admit one remember and commit what it writes, as the served handler does.
fn commit(core: &GraphCore, tenant: &str, request: &MemoryRememberRequest) -> Result<MemoryRememberResult, String> {
    match admit(core, tenant, request, &Default::default())? {
        Admitted::Write(write, embed, answer) => {
            apply_batch(core, &write);
            if let Some(embed) = embed {
                let Method::AddEmbedding { node_id, embedding } = *embed else {
                    panic!("the vector write is an AddEmbedding");
                };
                core.semantic_store.write().add_embedding(node_id, embedding).unwrap();
            }
            Ok(answer)
        }
        Admitted::Embed(_, answer) | Admitted::Same(answer) => Ok(answer),
    }
}

fn with_episode(
    mut request: MemoryRememberRequest,
    spec: medulla_work_types::memory::EpisodeSpec,
) -> MemoryRememberRequest {
    request.episode = Some(spec);
    request
}

/// Remember the earlier records bare, then the last one with the episode spec.
fn seal(
    core: &GraphCore,
    tenant: &str,
    episode_id: &str,
    session: &str,
    stamps: &[(&str, i64)],
) -> MemoryRememberResult {
    let (last, earlier) = stamps.split_last().unwrap();
    for (id, at) in earlier {
        commit(core, tenant, &remember(episodic(tenant, id, session, *at), vec![])).unwrap();
    }
    let preceding: Vec<&str> = earlier.iter().map(|(id, _)| *id).collect();
    commit(
        core,
        tenant,
        &with_episode(
            remember(episodic(tenant, last.0, session, last.1), vec![]),
            episode_spec(episode_id, Some(session), &preceding),
        ),
    )
    .unwrap()
}

fn episode_recall(scope: EpisodeScope, as_of_ms: i64) -> MemoryRecallRequest {
    let mut request = recall_request(vec![], &["note"], 10_000);
    request.episode_scope = Some(scope);
    request.as_of_ms = Some(as_of_ms);
    request
}

fn contained(core: &GraphCore, episode_node: &str) -> usize {
    core.get_edges()
        .into_iter()
        .filter(|(source, target, _)| source == episode_node && target.starts_with("memory:"))
        .count()
}

// spec: MEDULLA-DECISION-ENGINE-R155.3
#[test]
fn remember_seals_an_episode_and_recall_filters_by_time() {
    let core = GraphCore::new();
    let sealed = seal(&core, "tenant-a", "ep:1", "s1", &[("e1", 100), ("e2", 200), ("e3", 300)]);
    let view = sealed.episode.expect("the remember reports the episode it sealed");
    assert_eq!((view.start_ms, view.end_ms, view.record_count), (100, 300, 3));
    assert_eq!(view.digest.len(), 64);
    // A later session's episode, ending after the as-of instant of the first recall.
    seal(&core, "tenant-a", "ep:2", "s2", &[("f1", 1_000)]);
    let mut live = record("tenant-a", "w1", MemoryTier::Working, "note held live");
    live.timestamp_ms = 50;
    commit(&core, "tenant-a", &remember(live, vec![])).unwrap();
    // The episode is one node with a CONTAINS edge per record.
    assert_eq!(contained(&core, "episode:tenant-a:ep:1"), 3);

    let episodes = recall(&core, "tenant-a", 5_000, &episode_recall(EpisodeScope::Episodes, 500), None).unwrap();
    assert_eq!(
        episodes.episodes.iter().map(|e| e.episode_id.as_str()).collect::<Vec<_>>(),
        ["ep:1"],
        "the episode ending after the as-of time is hidden"
    );
    let mut held = ids(&episodes.hits);
    held.sort();
    assert_eq!(held, ["e1", "e2", "e3"], "only records of visible episodes");

    let live_only = recall(&core, "tenant-a", 5_000, &episode_recall(EpisodeScope::Live, 500), None).unwrap();
    assert_eq!(ids(&live_only.hits), ["w1"]);
    assert!(live_only.episodes.is_empty());

    let both = recall(&core, "tenant-a", 5_000, &episode_recall(EpisodeScope::Both, 500), None).unwrap();
    assert_eq!(both.hits.len(), 4);

    let later = recall(&core, "tenant-a", 5_000, &episode_recall(EpisodeScope::Episodes, 2_000), None).unwrap();
    assert_eq!(later.episodes.len(), 2);

    // Another tenant sees no episode, and cannot seal one over this tenant's records.
    let foreign = recall(&core, "tenant-b", 5_000, &episode_recall(EpisodeScope::Both, 2_000), None).unwrap();
    assert!(foreign.episodes.is_empty() && foreign.hits.is_empty());
    let nodes = core.node_count();
    let rival = with_episode(
        remember(episodic("tenant-b", "g1", "s1", 400), vec![]),
        episode_spec("ep:rival", Some("s1"), &["e1"]),
    );
    let refusal = commit(&core, "tenant-b", &rival).err().unwrap();
    assert_eq!(refusal, absent_record("e1"));
    assert_eq!(core.node_count(), nodes);
}

// spec: MEDULLA-DECISION-ENGINE-R155.4
#[test]
fn episode_refusals() {
    let core = GraphCore::new();
    seal(&core, "tenant-a", "ep:1", "s1", &[("e1", 100), ("e2", 200)]);
    let nodes = core.node_count();
    let edges = core.get_edges().len();
    let refused = |request: MemoryRememberRequest| commit(&core, "tenant-a", &request).err().unwrap();

    // No session on the spec, and a record outside the spec's session.
    let no_session = with_episode(remember(episodic("tenant-a", "x1", "s9", 900), vec![]), episode_spec("ep:x", None, &[]));
    assert!(refused(no_session).starts_with("EPISODE_BOUNDARY_MISSING"));
    let other_session = with_episode(
        remember(episodic("tenant-a", "x2", "s9", 900), vec![]),
        episode_spec("ep:x", Some("s8"), &[]),
    );
    assert!(refused(other_session).starts_with("EPISODE_BOUNDARY_MISSING"));
    // A record without a timestamp has no place in an episode.
    let no_time = with_episode(
        remember(episodic("tenant-a", "x3", "s1", 0), vec![]),
        episode_spec("ep:y", Some("s1"), &[]),
    );
    assert!(refused(no_time).starts_with("EPISODE_ORDER_UNKNOWN"));
    // A window inside another episode of the session.
    let overlapping = with_episode(
        remember(episodic("tenant-a", "x4", "s1", 150), vec![]),
        episode_spec("ep:z", Some("s1"), &[]),
    );
    assert!(refused(overlapping).starts_with("EPISODE_OVERLAP"));
    // A sealed id written again with a different body.
    let again = with_episode(
        remember(episodic("tenant-a", "x5", "s1", 300), vec![]),
        episode_spec("ep:1", Some("s1"), &["e1", "e2"]),
    );
    assert!(refused(again).starts_with("EPISODE_IMMUTABLE"));

    // Every refusal came before any write.
    assert_eq!(core.node_count(), nodes);
    assert_eq!(core.get_edges().len(), edges);
}

// spec: MEDULLA-DECISION-ENGINE-R155.5
#[test]
fn recall_result_binds_episode_digests() {
    let core = GraphCore::new();
    seal(&core, "tenant-a", "ep:1", "s1", &[("e1", 100), ("e2", 200), ("e3", 300)]);
    seal(&core, "tenant-a", "ep:2", "s2", &[("f1", 400), ("f2", 500)]);
    let nodes = core.node_count();
    let result = recall(&core, "tenant-a", 9_000, &episode_recall(EpisodeScope::Episodes, 600), None).unwrap();
    assert_eq!((result.as_of_ms, result.episode_scope), (600, Some(EpisodeScope::Episodes)));
    assert_eq!(result.episodes.len(), 2);
    for view in &result.episodes {
        // The digest is the one the stored records seal to.
        let mut members: Vec<(i64, String)> = result
            .hits
            .iter()
            .filter(|hit| hit.record.session_id.as_ref() == Some(&view.session_id))
            .map(|hit| (hit.record.timestamp_ms, hit.record.memory_id.as_str().to_string()))
            .collect();
        members.sort();
        let again = Episode::seal(
            view.episode_id.as_str(),
            "tenant-a",
            view.session_id.as_str(),
            view.start_ms,
            view.end_ms,
            members.iter().map(|(_, id)| id.clone()).collect(),
        )
        .unwrap();
        assert_eq!(again.digest, view.digest);
        assert_eq!(usize::try_from(view.record_count).unwrap(), members.len());
        assert_eq!(members.first().unwrap().0, view.start_ms);
        assert_eq!(members.last().unwrap().0, view.end_ms);
    }
    // A read seals nothing.
    assert_eq!(core.node_count(), nodes);
}

// spec: MEDULLA-DECISION-ENGINE-R155.1, MEDULLA-DECISION-ENGINE-R155.2, MEDULLA-DECISION-ENGINE-R155.6
#[test]
fn later_session_recalls_earlier_episode_in_order() {
    let core = GraphCore::new();
    // Three sessions sealed out of order; a fourth ends in the future.
    seal(&core, "tenant-a", "ep:b", "s2", &[("b1", 2_000), ("b2", 2_100)]);
    seal(&core, "tenant-a", "ep:a", "s1", &[("a1", 1_000), ("a2", 1_100)]);
    seal(&core, "tenant-a", "ep:c", "s3", &[("c1", 3_000)]);
    seal(&core, "tenant-a", "ep:future", "s4", &[("d1", 90_000)]);
    let result = recall(&core, "tenant-a", 10_000, &episode_recall(EpisodeScope::Episodes, 5_000), None).unwrap();
    let order: Vec<&str> = result.episodes.iter().map(|e| e.episode_id.as_str()).collect();
    assert_eq!(order, ["ep:a", "ep:b", "ep:c"], "by start time, the future episode hidden");
    assert!(!ids(&result.hits).contains(&"d1"));

    // A sealed episode is immutable: the same write is found again, another body is refused.
    let replay = seal_again(&core, "ep:a", "s1", &[("a1", 1_000), ("a2", 1_100)]);
    assert!(!replay.stored, "nothing is written the second time");
    assert_eq!(replay.episode.unwrap().digest, result.episodes[0].digest);
    let changed = with_episode(
        remember(episodic("tenant-a", "a3", "s1", 1_200), vec![]),
        episode_spec("ep:a", Some("s1"), &["a1", "a2"]),
    );
    assert!(commit(&core, "tenant-a", &changed).err().unwrap().starts_with("EPISODE_IMMUTABLE"));

    // A session scope narrows the episodes to that session.
    let mut one = episode_recall(EpisodeScope::Episodes, 5_000);
    one.scope = Some(ResourceId::new("s2").unwrap());
    let narrowed = recall(&core, "tenant-a", 10_000, &one, None).unwrap();
    assert_eq!(narrowed.episodes.len(), 1);
    assert_eq!(narrowed.episodes[0].episode_id.as_str(), "ep:b");
}

/// The last write of `seal` again, against a store that already holds everything.
fn seal_again(core: &GraphCore, episode_id: &str, session: &str, stamps: &[(&str, i64)]) -> MemoryRememberResult {
    let (last, earlier) = stamps.split_last().unwrap();
    let preceding: Vec<&str> = earlier.iter().map(|(id, _)| *id).collect();
    commit(
        core,
        "tenant-a",
        &with_episode(
            remember(episodic("tenant-a", last.0, session, last.1), vec![]),
            episode_spec(episode_id, Some(session), &preceding),
        ),
    )
    .unwrap()
}

fn chat(tenant: &str, turns: &[(&str, MessageRole, i64)]) -> ConversationRecord {
    ConversationRecord::new(
        tenant,
        "chat:1",
        turns
            .iter()
            .map(|(id, role, at)| ConversationMessage {
                message_id: ResourceId::new(*id).unwrap(),
                role: *role,
                provenance_key: ResourceId::new(format!("src:{id}")).unwrap(),
                timestamp_ms: *at,
                text: format!("note {id}"),
            })
            .collect(),
    )
    .unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R617.3
#[test]
fn source_ingest_transcript_becomes_episodic_memory() {
    const MINUTE: i64 = 60_000;
    let core = GraphCore::new();
    // Two runs of turns an hour apart, and a tool turn between them.
    let thread = chat(
        "tenant-a",
        &[
            ("m1", MessageRole::User, MINUTE),
            ("m2", MessageRole::Assistant, 2 * MINUTE),
            ("m3", MessageRole::Tool, 3 * MINUTE),
            ("m4", MessageRole::User, 90 * MINUTE),
            ("m5", MessageRole::Assistant, 91 * MINUTE),
        ],
    );
    let embeddings = vec![vec![1.0, 0.0]; 5];
    let requests = super::memory_ingest::thread_requests(&thread, embeddings.clone(), 30 * MINUTE).unwrap();
    assert_eq!(requests.len(), 5);
    let sealing: Vec<_> = requests.iter().filter_map(|request| request.episode.as_ref()).collect();
    assert_eq!(sealing.len(), 1, "the closed run seals; the open run stays open");
    assert_eq!(sealing[0].preceding.len(), 1);
    for request in &requests {
        commit(&core, "tenant-a", request).unwrap();
    }
    // Replaying the thread writes nothing new.
    let nodes = core.node_count();
    for request in &requests {
        commit(&core, "tenant-a", request).unwrap();
    }
    assert_eq!(core.node_count(), nodes);

    let mut ask = recall_request(vec![], &["note"], 10_000);
    ask.episode_scope = Some(EpisodeScope::Episodes);
    let recalled = recall(&core, "tenant-a", 200 * MINUTE, &ask, None).unwrap();
    assert_eq!(recalled.episodes.len(), 1);
    assert_eq!(recalled.episodes[0].record_count, 2);
    let mut held = ids(&recalled.hits);
    held.sort();
    assert_eq!(held, ["chat:1:m1", "chat:1:m2"]);
    // Without the episode rule every turn is recallable; the tool turn is procedural.
    let all = recall(&core, "tenant-a", 200 * MINUTE, &recall_request(vec![], &["note"], 10_000), None).unwrap();
    assert_eq!(all.hits.len(), 5);
    let tool = all.hits.iter().find(|hit| hit.record.memory_id.as_str() == "chat:1:m3").unwrap();
    assert_eq!(tool.record.tier, MemoryTier::Procedural);
    assert!(all.hits.iter().all(|hit| hit.record.session_id.as_ref().map(ResourceId::as_str) == Some("chat:1")));

    // Another tenant recalls none of it.
    let foreign = recall(&core, "tenant-b", 200 * MINUTE, &ask, None).unwrap();
    assert!(foreign.hits.is_empty() && foreign.episodes.is_empty());
}
