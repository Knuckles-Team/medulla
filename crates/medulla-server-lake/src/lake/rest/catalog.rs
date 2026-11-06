//! The Iceberg REST catalog: what a request addresses, and the operation each
//! address answers.
//!
//! [`super`] owns the listener, the bearer/carrier boundary and the response
//! vocabulary; this module owns routing and the operations themselves. Before
//! the split they were one `handle` of cyclomatic 37 / cognitive 31, in which
//! every route re-derived the caller's visibility and four of them re-emitted
//! the same audit event by hand.

use serde_json::{json, Value};

use super::super::{namespace_levels, LakeManager, LakeVisibility};
use super::{
    carrier_denied_response, err_body, not_found, operation_scope, parse_as_of_lsn,
    parse_pagination, percent_decode, scope_authorized, table_ident_namespace, visibility_for,
};
use medulla_lake::catalog::{
    refuse_projection_write, CatalogOperation, ProjectionAccess, TableEntry, PROJECTION_READ_ONLY,
};
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::http1::HttpMessage;
use medulla_server_blob::blob::store::ChunkStore;

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

/// Record one operation's audit event, so every route emits the same fields
/// around its own subject identity and a new route cannot omit one.
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
    _store: &dyn ChunkStore,
    req: &HttpMessage,
    carrier: Option<&CarrierAuthority>,
) -> (&'static str, String) {
    // `_store`: every chunk-store write route is refused at the projection
    // boundary, so routing reads only the lake catalog.
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

    if let Some(refused) = guard_projection_write(lake, req, &seg_refs, &scope) {
        return refused;
    }
    catalog_route(lake, req, &seg_refs, &scope)
}

/// The projection write boundary (MEDULLA-FEDERATED-QUERY-R080.3): every
/// POST, PUT, PATCH or DELETE is classified into a [`CatalogOperation`] and
/// refused through [`refuse_projection_write`], because the graph is the only
/// write boundary and every lake table is a read-only projection. The
/// visibility check runs first, so a table the caller cannot see stays the
/// privacy-safe 404 (MEDULLA-FEDERATED-QUERY-R080.4). The refusal runs before
/// any `LakeManager` call and is recorded (MEDULLA-FEDERATED-QUERY-R080.5).
fn guard_projection_write(
    lake: &LakeManager,
    req: &HttpMessage,
    segs: &[&str],
    scope: &RequestScope,
) -> Option<(&'static str, String)> {
    if !matches!(req.method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        return None;
    }
    let (operation, namespace, table) = match classify_write(lake, req, segs, scope) {
        Ok(classified) => classified,
        Err((operation, namespace, table)) => {
            // A hidden table: audited as a denial, answered exactly like a
            // table that does not exist.
            let status = "404 Not Found";
            record_operation(
                lake,
                operation.name(),
                &[("namespace", json!(namespace)), ("table", json!(table))],
                &scope.owner_label(),
                status,
                false,
            );
            return Some((status, not_found("table")));
        }
    };
    let entry = TableEntry {
        namespace,
        name: table,
        metadata_location: String::new(),
        metadata_json: None,
        access: ProjectionAccess::ReadOnly,
    };
    let refusal = refuse_projection_write(&operation, &entry).err()?;
    let status = "405 Method Not Allowed";
    record_operation(
        lake,
        "ProjectionWriteRefused",
        &[
            ("table", json!(refusal.table)),
            ("operation", json!(refusal.operation)),
            ("principal", json!(scope.owner_label())),
        ],
        &scope.owner_label(),
        status,
        false,
    );
    Some((
        status,
        err_body(
            &format!("{PROJECTION_READ_ONLY}: {}", refusal.table),
            "ProjectionReadOnlyException",
            405,
        ),
    ))
}

/// The catalog operation, namespace and table one write route addresses.
/// `Err` names an addressed table the caller cannot see (or that does not
/// exist), which the guard answers with the privacy-safe 404.
fn classify_write(
    lake: &LakeManager,
    req: &HttpMessage,
    segs: &[&str],
    scope: &RequestScope,
) -> Result<(CatalogOperation, String, String), (CatalogOperation, String, String)> {
    let classified = match (req.method.as_str(), segs) {
        ("POST", ["v1", "namespaces", ns, "tables"]) => {
            let body = serde_json::from_str::<Value>(&req.text()).unwrap_or(Value::Null);
            let table = body.get("name").and_then(Value::as_str).unwrap_or_default();
            (
                CatalogOperation::CreateTable,
                (*ns).to_string(),
                table.to_string(),
            )
        }
        ("POST", ["v1", "tables", "rename"]) => {
            let body = serde_json::from_str::<Value>(&req.text()).unwrap_or(Value::Null);
            let source = body.get("source").cloned().unwrap_or(Value::Null);
            let ns = table_ident_namespace(&source).unwrap_or_default();
            let table = source
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            (CatalogOperation::RenameTable, ns, table.to_string())
        }
        ("POST", ["v1", "namespaces", ns, "tables", table]) => (
            CatalogOperation::CommitData,
            (*ns).to_string(),
            (*table).to_string(),
        ),
        ("DELETE", ["v1", "namespaces", ns, "tables", table]) => (
            CatalogOperation::DropTable,
            (*ns).to_string(),
            (*table).to_string(),
        ),
        (method, _) => {
            let (path, _) = req.path_and_query();
            (
                CatalogOperation::Other(format!("{method} {path}")),
                String::new(),
                path.to_string(),
            )
        }
    };
    let addresses_table = matches!(
        classified.0,
        CatalogOperation::RenameTable | CatalogOperation::CommitData | CatalogOperation::DropTable
    );
    if addresses_table
        && lake
            .load_table_visible(&classified.1, &classified.2, &scope.visibility)
            .is_none()
    {
        return Err(classified);
    }
    Ok(classified)
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

/// Record a served LoadTable with the snapshot LSN of the returned table:
/// the graph state version the projection was materialized at
/// (MEDULLA-FEDERATED-QUERY-R080.5).
fn record_load(lake: &LakeManager, ns: &str, table: &str, lsn: u64, scope: &RequestScope) {
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
        if let Some(lsn) = lake.table_lsn(ns, table, &scope.visibility) {
            record_load(lake, ns, table, lsn, scope);
        }
        return ("200 OK", current.to_string());
    };
    match lake.load_table_as_of(ns, table, lsn, &scope.visibility) {
        Ok(Some(snapshot)) => {
            record_load(lake, ns, table, lsn, scope);
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
