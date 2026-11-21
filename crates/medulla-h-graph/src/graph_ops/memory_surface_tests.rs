//! Served memory surface proofs (MEDULLA-DECISION-ENGINE-R651.1 .. R651.3).
//! They run the admission, recall and export rules over one graph holding two
//! tenants, and check how the three methods are routed.

use super::memory_surface::{absent_record, admit_remember, export, promotion_support_refusal, recall, Admitted};
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use medulla_server_mutation::mutation::{apply_coalescable_write, is_gateway_routed};
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

fn remember(record: MemoryRecord, embedding: Vec<f32>) -> MemoryRememberRequest {
    MemoryRememberRequest { record, embedding }
}

fn store(core: &GraphCore, tenant: &str, request: &MemoryRememberRequest) {
    let Admitted::Write(write, embed, answer) = admit_remember(core, tenant, request, &Default::default()).unwrap()
    else {
        panic!("a first remember writes");
    };
    assert!(answer.stored);
    apply_coalescable_write(core, &write).unwrap();
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
    let refusal = admit_remember(&core, "tenant-b", &foreign, &Default::default()).err().unwrap();
    assert!(refusal.starts_with("PARAMETER_INVALID"), "{refusal}");
    assert!(!refusal.contains("ACCESS_DENIED") && !refusal.to_lowercase().contains("tenant"), "{refusal}");
    assert_eq!(core.node_count(), 0);
    // Byte for byte the answer a write naming a nonexistent record receives.
    assert_eq!(refusal, absent_record("m:1"));

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
    assert!(matches!(admit_remember(&core, "tenant-a", &same, &Default::default()), Ok(Admitted::Same(_))));
    let mut changed = sky.clone();
    changed.text = "the sky is green".into();
    assert!(admit_remember(&core, "tenant-a", &remember(changed, vec![1.0, 0.0]), &Default::default()).is_err());

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
    assert_eq!(exported.manifest.manifest_digest.len(), 64);
    assert!(!exported.manifest.truncated);
    let limited = export(&core, "tenant-a", &MemoryExportRequest { limit: Some(1) }).unwrap();
    assert_eq!(limited.records.len(), 100.min(exported.records.len()));
}

// spec: MEDULLA-DECISION-ENGINE-R154.2
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
        let vector = recall(&core, "tenant-a", NOW, &by_vector).unwrap();
        let keyword = recall(&core, "tenant-a", NOW, &by_keyword).unwrap();
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
    let refusal = admit_remember(&core, "tenant-a", &remember(image.clone(), vec![0.0, 1.0]), &Default::default())
        .err()
        .unwrap();
    assert!(refusal.starts_with("MEDIA_BLOB_UNKNOWN"), "{refusal}");
    assert_eq!(core.node_count(), 0);

    let known: std::collections::BTreeSet<String> = [digest].into_iter().collect();
    let request = remember(image, vec![0.0, 1.0]);
    let Admitted::Write(write, embed, _) = admit_remember(&core, "tenant-a", &request, &known).unwrap() else {
        panic!("a first remember writes");
    };
    apply_coalescable_write(&core, &write).unwrap();
    let Method::AddEmbedding { node_id, embedding } = *embed.unwrap() else {
        panic!("the vector write is an AddEmbedding");
    };
    core.semantic_store.write().add_embedding(node_id, embedding).unwrap();
    // The caption embedding sits on its derivation node.
    core.semantic_store.write().add_embedding(derivation, vec![1.0, 0.0]).unwrap();

    // Without `include_media` the caption vector does not find the record.
    let query = recall_request(vec![1.0, 0.0], &[], 100);
    assert!(recall(&core, "tenant-a", NOW, &query).unwrap().hits.is_empty());
    let mut media = query.clone();
    media.include_media = true;
    let hits = recall(&core, "tenant-a", NOW, &media).unwrap().hits;
    assert_eq!(ids(&hits), ["m:image"]);
    assert_eq!(hits[0].record.attachments.len(), 1);
    // Another tenant sees nothing, and `include_media` needs a query embedding.
    assert!(recall(&core, "tenant-b", NOW, &media).unwrap().hits.is_empty());
    let mut keywords_only = recall_request(vec![], &["photo"], 100);
    keywords_only.include_media = true;
    assert!(recall(&core, "tenant-a", NOW, &keywords_only)
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
    let refusal = admit_remember(&core, "tenant-a", &attach(limit + 1), &known).err().unwrap();
    assert!(refusal.starts_with("MEDIA_TOO_LARGE"), "{refusal}");
    assert_eq!(core.node_count(), 0);
    // A blob at the limit passes the size check.
    assert!(admit_remember(&core, "tenant-a", &attach(limit), &known).is_ok());
    // A digest the owner scope does not hold is refused before the size is read.
    let refusal = admit_remember(&core, "tenant-a", &attach(limit + 1), &Default::default())
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
    let Admitted::Write(write, _, _) = admit_remember(&core, "tenant-a", &request, &known).unwrap() else {
        panic!("a first remember writes");
    };
    apply_coalescable_write(&core, &write).unwrap();

    // The attachment keys ride the memory node, whose id is the citing memory id.
    let blob = core.get_node_properties("memory:tenant-a:m:evidence").unwrap();
    let props = medulla_types::msgpack::decode_property_object(&blob).unwrap();
    let attachment = &props["attachments"].as_array().unwrap()[0];
    assert_eq!(attachment["blob_digest"], digest.as_str());
    assert_eq!(attachment["mime"], "image/png");
    assert_eq!(attachment["byte_len"], 4096);
    assert_eq!(attachment["derivation_ids"][0], derivation.as_str());
    // The same record is readable under its own tenant only.
    assert!(recall(&core, "tenant-b", NOW, &recall_request(vec![], &["chart"], 100))
        .unwrap()
        .hits
        .is_empty());
    let own = recall(&core, "tenant-a", NOW, &recall_request(vec![], &["chart"], 100)).unwrap();
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
    let plain = recall(&core, "tenant-a", NOW, &asked).unwrap();
    assert_eq!(plain.plan_digest.len(), 64);
    assert!(plain.drift.is_none());
    assert_eq!(recall(&core, "tenant-a", NOW, &asked).unwrap().plan_digest, plain.plan_digest);
    let mut narrower = asked.clone();
    narrower.scope = Some(ResourceId::new("sess:1").unwrap());
    assert_ne!(recall(&core, "tenant-a", NOW, &narrower).unwrap().plan_digest, plain.plan_digest);

    asked.previous_manifest_digest = Some(digest.clone());
    let same = recall(&core, "tenant-a", NOW, &asked).unwrap();
    assert_eq!(same.plan_digest, plain.plan_digest, "the comparison input is not part of the plan");
    let report = same.drift.expect("a previous digest was supplied");
    assert!(!report.drifted);
    assert_eq!(report.manifest_digest, digest);
    asked.previous_manifest_digest = Some(changed.manifest.manifest_digest.clone());
    let drifted = recall(&core, "tenant-a", NOW, &asked).unwrap().drift.expect("a report");
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
    let refusal = admit_remember(&core, "tenant-b", &foreign, &Default::default()).err().unwrap();
    assert!(refusal.starts_with("PARAMETER_INVALID"), "{refusal}");
    assert_eq!(refusal, absent_record("m:1"), "a foreign tenant's write is an existence oracle");
    assert!(!refusal.to_lowercase().contains("tenant"), "{refusal}");
    assert_eq!(core.node_count(), 0);

    // A record whose stored encoding is above the knob default of 65536 bytes is refused.
    let huge = remember(
        record("tenant-a", "m:big", MemoryTier::Semantic, &"x".repeat(70_000)),
        vec![],
    );
    let refusal = admit_remember(&core, "tenant-a", &huge, &Default::default()).err().unwrap();
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
