//! `Method::CatalogRead` (MEDULLA-TYPED-PACKS-R108.17): the served catalog reads.
//!
//! The handler validates every clause of the request before it runs anything: a blank or
//! unknown asset id, an unknown kind, a bad cursor, a page size out of bounds, a blank search
//! text and a search text over `catalog_search.max_query_chars`. It then runs the registered
//! stored query of the op (`catalog.assets_by_kind`, `catalog.assets_search`,
//! `catalog.asset_detail`) through the one stored-query call path, so the caller's read grants
//! narrow every row, and shapes the rows into one `CatalogReadAnswer`.

use std::collections::BTreeMap;

use medulla_catalog_types::catalog_read::{
    CatalogAssetDetail, CatalogAssetRow, CatalogComment, CatalogKindCount, CatalogReadAnswer,
    CatalogReadOp, CATALOG_ASSETS_BY_KIND, CATALOG_ASSETS_SEARCH, CATALOG_ASSET_DETAIL,
    CATALOG_ASSET_KINDS, CATALOG_ASSET_NOT_FOUND, CATALOG_GLOSSARY_KIND, CATALOG_INVALID_ARGUMENT,
};
use medulla_protocol_types::result_contract::query as results;
use medulla_source_types::stored_query::STORED_QUERY_NOT_FOUND;
use serde_json::{json, Value};
use {
    medulla_catalog_source_types::catalog_relevance::CATALOG_QUERY_EMPTY,
    medulla_catalog_source_types::catalog_relevance::CATALOG_QUERY_TOO_LONG,
    medulla_catalog_source_types::catalog_relevance::CATALOG_SEARCH_MAX_QUERY_CHARS,
    medulla_catalog_source_types::catalog_relevance::CATALOG_SEARCH_PAGE_MAX,
};

use super::stored_query::handle_stored_call;
use super::*;

/// One validated page request.
pub(super) struct Page {
    pub(super) limit: u64,
    pub(super) offset: u64,
    /// Where the glossary leg of a search resumes (zero until the asset leg is exhausted).
    pub(super) glossary_offset: u64,
}

pub(super) fn invalid(detail: impl std::fmt::Display) -> String {
    format!("{CATALOG_INVALID_ARGUMENT}: {detail}")
}

fn not_found() -> String {
    format!("{CATALOG_ASSET_NOT_FOUND}: no readable asset has this id")
}

/// The page size and the offset of a request: the size at most `catalog.search_page_max`, the
/// cursor the decimal offset a previous page returned.
pub(super) fn page_of(limit: Option<u32>, cursor: Option<&str>) -> Result<Page, String> {
    let max = CATALOG_SEARCH_PAGE_MAX.value();
    let limit = match limit {
        None => max,
        Some(n) if (1..=max).contains(&u64::from(n)) => u64::from(n),
        Some(_) => return Err(invalid(format!("limit must be between 1 and {max}"))),
    };
    let bad_cursor = || invalid("the cursor is not one a previous page returned");
    // A cursor is the decimal offset into the asset rows, and after them `:` and the decimal
    // offset into the glossary rows of a search.
    let (offset, glossary_offset) = match cursor.map(str::trim) {
        None => (0, 0),
        Some(raw) => match raw.split_once(':') {
            None => (raw.parse::<u64>().map_err(|_| bad_cursor())?, 0),
            Some((assets, glossary)) => (
                assets.parse::<u64>().map_err(|_| bad_cursor())?,
                glossary.parse::<u64>().map_err(|_| bad_cursor())?,
            ),
        },
    };
    Ok(Page {
        limit,
        offset,
        glossary_offset,
    })
}

fn check_kind(kind: &str) -> Result<(), String> {
    if CATALOG_ASSET_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(invalid(format!("`{kind}` is not an asset kind")))
    }
}

/// An IRI as the caller wrote it, without the angle brackets of a Turtle reference.
fn bare_iri(raw: &str) -> &str {
    let raw = raw.trim();
    raw.strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(raw)
}

/// The lexical form of one result cell: an IRI without its brackets, a literal without its
/// quotes and datatype tag.
fn plain(cell: &Value) -> Option<String> {
    let text = cell.as_str()?;
    if let Some(rest) = text.strip_prefix('"') {
        let end = rest.rfind('"').unwrap_or(rest.len());
        return Some(rest[..end].to_string());
    }
    Some(bare_iri(text).to_string())
}

fn cell(row: &Value, column: &str) -> Option<String> {
    row.get(column)
        .and_then(plain)
        .filter(|text| !text.is_empty())
}

/// Run one registered stored query and return its rows. A graph that has taken no catalog
/// write holds no registered queries yet and so holds no assets: the rows are empty.
async fn run(
    ctx: &QueryHandlerCtx<'_>,
    name: &str,
    args: BTreeMap<String, Value>,
) -> Result<Vec<Value>, String> {
    match handle_stored_call(ctx, name, None, &args).await {
        Ok(called) => Ok(called.rows.as_array().cloned().unwrap_or_default()),
        Err(refusal) if refusal.code == STORED_QUERY_NOT_FOUND => Ok(Vec::new()),
        Err(refusal) => Err(refusal.to_string()),
    }
}

async fn browse(
    ctx: &QueryHandlerCtx<'_>,
    kind: Option<String>,
    owner: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Result<CatalogReadAnswer, String> {
    if let Some(kind) = kind.as_deref() {
        check_kind(kind)?;
    }
    let page = page_of(limit, cursor.as_deref())?;
    let args = BTreeMap::from([
        ("kind".to_string(), json!(kind.unwrap_or_default())),
        (
            "owner".to_string(),
            json!(owner.as_deref().map(bare_iri).unwrap_or_default()),
        ),
        ("limit".to_string(), json!(page.limit + 1)),
        ("offset".to_string(), json!(page.offset)),
    ]);
    let mut kind_counts = Vec::new();
    let mut rows = Vec::new();
    for row in run(ctx, CATALOG_ASSETS_BY_KIND, args).await? {
        match (cell(&row, "count"), cell(&row, "asset"), cell(&row, "kind")) {
            (Some(count), _, Some(kind)) => kind_counts.push(CatalogKindCount {
                kind,
                count: count.parse().unwrap_or(0),
            }),
            (_, Some(asset), Some(kind)) => rows.push(CatalogAssetRow {
                asset,
                kind,
                name: cell(&row, "name"),
                owner: cell(&row, "owner"),
                score: None,
            }),
            _ => {}
        }
    }
    kind_counts.sort_by(|a, b| a.kind.cmp(&b.kind));
    let next_cursor = trim_page(&mut rows, &page);
    Ok(CatalogReadAnswer {
        kind_counts,
        rows,
        next_cursor,
        detail: None,
        governance: None,
        proposals: Vec::new(),
    })
}

/// Keep one page of `rows` (the query asked for one more than the page) and name the next
/// cursor when that extra row was there.
fn trim_page(rows: &mut Vec<CatalogAssetRow>, page: &Page) -> Option<String> {
    if rows.len() as u64 > page.limit {
        rows.truncate(page.limit as usize);
        Some((page.offset + page.limit).to_string())
    } else {
        None
    }
}

async fn search(
    ctx: &QueryHandlerCtx<'_>,
    text: String,
    kinds: Vec<String>,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Result<CatalogReadAnswer, String> {
    if text.trim().is_empty() {
        return Err(format!("{CATALOG_QUERY_EMPTY}: the search text is blank"));
    }
    let max_chars = CATALOG_SEARCH_MAX_QUERY_CHARS.value();
    if text.chars().count() as u64 > max_chars {
        return Err(format!(
            "{CATALOG_QUERY_TOO_LONG}: the search text is over {max_chars} characters"
        ));
    }
    // A search may name the glossary kind beside the asset kinds (MEDULLA-TYPED-PACKS-R801.6).
    let (glossary_kinds, asset_kinds): (Vec<String>, Vec<String>) = kinds
        .into_iter()
        .partition(|kind| kind == CATALOG_GLOSSARY_KIND);
    for kind in &asset_kinds {
        check_kind(kind)?;
    }
    let all_kinds = asset_kinds.is_empty() && glossary_kinds.is_empty();
    let want_assets = all_kinds || !asset_kinds.is_empty();
    let want_glossary = all_kinds || !glossary_kinds.is_empty();
    let page = page_of(limit, cursor.as_deref())?;
    let mut rows: Vec<CatalogAssetRow> = Vec::new();
    let mut asset_end = page.offset;
    let mut asset_more = false;
    if want_assets {
        let args = BTreeMap::from([
            ("text".to_string(), json!(text)),
            ("kinds".to_string(), json!(asset_kinds.join(" "))),
            ("limit".to_string(), json!(page.limit + 1)),
            ("offset".to_string(), json!(page.offset)),
        ]);
        rows = run(ctx, CATALOG_ASSETS_SEARCH, args)
            .await?
            .iter()
            .filter_map(|row| {
                Some(CatalogAssetRow {
                    asset: cell(row, "asset")?,
                    kind: cell(row, "kind")?,
                    name: cell(row, "name"),
                    owner: None,
                    score: cell(row, "score").and_then(|score| score.parse().ok()),
                })
            })
            .collect();
        asset_more = rows.len() as u64 > page.limit;
        rows.truncate(page.limit as usize);
        asset_end = page.offset + rows.len() as u64;
    }
    // The glossary rows follow the asset rows: they fill the page only once the asset leg
    // has no row left, so one cursor resumes both legs.
    let mut glossary_end = page.glossary_offset;
    let mut glossary_more = false;
    if want_glossary && !asset_more {
        let room = (page.limit - rows.len() as u64) as usize;
        let matches = super::catalog_glossary::matching_terms(ctx, &text);
        let from = (page.glossary_offset as usize).min(matches.len());
        let taken: Vec<CatalogAssetRow> = matches.iter().skip(from).take(room).cloned().collect();
        glossary_end = (from + taken.len()) as u64;
        glossary_more = (glossary_end as usize) < matches.len();
        rows.extend(taken);
    }
    let next_cursor = if asset_more {
        Some(if want_glossary {
            format!("{asset_end}:{}", page.glossary_offset)
        } else {
            asset_end.to_string()
        })
    } else if glossary_more {
        Some(format!("{asset_end}:{glossary_end}"))
    } else {
        None
    };
    Ok(CatalogReadAnswer {
        kind_counts: Vec::new(),
        rows,
        next_cursor,
        detail: None,
        governance: None,
        proposals: Vec::new(),
    })
}

async fn detail(ctx: &QueryHandlerCtx<'_>, asset_id: String) -> Result<CatalogReadAnswer, String> {
    let asset = bare_iri(&asset_id).to_string();
    if asset.is_empty() {
        return Err(invalid("asset_id is required"));
    }
    let args = BTreeMap::from([("asset_id".to_string(), json!(asset))]);
    let rows = run(ctx, CATALOG_ASSET_DETAIL, args).await?;
    // The kind fact is the asset's existence: no row means an unknown asset or one the caller
    // may not read, and both answer with the same body.
    let Some(kind) = rows
        .iter()
        .find(|row| cell(row, "facet").as_deref() == Some("kind"))
        .and_then(|row| cell(row, "value"))
    else {
        return Err(not_found());
    };
    let mut out = CatalogAssetDetail {
        asset,
        kind,
        name: None,
        owners: Vec::new(),
        terms: Vec::new(),
        upstream: Vec::new(),
        downstream: Vec::new(),
        rating_count: 0,
        rating_mean: None,
        comments: Vec::new(),
    };
    let mut rating_sum = 0.0;
    for row in &rows {
        let (Some(facet), Some(value)) = (cell(row, "facet"), cell(row, "value")) else {
            continue;
        };
        match facet.as_str() {
            "name" => out.name = Some(value),
            "owner" => out.owners.push(value),
            "term" => out.terms.push(value),
            "upstream" => out.upstream.push(value),
            "downstream" => out.downstream.push(value),
            "rating" => {
                if let Ok(rating) = value.parse::<f64>() {
                    out.rating_count += 1;
                    rating_sum += rating;
                }
            }
            "comment" => out.comments.push(CatalogComment {
                id: cell(row, "extra").unwrap_or_default(),
                text: value,
                author: cell(row, "who"),
                at_ms: cell(row, "at").and_then(|at| at.parse().ok()),
            }),
            _ => {}
        }
    }
    if out.rating_count > 0 {
        out.rating_mean = Some(rating_sum / out.rating_count as f64);
    }
    out.owners.sort();
    out.owners.dedup();
    out.terms.sort();
    out.terms.dedup();
    out.upstream.sort();
    out.upstream.dedup();
    out.downstream.sort();
    out.downstream.dedup();
    out.comments
        .sort_by(|a, b| (a.at_ms, &a.id).cmp(&(b.at_ms, &b.id)));
    Ok(CatalogReadAnswer {
        kind_counts: Vec::new(),
        rows: Vec::new(),
        next_cursor: None,
        detail: Some(out),
        governance: None,
        proposals: Vec::new(),
    })
}

/// Serve one `CatalogRead`.
pub(crate) async fn handle_catalog_read(ctx: &QueryHandlerCtx<'_>, op: CatalogReadOp) -> Response {
    let (answer, kind) = match op {
        CatalogReadOp::Browse {
            kind,
            owner,
            limit,
            cursor,
        } => (browse(ctx, kind, owner, limit, cursor).await, 0),
        CatalogReadOp::Search {
            text,
            kinds,
            limit,
            cursor,
        } => (search(ctx, text, kinds, limit, cursor).await, 1),
        CatalogReadOp::Detail { asset_id } => (detail(ctx, asset_id).await, 2),
        CatalogReadOp::Governance { target } => {
            (super::catalog_governance::governance(ctx, &target), 3)
        }
        CatalogReadOp::Proposals {
            state,
            target,
            limit,
            cursor,
        } => (
            super::catalog_governance::proposals(ctx, state, target, limit, cursor),
            4,
        ),
    };
    let payload = answer.and_then(|answer| match kind {
        0 => ResultPayload::of::<results::CatalogReadBrowse>(answer),
        1 => ResultPayload::of::<results::CatalogReadSearch>(answer),
        2 => ResultPayload::of::<results::CatalogReadDetail>(answer),
        3 => ResultPayload::of::<results::CatalogReadGovernance>(answer),
        _ => ResultPayload::of::<results::CatalogReadProposals>(answer),
    });
    match payload {
        Ok(payload) => Response::ok(ctx.req_id, payload),
        Err(error) => Response::err(ctx.req_id, error),
    }
}
