// MEDULLA-FEDERATED-QUERY-R708.12: reading, listing and retiring stored queries through the
// served dispatch. A missing name, a name outside the caller's share scope and a retired name
// all answer the same refusal, the list is cursor-paged and bound to its tenant, and a retired
// name keeps its versions but is never served or published again.

use medulla_identity_types::identity::ShareScope;
use medulla_protocol_types::protocol::{Method, Response};
use medulla_source_types::stored_query::{
    RefreshPolicy, StoredParamType, StoredQuery, StoredQueryLanguage, StoredQueryPage,
    StoredQueryRetireReason, StoredQueryRetired, StoredQueryView,
};

use super::connector_pack_harness::{ok, Served, GRAPH, TENANT};

const CONNECTOR: &str = "stored-query-source";
const STREAM: &str = "paper";
const TEXT: &str = "MATCH (:StoredIncident) WHERE status = $status";
const OTHER: &str = "usr:other-reader";

fn stored(name: &str, version: u32, share: ShareScope) -> StoredQuery {
    StoredQuery {
        name: name.to_string(),
        version,
        language: StoredQueryLanguage::Uql,
        text: TEXT.to_string(),
        params: [("status".to_string(), StoredParamType::String)].into(),
        materialize: false,
        refresh: RefreshPolicy::OnChange,
        author: String::new(),
        share,
    }
}

async fn put(served: &Served, query: StoredQuery) {
    let key = format!("put-{}-{}", query.name, query.version);
    let response = served
        .call_on(served.graph(), &key, Method::StoredQueryPut { query })
        .await;
    assert!(response.error.is_none(), "StoredQueryPut: {response:?}");
}

/// As the reader `OTHER`, who is outside every private share scope.
async fn as_other(served: &Served, method: Method) -> Response {
    served.verified_as(OTHER, TENANT, &["kg:read"], method).await
}

fn refusal_text(response: &Response) -> String {
    format!(
        "{} {}",
        response.error.clone().unwrap_or_default(),
        response.error_detail.clone().unwrap_or_default()
    )
}

fn get(name: &str, version: Option<u32>) -> Method {
    Method::StoredQueryGet {
        name: name.to_string(),
        version,
    }
}

fn list(limit: Option<u32>, cursor: Option<String>, include_retired: bool) -> Method {
    Method::StoredQueryList {
        limit,
        cursor,
        author: None,
        include_retired,
    }
}

fn names(page: &StoredQueryPage) -> Vec<&str> {
    page.queries.iter().map(|entry| entry.name.as_str()).collect()
}

// spec: MEDULLA-FEDERATED-QUERY-R708.12
#[tokio::test]
async fn get_and_list_hide_a_private_query_from_another_principal_exactly_like_a_missing_one() {
    let served = Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await;
    served.register_reader(OTHER).await;
    put(&served, stored("alpha_private", 1, ShareScope::Private)).await;
    put(&served, stored("beta_shared", 1, ShareScope::Tenant)).await;
    put(&served, stored("beta_shared", 2, ShareScope::Tenant)).await;

    // The author reads the latest version as written, and a named version.
    let latest: StoredQueryView = ok(
        "StoredQueryGet latest",
        served.verified(TENANT, &["kg:read"], get("beta_shared", None)).await,
    );
    assert_eq!((latest.version, latest.text.as_str()), (2, TEXT));
    assert!(latest.created_at_ms.is_some(), "a put stamps its publication time");
    let first: StoredQueryView = ok(
        "StoredQueryGet v1",
        served.verified(TENANT, &["kg:read"], get("beta_shared", Some(1))).await,
    );
    assert_eq!(first.version, 1);

    // Another principal reads the shared one and is refused the private one with the very text a
    // missing name gets.
    let shared: StoredQueryView = ok(
        "StoredQueryGet shared as other",
        as_other(&served, get("beta_shared", None)).await,
    );
    assert_eq!(shared.name, "beta_shared");
    let foreign = refusal_text(&as_other(&served, get("alpha_private", None)).await);
    let missing = refusal_text(&as_other(&served, get("absent_query", None)).await);
    assert!(foreign.contains("STORED_QUERY_NOT_FOUND"), "{foreign}");
    assert_eq!(
        foreign.replace("alpha_private", "<name>"),
        missing.replace("absent_query", "<name>"),
        "a foreign name and a missing name must refuse alike"
    );
    let unpublished = refusal_text(&as_other(&served, get("beta_shared", Some(9))).await);
    assert!(unpublished.contains("STORED_QUERY_NOT_FOUND"), "{unpublished}");

    // The list shows the other principal only what it may call; the author sees both.
    let other_page: StoredQueryPage = ok(
        "StoredQueryList as other",
        as_other(&served, list(None, None, false)).await,
    );
    assert_eq!(names(&other_page), vec!["beta_shared"]);
    assert_eq!(other_page.queries[0].latest_version, 2);
    let author_page: StoredQueryPage = ok(
        "StoredQueryList as author",
        served.verified(TENANT, &["kg:read"], list(None, None, false)).await,
    );
    assert_eq!(names(&author_page), vec!["alpha_private", "beta_shared"]);
}

// spec: MEDULLA-FEDERATED-QUERY-R708.12
#[tokio::test]
async fn list_pages_by_cursor_in_name_order_and_refuses_a_foreign_cursor() {
    let served = Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await;
    for name in ["q_a", "q_b", "q_c"] {
        put(&served, stored(name, 1, ShareScope::Tenant)).await;
    }
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page: StoredQueryPage = ok(
            "StoredQueryList page",
            served
                .verified(TENANT, &["kg:read"], list(Some(2), cursor.clone(), false))
                .await,
        );
        seen.extend(page.queries.iter().map(|entry| entry.name.clone()));
        pages += 1;
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, vec!["q_a", "q_b", "q_c"]);
    assert_eq!(pages, 2);

    // A cursor minted for this tenant does not resume another tenant's listing (the cursor
    // binding itself is proved at L1), and a limit outside its bound is refused.
    let minted = ok::<StoredQueryPage>(
        "StoredQueryList first page",
        served.verified(TENANT, &["kg:read"], list(Some(1), None, false)).await,
    )
    .next_cursor
    .expect("two names remain");
    let transplanted = served
        .verified("tenant-other", &["kg:read"], list(None, Some(minted), false))
        .await;
    assert!(transplanted.error.is_some(), "{transplanted:?}");
    let zero = served.verified(TENANT, &["kg:read"], list(Some(0), None, false)).await;
    assert!(refusal_text(&zero).contains("STORED_QUERY_INVALID"), "{zero:?}");
}

// spec: MEDULLA-FEDERATED-QUERY-R708.12
#[tokio::test]
async fn retire_is_a_tombstone_only_the_author_can_write() {
    let served = Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await;
    served.register_reader(OTHER).await;
    put(&served, stored("old_report", 1, ShareScope::Tenant)).await;
    put(&served, stored("keeper", 1, ShareScope::Tenant)).await;

    // Another principal cannot retire it, and the refusal changes nothing.
    let foreign = as_other(
        &served,
        Method::StoredQueryRetire {
            name: "old_report".to_string(),
            reason: StoredQueryRetireReason::Superseded,
        },
    )
    .await;
    assert!(foreign.error.is_some(), "{foreign:?}");
    let still: StoredQueryView = ok(
        "StoredQueryGet after a foreign retire",
        served.verified(TENANT, &["kg:read"], get("old_report", None)).await,
    );
    assert_eq!(still.name, "old_report");

    // The author retires it with a typed reason; a second retire answers the same tombstone.
    let retire = |key: &'static str| {
        served.call_on(
            served.graph(),
            key,
            Method::StoredQueryRetire {
                name: "old_report".to_string(),
                reason: StoredQueryRetireReason::Superseded,
            },
        )
    };
    let retired: StoredQueryRetired = ok("StoredQueryRetire", retire("retire-1").await);
    assert!(retired.retired);
    assert_eq!(retired.tombstone.reason, StoredQueryRetireReason::Superseded);
    let again: StoredQueryRetired = ok("StoredQueryRetire again", retire("retire-2").await);
    assert_eq!(again.tombstone, retired.tombstone);

    // The name is no longer served to anyone: not read, not called, not published again, and
    // left out of the list unless retired names are asked for.
    let gone = refusal_text(&served.verified(TENANT, &["kg:read"], get("old_report", None)).await);
    assert!(gone.contains("STORED_QUERY_NOT_FOUND"), "{gone}");
    let call = served
        .verified(
            TENANT,
            &["kg:read"],
            Method::StoredQueryCall {
                name: "old_report".to_string(),
                version: None,
                args: [("status".to_string(), serde_json::json!("open"))].into(),
            },
        )
        .await;
    assert!(refusal_text(&call).contains("STORED_QUERY_NOT_FOUND"), "{call:?}");
    let republish = served
        .call_on(
            served.graph(),
            "put-old-report-2",
            Method::StoredQueryPut {
                query: stored("old_report", 2, ShareScope::Tenant),
            },
        )
        .await;
    assert!(
        refusal_text(&republish).contains("STORED_QUERY_IMMUTABLE"),
        "{republish:?}"
    );
    let live: StoredQueryPage = ok(
        "StoredQueryList live",
        served.verified(TENANT, &["kg:read"], list(None, None, false)).await,
    );
    assert_eq!(names(&live), vec!["keeper"]);
    let with_retired: StoredQueryPage = ok(
        "StoredQueryList with retired",
        served.verified(TENANT, &["kg:read"], list(None, None, true)).await,
    );
    assert_eq!(names(&with_retired), vec!["keeper", "old_report"]);
    let tombstone = with_retired.queries[1]
        .retired
        .as_ref()
        .expect("the retired name carries its tombstone");
    assert_eq!(tombstone.reason, StoredQueryRetireReason::Superseded);
}
