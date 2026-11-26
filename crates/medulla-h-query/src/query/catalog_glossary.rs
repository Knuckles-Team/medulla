//! Glossary term search through `CatalogRead::Search` (MEDULLA-TYPED-PACKS-R801.6).
//!
//! `CatalogRead::Search` is the one catalog search path. A glossary term is a
//! `cat:CatalogGlossaryTerm` graph node, not an asset the ranked stored query reads, so the
//! search reads the caller's terms and answers a label match through
//! [`medulla_core::glossary::GlossaryLabelIndex`]: a word-prefix index over preferred and
//! alternative labels, so a search verifies only the terms of its rarest query word.
//!
//! The terms and their index are built once per graph version and caller and reused until the
//! graph changes, so a search never rescans every term. A term another tenant owns, or one the
//! caller may not read, is never in the list.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use medulla_catalog_types::catalog_read::{CatalogAssetRow, CATALOG_GLOSSARY_KIND};
use medulla_catalog_types::glossary::{GlossaryTerm, CAT_GLOSSARY_TERM};
use medulla_core::glossary::GlossaryLabelIndex;

use serde_json::Value;

use super::*;

/// The property that holds a term's whole record (`medulla-server-graph`'s glossary writer).
const RECORD_PROPERTY: &str = "record";
/// The property that holds the owning tenant of a term node.
const TENANT_PROPERTY: &str = "tenant";
/// Most term nodes one search reads.
const MAX_TERMS_READ: usize = 1_000_000;
/// Most (graph, caller) term lists kept; past it the cache is dropped and rebuilt on demand.
const MAX_CACHED_LISTS: usize = 16;

/// The terms a caller can read and their label index.
struct TermList {
    iris: Vec<String>,
    terms: Vec<GlossaryTerm>,
    index: GlossaryLabelIndex,
}

type CacheKey = (String, String, String);

fn cache() -> &'static Mutex<HashMap<CacheKey, (u64, Arc<TermList>)>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, (u64, Arc<TermList>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn build(ctx: &QueryHandlerCtx<'_>, tenant: &str) -> TermList {
    let mut terms: Vec<(String, GlossaryTerm)> = ctx
        .core
        .get_nodes_by_label(CAT_GLOSSARY_TERM, MAX_TERMS_READ)
        .into_iter()
        .filter(|(id, blob)| {
            ctx.read_authority
                .is_none_or(|authority| authority.can_see_node(blob, ctx.core.is_schema_node(id)))
        })
        .filter_map(|(id, blob)| {
            let properties = medulla_types::msgpack::decode_property_object(&blob).ok()?;
            if properties.get(TENANT_PROPERTY).and_then(Value::as_str) != Some(tenant) {
                return None;
            }
            let record = properties.get(RECORD_PROPERTY).and_then(Value::as_str)?;
            let term = serde_json::from_str::<GlossaryTerm>(record).ok()?;
            Some((id, term))
        })
        .collect();
    terms.sort_by(|a, b| a.0.cmp(&b.0));
    let (iris, terms): (Vec<String>, Vec<GlossaryTerm>) = terms.into_iter().unzip();
    let index = GlossaryLabelIndex::build(&terms);
    TermList { iris, terms, index }
}

/// The caller's term list at the graph's current version, built at most once per version.
fn term_list(ctx: &QueryHandlerCtx<'_>, tenant: &str) -> Arc<TermList> {
    let version = ctx.core.version();
    let key = (
        ctx.graph_name.to_string(),
        tenant.to_string(),
        ctx.caller.to_string(),
    );
    if let Ok(cached) = cache().lock() {
        if let Some((cached_version, list)) = cached.get(&key) {
            if *cached_version == version {
                return Arc::clone(list);
            }
        }
    }
    let list = Arc::new(build(ctx, tenant));
    if let Ok(mut cached) = cache().lock() {
        if cached.len() >= MAX_CACHED_LISTS {
            cached.clear();
        }
        cached.insert(key, (version, Arc::clone(&list)));
    }
    list
}

/// The caller's glossary terms whose labels match `text`, as asset rows ordered by term IRI.
pub(super) fn matching_terms(ctx: &QueryHandlerCtx<'_>, text: &str) -> Vec<CatalogAssetRow> {
    let tenant = super::stored_query::reader_of(ctx).tenant;
    let list = term_list(ctx, &tenant);
    list.index
        .search(&list.terms, text)
        .into_iter()
        .filter_map(|position| Some((list.iris.get(position)?, list.terms.get(position)?)))
        .map(|(iri, term)| CatalogAssetRow {
            asset: iri.clone(),
            kind: CATALOG_GLOSSARY_KIND.to_string(),
            name: Some(term.pref_label.clone()),
            owner: None,
            score: None,
        })
        .collect()
}
