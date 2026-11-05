//! The Iceberg REST catalog: what a request addresses, and the operation each
//! address answers.
//!
//! [`super`] owns the listener, the bearer/carrier boundary and the response
//! vocabulary; this module owns routing and the operations themselves. Before
//! the split they were one `handle` of cyclomatic 37 / cognitive 31, in which
//! every route re-derived the caller's visibility and four of them re-emitted
//! the same audit event by hand.

use medulla_catalog_types::lake_catalog::ProjectionAccess;
use medulla_lake::catalog::{refuse_projection_write, CatalogOperation, TableEntry};
use serde_json::{json, Value};

use super::super::{namespace_levels, LakeManager, LakeVisibility};
use super::{
    carrier_denied_response, err_body, not_found, operation_scope, parse_as_of_lsn,
    parse_pagination, percent_decode, scope_authorized, table_ident_namespace, visibility_for,
};
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::http1::HttpMessage;

/// Who the caller is for every routed operation: what it may see, and the
/// owner label its audit events carry.
struct RequestScope {
    visibility: LakeVisibility,
    owner: Option<String>,
}

impl RequestScope {
    /// The scope a verified carrier grants, or the anonymous public view.
    fn of(carrier: Option<&CarrierAuthority>) -> Self {
        Self {
            visibility: visibility_for(carrier),
            owner: carrier.map(|c| c.owner_scope().to_string()),
        }
    }

    /// The `owner` field of an audit event: the caller's scope, or `system`.
    fn owner_label(&self) -> String {
        self.owner.clone().unwrap_or_else(|| "system".to_string())
    }
}

/// Record one operation's audit event. Four routes emitted the same five
/// fields around their own subject identity, so the SHAPE lives here and a new
/// route cannot omit a field. The allow/deny verdict stays with the caller
/// because the three derivations genuinely differ — CreateTable and
/// CommitTable succeed with `200 OK`, RenameTable with `204 No Content`, and
/// DropTable reports whether the drop happened at all.
fn record_operation(
    lake: &LakeManager,
    op: &str,
    subject: &[(&str, Value)],
    owner: &str,
    status: &str,
    allowed: bool,
) {
    let mut event = serde_json::Map::new();
    event.insert("ts_ms".to_string(), json!(crate::lake::lineage::now_ms()));
    event.insert("op".to_string(), json!(op));
    for (key, value) in subject {
        event.insert((*key).to_string(), value.clone());
    }
    event.insert("owner".to_string(), json!(owner));
    event.insert(
        "outcome".to_string(),
        json!(if allowed { "allow" } else { "deny" }),
    );
    event.insert("status".to_string(), json!(status));
    lake.record_audit(Value::Object(event));
}

/// Route + execute one Iceberg REST request. The two guards that precede
/// routing are the browser-origin refusal and the carrier's own scope claim;
/// the scope check runs BEFORE body parsing and existence lookups deliberately
/// (NE-048 P0), so an unauthorized caller learns nothing about whether the
/// target namespace or table even exists.
pub(super) fn handle(
    lake: &LakeManager,
    req: &HttpMessage,
    carrier: Option<&CarrierAuthority>,
) -> (&'static str, String) {
    if !req.header("origin").is_empty() {
        return (
            "403 Forbidden",
            err_body("browser origin denied", "ForbiddenException", 403),
        );
    }
    let (path, _) = req.path_and_query();
    let segs: Vec<String> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .collect();
    let seg_refs: Vec<&str> = segs.iter().map(String::as_str).collect();
    let scope = RequestScope::of(carrier);

    if !scope_authorized(carrier, operation_scope(req.method.as_str(), &seg_refs)) {
        // A denial is not an operation outcome: it carries no status of its own
        // and names the request that was refused, not a namespace or table.
        lake.record_audit(json!({
            "ts_ms": crate::lake::lineage::now_ms(),
            "op": "InsufficientScope",
            "method": req.method,
            "path": req.target,
            "owner": scope.owner_label(),
            "outcome": "deny",
        }));
        return carrier_denied_response();
    }

    if let Some(refusal) = guard_projection_write(lake, req, &seg_refs, &scope) {
        return refusal;
    }

    catalog_route(lake, req, &seg_refs, &scope)
}

/// Refuse every catalog write: the lake registers no authoritative table, so
/// each POST, PUT, PATCH and DELETE is classified into a
/// [`CatalogOperation`] and run through `refuse_projection_write` before any
/// `LakeManager` call (MEDULLA-FEDERATED-QUERY-R080.3). A table the caller
/// cannot see keeps the privacy-safe 404 (R080.4); every refusal leaves an
/// audit record (R080.5).
fn guard_projection_write(
    lake: &LakeManager,
    req: &HttpMessage,
    segs: &[&str],
    scope: &RequestScope,
) -> Option<(&'static str, String)> {
    if !matches!(req.method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        return None;
    }
    let body = || serde_json::from_str::<Value>(&req.text()).unwrap_or(Value::Null);
    let (operation, table, addressed) = match (req.method.as_str(), segs) {
        ("POST", ["v1", "namespaces", ns, "tables"]) => {
            let name = body()
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            (CatalogOperation::CreateTable, format!("{ns}.{name}"), None)
        }
        ("POST", ["v1", "tables", "rename"]) => {
            let source = body().get("source").cloned().unwrap_or(Value::Null);
            let ns = table_ident_namespace(&source).unwrap_or_default();
            let name = source
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let addressed = Some((ns.clone(), name.clone()));
            (
                CatalogOperation::RenameTable,
                format!("{ns}.{name}"),
                addressed,
            )
        }
        ("POST", ["v1", "namespaces", ns, "tables", table]) => (
            CatalogOperation::CommitData,
            format!("{ns}.{table}"),
            Some(((*ns).to_string(), (*table).to_string())),
        ),
        ("DELETE", ["v1", "namespaces", ns, "tables", table]) => (
            CatalogOperation::DropTable,
            format!("{ns}.{table}"),
            Some(((*ns).to_string(), (*table).to_string())),
        ),
        _ => (
            CatalogOperation::Other(format!("{} {}", req.method, segs.join("/"))),
            segs.join("/"),
            None,
        ),
    };
    if let Some((ns, name)) = addressed {
        if lake
            .load_table_visible(&ns, &name, &scope.visibility)
            .is_none()
        {
            return Some(("404 Not Found", not_found("table")));
        }
    }
    let (namespace, name) = table.split_once('.').unwrap_or(("", table.as_str()));
    let entry = TableEntry {
        namespace: namespace.to_string(),
        name: name.to_string(),
        metadata_location: String::new(),
        metadata_json: None,
        access: ProjectionAccess::ReadOnly,
    };
    let refusal = refuse_projection_write(&operation, &entry).err()?;
    let message = format!("{}: {}", refusal.code, refusal.table);
    record_operation(
        lake,
        "ProjectionWriteRefused",
        &[
            ("table", json!(refusal.table)),
            ("operation", json!(refusal.operation)),
            ("principal", json!(scope.owner_label())),
        ],
        &scope.owner_label(),
        "405 Method Not Allowed",
        false,
    );
    Some((
        "405 Method Not Allowed",
        err_body(&message, "ProjectionReadOnlyException", refusal.http_status),
    ))
}

/// Catalog-level routes: configuration, namespace listing and inspection,
/// table listing and creation, and the cross-namespace rename. Anything else
/// addresses one existing table.
fn catalog_route(
    lake: &LakeManager,
    req: &HttpMessage,
    segs: &[&str],
    scope: &RequestScope,
) -> (&'static str, String) {
    match (req.method.as_str(), segs) {
        ("GET", ["v1", "config"]) => (
            "200 OK",
            json!({ "defaults": {}, "overrides": {} }).to_string(),
        ),
        ("GET", ["v1", "namespaces"]) => {
            let (page_token, page_size) = parse_pagination(&req.target);
            (
                "200 OK",
                lake.list_namespaces_visible(&scope.visibility, page_token.as_deref(), page_size)
                    .to_string(),
            )
        }
        ("GET", ["v1", "namespaces", ns]) => describe_namespace(lake, ns, scope),
        ("GET", ["v1", "namespaces", ns, "tables"]) => {
            let (page_token, page_size) = parse_pagination(&req.target);
            (
                "200 OK",
                lake.list_tables_visible(ns, &scope.visibility, page_token.as_deref(), page_size)
                    .to_string(),
            )
        }
        _ => table_route(lake, req, segs, scope),
    }
}

/// Routes addressing ONE table under a namespace.
fn table_route(
    lake: &LakeManager,
    req: &HttpMessage,
    segs: &[&str],
    scope: &RequestScope,
) -> (&'static str, String) {
    match (req.method.as_str(), segs) {
        ("GET", ["v1", "namespaces", ns, "tables", table]) => {
            load_table(lake, req, ns, table, scope)
        }
        ("HEAD", ["v1", "namespaces", ns, "tables", table]) => {
            if lake
                .load_table_visible(ns, table, &scope.visibility)
                .is_some()
            {
                ("200 OK", String::new())
            } else {
                ("404 Not Found", String::new())
            }
        }
        _ => ("404 Not Found", not_found("route")),
    }
}

/// `GET /v1/namespaces/<ns>` — a namespace the caller may see, else the
/// privacy-safe 404.
fn describe_namespace(
    lake: &LakeManager,
    ns: &str,
    scope: &RequestScope,
) -> (&'static str, String) {
    if !lake.namespace_exists_visible(ns, &scope.visibility) {
        return ("404 Not Found", not_found("namespace"));
    }
    (
        "200 OK",
        json!({ "namespace": namespace_levels(ns), "properties": {} }).to_string(),
    )
}

/// Record a successful current-snapshot load with the snapshot LSN served
/// (MEDULLA-FEDERATED-QUERY-R080.5).
fn record_load(lake: &LakeManager, ns: &str, table: &str, scope: &RequestScope) {
    let Some(lsn) = lake.table_lsn(ns, table, &scope.visibility) else {
        return;
    };
    record_operation(
        lake,
        "LoadTable",
        &[
            ("table", json!(format!("{ns}.{table}"))),
            ("principal", json!(scope.owner_label())),
            ("lsn", json!(lsn)),
        ],
        &scope.owner_label(),
        "200 OK",
        true,
    );
}

/// `GET /v1/namespaces/<ns>/tables/<table>` — LoadTable, optionally `?as_of=`.
/// Visibility resolves BEFORE the `as_of` parse or validation, so a hidden
/// table stays the same privacy-safe 404 for malformed, future, hole and
/// overflow LSNs and is never an existence oracle.
fn load_table(
    lake: &LakeManager,
    req: &HttpMessage,
    ns: &str,
    table: &str,
    scope: &RequestScope,
) -> (&'static str, String) {
    let Some(current) = lake.load_table_visible(ns, table, &scope.visibility) else {
        return ("404 Not Found", not_found("table"));
    };
    let as_of = match parse_as_of_lsn(&req.target) {
        Ok(as_of) => as_of,
        Err(()) => {
            return (
                "400 Bad Request",
                err_body(
                    "as_of must be one unsigned decimal LSN",
                    "InvalidAsOfException",
                    400,
                ),
            )
        }
    };
    let Some(lsn) = as_of else {
        record_load(lake, ns, table, scope);
        return ("200 OK", current.to_string());
    };
    match lake.load_table_as_of(ns, table, lsn, &scope.visibility) {
        Ok(Some(snapshot)) => {
            record_operation(
                lake,
                "LoadTable",
                &[
                    ("table", json!(format!("{ns}.{table}"))),
                    ("principal", json!(scope.owner_label())),
                    ("lsn", json!(lsn)),
                ],
                &scope.owner_label(),
                "200 OK",
                true,
            );
            ("200 OK", snapshot.to_string())
        }
        // The visibility check above and the manager's scoped check
        // intentionally both fail closed if a concurrent delete or policy
        // change removes the table between the two reads.
        Ok(None) => ("404 Not Found", not_found("table")),
        Err(_) => (
            "400 Bad Request",
            err_body(
                "requested as_of LSN is unavailable",
                "InvalidSnapshotException",
                400,
            ),
        ),
    }
}
