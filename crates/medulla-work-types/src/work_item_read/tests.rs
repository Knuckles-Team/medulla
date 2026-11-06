use super::*;
use serde_json::json;

fn row(tenant: &str, kind: &str, status: &str) -> Map<String, Value> {
    json!({
        "node_type": "WorkItem",
        "tenant": tenant,
        "kind": kind,
        "status": status,
        "payload_ref": "au-task:sha256:abc",
        "metadata": {"au:description": "summarize"},
        "row_revision": 3,
        "updated_at": 12.3456,
        "lease_owner": "worker-a",
        "lease_epoch": 4,
        "fencing_token": 9,
    })
    .as_object()
    .cloned()
    .expect("fixture row is an object")
}

fn list(limit: u32, cursor: Option<String>, kind: Option<&str>) -> WorkItemListRequest {
    WorkItemListRequest {
        tenant: "tenant-a".to_string(),
        cursor,
        limit,
        kind: kind.map(str::to_string),
        metadata_match: None,
    }
}

/// Drive one page the way the store does: skip the cursor row, check the
/// bounds before each row, consume, close.
fn page(rows: &[(String, Map<String, Value>)], request: &WorkItemListRequest) -> WorkItemPage {
    let resume_after = request.resume_after().expect("cursor decodes");
    let mut scan = medulla_types::keyset_page::KeysetScan::new(request);
    for (id, props) in rows {
        if resume_after.as_deref() >= Some(id.as_str()) {
            continue;
        }
        if !scan.admits_another_row() {
            break;
        }
        scan.consume(id, 100, props).expect("row projects");
    }
    scan.finish().into()
}

fn mixed_rows() -> Vec<(String, Map<String, Value>)> {
    let mut rows = Vec::new();
    for index in 0..7 {
        let tenant = if index % 2 == 0 {
            "tenant-a"
        } else {
            "tenant-b"
        };
        rows.push((format!("wi-{index}"), row(tenant, "au.task", "ready")));
    }
    rows.push(("wi-7".to_string(), row("tenant-a", "other", "running")));
    rows
}

#[test]
fn the_view_carries_no_lease_authority_and_projects_revision_and_time() {
    let view =
        WorkItemView::from_tenant_row("wi-1", &row("tenant-a", "au.task", "leased"), "tenant-a")
            .unwrap()
            .expect("own tenant's WorkItem is visible");
    assert_eq!(view.status, WorkItemStatus::Leased);
    assert_eq!(view.version, 3);
    assert_eq!(view.updated_at_ms, 12_346);
    assert_eq!(view.input_ref, "au-task:sha256:abc");
    let wire = serde_json::to_value(&view).unwrap();
    let mut keys: Vec<&str> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "input_ref",
            "kind",
            "metadata",
            "status",
            "updated_at_ms",
            "version",
            "work_item_id"
        ]
    );
}

#[test]
fn another_tenants_row_and_a_non_work_item_are_invisible() {
    let foreign = row("tenant-b", "au.task", "ready");
    assert_eq!(
        WorkItemView::from_tenant_row("wi-1", &foreign, "tenant-a").unwrap(),
        None
    );
    let mut plain = row("tenant-a", "au.task", "ready");
    plain.insert("node_type".into(), json!("Document"));
    assert_eq!(
        WorkItemView::from_tenant_row("n-1", &plain, "tenant-a").unwrap(),
        None
    );
    let legacy = row("tenant-a", "au.task", "completed");
    assert!(WorkItemView::from_tenant_row("wi-1", &legacy, "tenant-a").is_err());
}

#[test]
fn a_row_without_a_revision_reads_as_revision_one() {
    let mut unrevised = row("tenant-a", "au.task", "ready");
    unrevised.remove(WORK_ITEM_ROW_REVISION);
    let view = WorkItemView::from_tenant_row("wi-1", &unrevised, "tenant-a")
        .unwrap()
        .unwrap();
    assert_eq!(view.version, 1);
}

#[test]
fn paging_walks_every_own_row_exactly_once_and_ends_without_a_cursor() {
    let rows = mixed_rows();
    let mut seen = Vec::new();
    let mut cursor = None;
    for _ in 0..rows.len() {
        let current = page(&rows, &list(2, cursor.take(), None));
        seen.extend(current.items.into_iter().map(|item| item.work_item_id));
        cursor = current.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(cursor, None);
    assert_eq!(seen, ["wi-0", "wi-2", "wi-4", "wi-6", "wi-7"]);
}

#[test]
fn a_kind_filter_selects_within_the_same_scan() {
    let only_other = page(&mixed_rows(), &list(100, None, Some("other")));
    let ids: Vec<&str> = only_other
        .items
        .iter()
        .map(|item| item.work_item_id.as_str())
        .collect();
    assert_eq!(ids, ["wi-7"]);
    assert_eq!(only_other.next_cursor, None);
}

#[test]
fn a_cursor_is_bound_to_the_tenant_it_was_minted_for() {
    let first = page(&mixed_rows(), &list(1, None, None));
    let cursor = first.next_cursor.expect("a limited page resumes");
    let mut foreign = list(1, Some(cursor), None);
    foreign.tenant = "tenant-b".to_string();
    let error = foreign.resume_after().unwrap_err();
    assert_eq!(error, "WorkItem list cursor was not minted for this tenant");
}

#[test]
fn the_scan_bound_closes_an_empty_page_with_a_cursor() {
    let rows: Vec<(String, Map<String, Value>)> = (0
        ..medulla_types::keyset_page::MAX_KEYSET_PAGE_SCAN + 1)
        .map(|index| (format!("n-{index:05}"), row("tenant-b", "au.task", "ready")))
        .collect();
    let first = page(&rows, &list(100, None, None));
    assert!(first.items.is_empty());
    let cursor = first.next_cursor.expect("an exhausted scan bound resumes");
    let second = page(&rows, &list(100, Some(cursor), None));
    assert!(second.items.is_empty());
    assert_eq!(second.next_cursor, None);
}

#[test]
fn request_bounds_are_enforced() {
    assert!(list(0, None, None).validate().is_err());
    assert!(list(MAX_WORK_ITEM_LIST_LIMIT + 1, None, None)
        .validate()
        .is_err());
    assert!(list(MAX_WORK_ITEM_LIST_LIMIT, None, Some(" "))
        .validate()
        .is_err());
    list(MAX_WORK_ITEM_LIST_LIMIT, None, Some("au.task"))
        .validate()
        .unwrap();
    assert!(validate_work_item_get("tenant-a", "").is_err());
    assert!(validate_work_item_get("", "wi-1").is_err());
    validate_work_item_get("tenant-a", "wi-1").unwrap();
}

#[test]
fn a_metadata_match_selects_exact_top_level_pairs_and_is_bounded() {
    let mut rows = mixed_rows();
    rows[2]
        .1
        .insert("metadata".into(), json!({"correlation_id": "c-9", "x": 1}));
    let mut request = list(100, None, None);
    request.metadata_match = json!({"correlation_id": "c-9"}).as_object().cloned();
    request.validate().unwrap();
    let ids: Vec<String> = page(&rows, &request)
        .items
        .into_iter()
        .map(|item| item.work_item_id)
        .collect();
    assert_eq!(ids, ["wi-2"]);

    request.metadata_match = Some(Map::new());
    assert!(request.validate().is_err());
    let too_many: Map<String, Value> = (0..=medulla_types::keyset_page::MAX_PAIR_FILTER_KEYS)
        .map(|index| (format!("k{index}"), json!(index)))
        .collect();
    request.metadata_match = Some(too_many);
    assert!(request.validate().is_err());
}

#[test]
fn effect_records_round_trip_with_golden_digest() {
    let mut stored = row("tenant-a", "agent_turn", "running");
    stored.insert("effect_phase".into(), json!("started"));
    let view = WorkItemView::from_tenant_row("wi-1", &stored, "tenant-a")
        .expect("projects")
        .expect("visible");
    assert_eq!(view.effect_phase, EffectPhase::Started);
    let back: WorkItemView =
        serde_json::from_value(serde_json::to_value(&view).expect("ser")).expect("de");
    assert_eq!(back, view);
    assert!(NATIVE_WORK_ITEM_ROW_KEYS.contains(&WORK_ITEM_EFFECT_PHASE));
    for phase in [
        EffectPhase::Unstarted,
        EffectPhase::Started,
        EffectPhase::Resolved,
    ] {
        let mut r = Map::new();
        r.insert(WORK_ITEM_EFFECT_PHASE.into(), json!(phase.as_stored()));
        assert_eq!(EffectPhase::from_row(&r), Ok(phase));
    }
    assert_eq!(
        EffectPhase::from_row(&Map::new()),
        Ok(EffectPhase::Unstarted)
    );

    let canonical = br#"{"op":"charge","amount":5}"#;
    let request_ref = AdmittedRequest::request_ref_of(canonical);
    assert_eq!(
        request_ref,
        "sha256:1b52805c1a1c1c410c4e7843cc048a189d1cc1aa34138f5bf95ebbcf0b704634"
    );
    let request = AdmittedRequest {
        request_ref: request_ref.clone(),
        tenant: "tenant-a".into(),
        session_ref: "session-1".into(),
        payload_digest: request_ref.clone(),
        bytes: canonical.to_vec(),
    };
    let receipt = EffectReceipt {
        job_id: "wi-1".into(),
        request_ref,
        principal_ref: "principal-a".into(),
        outcome: EffectOutcome::NotApplied,
        receipt_ref: "receipt-1".into(),
        committed_at: 1_700_000_000_000,
    };
    let receipt_json = serde_json::to_value(&receipt).expect("ser");
    assert_eq!(receipt_json["outcome"], json!("not_applied"));
    assert_eq!(
        serde_json::from_value::<EffectReceipt>(receipt_json).expect("de"),
        receipt
    );
    let request_back: AdmittedRequest =
        serde_json::from_value(serde_json::to_value(&request).expect("ser")).expect("de");
    assert_eq!(request_back, request);
}

// spec: MEDULLA-DURABLE-KERNEL-R820.4
#[test]
fn admitted_request_refusals_are_registered_codes() {
    let bytes = b"{\"call\":\"send\"}".to_vec();
    let request = AdmittedRequest {
        request_ref: AdmittedRequest::request_ref_of(&bytes),
        tenant: "t".into(),
        session_ref: "s".into(),
        payload_digest: "blob:p".into(),
        bytes: bytes.clone(),
    };
    let record = request.admit().expect("admits");
    assert_eq!(record.request_ref, request.request_ref);
    assert_eq!(record.payload_ref, "blob:p");

    let tampered = AdmittedRequest {
        bytes: b"{\"call\":\"drop\"}".to_vec(),
        ..request.clone()
    };
    let error = tampered.admit().unwrap_err();
    assert!(
        error.starts_with("ADMITTED_REQUEST_DIGEST_MISMATCH:"),
        "{error}"
    );

    let big = vec![0u8; MAX_ADMITTED_REQUEST_BYTES + 1];
    let oversized = AdmittedRequest {
        request_ref: AdmittedRequest::request_ref_of(&big),
        bytes: big,
        ..request
    };
    let error = oversized.admit().unwrap_err();
    assert!(error.starts_with("ADMITTED_REQUEST_TOO_LARGE:"), "{error}");
}
