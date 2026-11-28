//! The served engine-contract read (MEDULLA-CONTRACT-R054).
//!
//! `Method::DescribeContract` answers one bounded page of method descriptors --
//! the typed rows of the served `catalog_json`, in ascending `id` byte order --
//! and, on the first page, the scope registry. Every page carries the contract
//! catalog digest so a client that sees it change between pages restarts.
//!
//! Pure over `medulla_capabilities`: no durable substrates are read, so the
//! method serves in every build, including non-`redb`.

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};

use medulla_types::contract_catalog::{ContractMethodRow, ContractScopeRow, DescribeContractPage};

/// The default and maximum page size of the served contract read.
const DEFAULT_PAGE_LIMIT: usize = 500;
const MAX_PAGE_LIMIT: u16 = 500;
/// A cursor is the `id` of the last descriptor of the previous page: at most
/// 128 characters.
const MAX_CURSOR_CHARS: usize = 128;

/// Answer one `Method::DescribeContract` request.
///
/// Refusals: `DESCRIBE_CURSOR_INVALID` for a cursor over 128 characters,
/// `DESCRIBE_LIMIT_INVALID` for a limit of 0 or over 500.
pub fn handle(req_id: u64, method: Method) -> Response {
    let Method::DescribeContract { cursor, limit } = method else {
        return Response::err(
            req_id,
            "INTERNAL: describe_contract handler received a foreign method",
        );
    };
    let Some(cursor) = cursor.as_ref() else {
        return serve_page(req_id, None, limit);
    };
    if cursor.len() > MAX_CURSOR_CHARS {
        return Response::err(
            req_id,
            format!(
                "DESCRIBE_CURSOR_INVALID: the cursor is {} characters, over the {}-character bound",
                cursor.len(),
                MAX_CURSOR_CHARS
            ),
        );
    }
    serve_page(req_id, Some(cursor), limit)
}

fn serve_page(req_id: u64, cursor: Option<&str>, limit: Option<u16>) -> Response {
    let limit = match limit {
        Some(value) if (1..=MAX_PAGE_LIMIT).contains(&value) => value as usize,
        Some(value) => {
            return Response::err(
                req_id,
                format!("DESCRIBE_LIMIT_INVALID: limit {value} is outside 1..={MAX_PAGE_LIMIT}"),
            );
        }
        None => DEFAULT_PAGE_LIMIT,
    };
    // Reuse the served catalog's row building for the field set: type the
    // `catalog_json` rows rather than re-rendering them here.
    let catalog: serde_json::Value =
        serde_json::from_str(&medulla_capabilities::catalog::catalog_json())
            .expect("catalog_json is JSON");
    let methods: Vec<ContractMethodRow> = serde_json::from_value(catalog["methods"].clone())
        .expect("catalog_json method rows are ContractMethodRow-shaped");
    let scope_rows: Vec<ContractScopeRow> = serde_json::from_value(catalog["scopes"].clone())
        .expect("catalog_json scope rows are ContractScopeRow-shaped");
    // Serve the descriptors in ascending `id` byte order; a page starts at the
    // first `id` strictly greater than the cursor.
    let mut methods = methods;
    methods.sort_by(|a, b| a.id.cmp(&b.id));
    let total = methods.len();
    let start = match cursor {
        Some(last) => match methods.binary_search_by(|row| row.id.as_str().cmp(last)) {
            // The cursor names a row: the page starts strictly after it.
            Ok(index) => index + 1,
            // An unknown id: the page starts at the first greater row.
            Err(index) => index,
        },
        None => 0,
    };
    let methods: Vec<ContractMethodRow> = methods.iter().skip(start).take(limit).cloned().collect();
    let next_cursor = if start + methods.len() < total {
        methods.last().map(|row| row.id.clone())
    } else {
        None
    };
    let page = DescribeContractPage {
        methods,
        scopes: if cursor.is_none() {
            scope_rows
        } else {
            Vec::new()
        },
        schema_digest: medulla_capabilities::contract_catalog_digest().to_string(),
        next_cursor,
    };
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::security::DescribeContract>(
            page,
        ),
    )
}
