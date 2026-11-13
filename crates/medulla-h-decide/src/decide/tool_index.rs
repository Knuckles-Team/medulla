//! The per-tenant tool relevance index (MEDULLA-DECISION-ENGINE-R134.110.7).
//!
//! The index holds the tenant's visible published `Tool` components as
//! [`IndexedTool`] values (pre-joined text, stored embedding, token cost) and
//! the role allowlists of its published `Toolset` components. It is derived
//! state: nothing in it is authoritative, a restart rebuilds it from the
//! Agent Library, and it lives in the store's derived slot beside the library
//! it was read from.
//!
//! The library generation is the sum of the head `entry_revision` of every
//! `Tool` and `Toolset` component of the tenant, whatever its lifecycle. A
//! publish adds a head or raises one revision and a retirement raises one, so
//! the sum only grows: that is the monotonic `index_revision`. A request whose
//! generation equals the cached one reuses the cached index; otherwise the
//! index is rebuilt by diffing component revisions, and every tool whose
//! revision and definition digest are unchanged keeps its cached entry.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_agent_types::agent_component::{
    AgentComponentEntry, AgentComponentFacts, AgentComponentKind, AgentComponentSearchRequest,
};
use medulla_agent_types::agent_library::AgentLibraryLifecycle;
use medulla_decision_stats::decision::tool_relevance::IndexedTool;
use medulla_decision_types::decision::tool_relevance::{
    ToolEntry, MAX_TOOL_CAPABILITIES, TOOL_RELEVANCE_SCHEMA_VERSION,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_types::contract::BoundedVec;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

/// The store slot the cache lives in.
const SLOT: &str = "decide.tool_index";
/// Entries one library page carries.
const PAGE: u32 = 256;
/// Most pages one scan walks before it refuses as unbounded: the catalog cap
/// (`tool_subset.max_tools` at most 50000) plus the toolsets and retired heads.
const MAX_PAGES: usize = 1024;
/// The attribute a tool publisher stores its Q32 embedding in, as
/// comma-separated integers. Attributes are the documented extension point of
/// a component entry, so no new stored field exists.
pub(super) const EMBEDDING_ATTRIBUTE: &str = "embedding_q32";
/// The attribute naming the tool as the caller knows it (default: the id).
pub(super) const NAME_ATTRIBUTE: &str = "name";
/// The attribute naming the server that serves the tool when no `Toolset` or
/// `McpServer` dependency does.
pub(super) const SERVER_ATTRIBUTE: &str = "server_id";

/// Why an index read failed.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ToolIndexError {
    /// More visible tools than the caller allows (`CATALOG_TOO_LARGE`).
    TooLarge { visible: usize, max: usize },
    /// The library read failed.
    Read(String),
}

/// One tenant's index at one library generation.
#[derive(Debug)]
pub(super) struct ToolIndex {
    /// The monotonic library generation the index was built from.
    pub(super) generation: u64,
    /// Visible published tools, sorted by tool id.
    pub(super) tools: Vec<IndexedTool>,
    /// Published toolsets: toolset id to the tool ids it allows, sorted.
    pub(super) roles: BTreeMap<String, Vec<String>>,
    /// SHA-256 over the sorted `(tool_id, definition digest)` pairs, 64
    /// lowercase hex characters.
    pub(super) catalog_digest: String,
    /// The per-component state the next rebuild diffs against.
    seen: BTreeMap<String, Seen>,
}

#[derive(Debug, Clone)]
struct Seen {
    entry_revision: u64,
    definition_digest: String,
    tool: Option<IndexedTool>,
}

impl ToolIndex {
    /// The definition digest of the published component behind `tool_id`.
    pub(super) fn definition_digest(&self, tool_id: &str) -> Option<&str> {
        self.seen
            .get(tool_id)
            .map(|seen| seen.definition_digest.as_str())
    }
}

/// The per-tenant cache.
#[derive(Default)]
pub(super) struct ToolIndexCache {
    tenants: Mutex<BTreeMap<String, Arc<ToolIndex>>>,
    rebuilds: AtomicU64,
}

impl ToolIndexCache {
    /// The cache beside `store`.
    pub(super) fn of(store: &AgentLibraryStore) -> Arc<Self> {
        store.derived_slot::<Self>(SLOT)
    }

    /// How many times an index was rebuilt (a hit does not count).
    #[cfg(test)]
    pub(super) fn rebuilds(&self) -> u64 {
        self.rebuilds.load(Ordering::Relaxed)
    }

    /// The tenant's index at the current library generation.
    ///
    /// `max_tools` bounds the visible tool count: a larger catalog is
    /// `TooLarge` and is never cached.
    pub(super) fn get(
        &self,
        store: &AgentLibraryStore,
        tenant_id: &str,
        max_tools: usize,
    ) -> Result<Arc<ToolIndex>, ToolIndexError> {
        let heads = read_heads(store, tenant_id)?;
        let generation: u64 = heads.iter().map(|entry| entry.entry_revision).sum();
        let cached = self.tenants.lock().get(tenant_id).cloned();
        if let Some(index) = &cached {
            if index.generation == generation {
                return check_size(index, max_tools).map(|()| Arc::clone(index));
            }
        }
        self.rebuilds.fetch_add(1, Ordering::Relaxed);
        let index = Arc::new(build(generation, heads, cached.as_deref()));
        check_size(&index, max_tools)?;
        self.tenants
            .lock()
            .insert(tenant_id.to_string(), Arc::clone(&index));
        Ok(index)
    }
}

fn check_size(index: &ToolIndex, max_tools: usize) -> Result<(), ToolIndexError> {
    if index.tools.len() > max_tools {
        return Err(ToolIndexError::TooLarge {
            visible: index.tools.len(),
            max: max_tools,
        });
    }
    Ok(())
}

/// Every `Tool` and `Toolset` head of the tenant the library serves to it.
fn read_heads(
    store: &AgentLibraryStore,
    tenant_id: &str,
) -> Result<Vec<AgentComponentEntry>, ToolIndexError> {
    let mut heads = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_PAGES {
        let page = store
            .search_components(&AgentComponentSearchRequest {
                tenant_id: tenant_id.to_string(),
                task: None,
                capabilities: Vec::new(),
                kinds: vec![AgentComponentKind::Tool, AgentComponentKind::Toolset],
                read_only: false,
                limit: Some(PAGE),
                cursor: cursor.take(),
            })
            .map_err(ToolIndexError::Read)?;
        heads.extend(page.entries);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(heads),
        }
    }
    Err(ToolIndexError::Read(
        "the tool catalog scan did not finish".to_string(),
    ))
}

fn build(
    generation: u64,
    heads: Vec<AgentComponentEntry>,
    previous: Option<&ToolIndex>,
) -> ToolIndex {
    let mut seen = BTreeMap::new();
    let mut roles = BTreeMap::new();
    for entry in heads {
        if entry.lifecycle != AgentLibraryLifecycle::Published {
            seen.insert(
                entry.component_id.clone(),
                Seen {
                    entry_revision: entry.entry_revision,
                    definition_digest: entry.definition_digest.clone(),
                    tool: None,
                },
            );
            continue;
        }
        match entry.kind {
            AgentComponentKind::Toolset => {
                let mut allowed: Vec<String> = entry
                    .requires
                    .iter()
                    .filter(|dependency| dependency.kind == AgentComponentKind::Tool)
                    .map(|dependency| dependency.component_id.clone())
                    .collect();
                allowed.sort();
                allowed.dedup();
                roles.insert(entry.component_id.clone(), allowed);
                seen.insert(
                    entry.component_id.clone(),
                    Seen {
                        entry_revision: entry.entry_revision,
                        definition_digest: entry.definition_digest.clone(),
                        tool: None,
                    },
                );
            }
            _ => {
                let unchanged = previous
                    .and_then(|index| index.seen.get(&entry.component_id))
                    .filter(|old| {
                        old.entry_revision == entry.entry_revision
                            && old.definition_digest == entry.definition_digest
                    });
                let tool = match unchanged {
                    Some(old) => old.tool.clone(),
                    None => indexed(&entry),
                };
                seen.insert(
                    entry.component_id.clone(),
                    Seen {
                        entry_revision: entry.entry_revision,
                        definition_digest: entry.definition_digest.clone(),
                        tool,
                    },
                );
            }
        }
    }
    let mut tools: Vec<IndexedTool> = seen.values().filter_map(|seen| seen.tool.clone()).collect();
    tools.sort_by(|a, b| a.entry.tool_id.cmp(&b.entry.tool_id));
    let mut digest = Sha256::new();
    for tool in &tools {
        let definition = seen
            .get(&tool.entry.tool_id)
            .map_or("", |seen| seen.definition_digest.as_str());
        digest.update(tool.entry.tool_id.as_bytes());
        digest.update([0u8]);
        digest.update(definition.as_bytes());
        digest.update([b'\n']);
    }
    let catalog_digest = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    ToolIndex {
        generation,
        tools,
        roles,
        catalog_digest,
        seen,
    }
}

/// The index value of one published tool component, or `None` when the tool
/// cannot be budgeted: a tool that declares no schema size has an unknown
/// cost, and an unknown cost is never read as zero.
fn indexed(entry: &AgentComponentEntry) -> Option<IndexedTool> {
    let AgentComponentFacts::Tool {
        schema_tokens: Some(token_cost),
        ..
    } = &entry.facts
    else {
        return None;
    };
    let server_id = entry
        .attributes
        .get(SERVER_ATTRIBUTE)
        .cloned()
        .or_else(|| {
            entry
                .requires
                .iter()
                .find(|dependency| {
                    matches!(
                        dependency.kind,
                        AgentComponentKind::Toolset | AgentComponentKind::McpServer
                    )
                })
                .map(|dependency| dependency.component_id.clone())
        })
        .unwrap_or_default();
    let mut capabilities = entry.declared_capabilities.clone();
    capabilities.sort();
    capabilities.dedup();
    capabilities.truncate(MAX_TOOL_CAPABILITIES);
    Some(IndexedTool {
        entry: ToolEntry {
            schema_version: TOOL_RELEVANCE_SCHEMA_VERSION,
            tool_id: entry.component_id.clone(),
            server_id,
            name: entry
                .attributes
                .get(NAME_ATTRIBUTE)
                .cloned()
                .unwrap_or_else(|| entry.component_id.clone()),
            description: entry.summary.clone(),
            capability_iris: BoundedVec::new(capabilities)
                .expect("capabilities were truncated to the bound"),
            token_cost: *token_cost,
        },
        embedding: entry.attributes.get(EMBEDDING_ATTRIBUTE).and_then(|text| {
            text.split(',')
                .map(|part| part.trim().parse::<i64>().ok())
                .collect::<Option<Vec<i64>>>()
        }),
        visible: true,
    })
}
