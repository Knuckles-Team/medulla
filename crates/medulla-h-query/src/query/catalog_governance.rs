//! The governance reads of `Method::CatalogRead` (MEDULLA-TYPED-PACKS-R108.18).
//!
//! `Governance` answers which owners and stewards govern a target, and `Proposals` lists the
//! curation work of the caller's tenant: glossary description proposals and stewardship change
//! requests. Both read the typed graph nodes the glossary and stewardship writers store
//! (`cat:StewardshipAssignment`, `cat:DescriptionProposal`, `cat:ChangeRequest`), narrowed to
//! the caller's tenant and to the nodes the caller may read, and neither writes. The roles come
//! from the stored assignments through the one resolver the writers use
//! (`medulla_core::stewardship::governing`), so a caller that must admit an approval reads the
//! engine's answer and holds no role table of its own.

use medulla_catalog_types::catalog_read::{
    CatalogGovernance, CatalogProposalKind, CatalogProposalRow, CatalogReadAnswer,
};
use medulla_catalog_types::stewardship::{StewardshipAssignment, StewardshipTarget};
use medulla_core::stewardship::{governing, TargetScopes};
use serde_json::{Map, Value};

use super::catalog_read::{invalid, page_of};
use super::*;

/// Node label of a stewardship assignment (MEDULLA-TYPED-PACKS-R802.3).
const ASSIGNMENT_LABEL: &str = "cat:StewardshipAssignment";
/// Node label of a glossary description proposal (MEDULLA-TYPED-PACKS-R801.7).
const DESCRIPTION_LABEL: &str = "cat:DescriptionProposal";
/// Node label of a stewardship change request (MEDULLA-TYPED-PACKS-R802.3).
const CHANGE_REQUEST_LABEL: &str = "cat:ChangeRequest";
/// The property that holds the owning tenant of a node.
const TENANT_PROPERTY: &str = "tenant_id";
/// Most nodes of one label a governance read scans.
const MAX_NODES_READ: usize = 1_000_000;
/// The states a `Proposals` read may filter on: the description states and the request states.
const PROPOSAL_STATES: [&str; 5] = ["proposed", "approved", "rejected", "open", "applied"];

/// The caller's tenant nodes of `label`, with their typed properties, ordered by id.
fn tenant_nodes(ctx: &QueryHandlerCtx<'_>, label: &str) -> Vec<(String, Map<String, Value>)> {
    let tenant = super::stored_query::reader_of(ctx).tenant;
    let mut nodes: Vec<(String, Map<String, Value>)> = ctx
        .core
        .get_nodes_by_label(label, MAX_NODES_READ)
        .into_iter()
        .filter(|(id, blob)| {
            ctx.read_authority
                .is_none_or(|authority| authority.can_see_node(blob, ctx.core.is_schema_node(id)))
        })
        .filter_map(|(id, blob)| {
            let properties = medulla_types::msgpack::decode_property_object(&blob).ok()?;
            (properties.get(TENANT_PROPERTY).and_then(Value::as_str) == Some(tenant.as_str()))
                .then_some((id, properties))
        })
        .collect();
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    nodes
}

fn answer() -> CatalogReadAnswer {
    CatalogReadAnswer {
        kind_counts: Vec::new(),
        rows: Vec::new(),
        next_cursor: None,
        detail: None,
        governance: None,
        proposals: Vec::new(),
    }
}

/// The owners and stewards that govern `target`.
pub(super) fn governance(
    ctx: &QueryHandlerCtx<'_>,
    target: &StewardshipTarget,
) -> Result<CatalogReadAnswer, String> {
    if target.asset.is_none() && target.concept_scheme.is_none() && target.domain.is_none() {
        return Err(invalid(
            "the target names no asset, concept scheme or domain",
        ));
    }
    let assignments: Vec<StewardshipAssignment> = tenant_nodes(ctx, ASSIGNMENT_LABEL)
        .iter()
        .filter_map(|(_, properties)| StewardshipAssignment::from_properties(properties))
        .collect();
    let (owners, stewards) = governing(&assignments, &TargetScopes::of_target(target));
    Ok(CatalogReadAnswer {
        governance: Some(CatalogGovernance { owners, stewards }),
        ..answer()
    })
}

fn text(properties: &Map<String, Value>, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn principals(properties: &Map<String, Value>, key: &str) -> Vec<String> {
    properties
        .get(key)
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One page of the caller's description proposals and change requests.
pub(super) fn proposals(
    ctx: &QueryHandlerCtx<'_>,
    state: Option<String>,
    target: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Result<CatalogReadAnswer, String> {
    if let Some(state) = state.as_deref() {
        if !PROPOSAL_STATES.contains(&state) {
            return Err(invalid(format!("`{state}` is not a proposal state")));
        }
    }
    let page = page_of(limit, cursor.as_deref())?;
    let mut rows: Vec<CatalogProposalRow> = tenant_nodes(ctx, DESCRIPTION_LABEL)
        .into_iter()
        .filter_map(|(id, properties)| {
            Some(CatalogProposalRow {
                id,
                kind: CatalogProposalKind::Description,
                target: text(&properties, "target")?,
                state: text(&properties, "state")?,
                opened_by: text(&properties, "proposed_by"),
                reviewers: Vec::new(),
                text: text(&properties, "text"),
                change_request: text(&properties, "change_request"),
            })
        })
        .chain(
            tenant_nodes(ctx, CHANGE_REQUEST_LABEL)
                .into_iter()
                .filter_map(|(id, properties)| {
                    Some(CatalogProposalRow {
                        id,
                        kind: CatalogProposalKind::ChangeRequest,
                        target: text(&properties, "target")?,
                        state: text(&properties, "state")?,
                        opened_by: text(&properties, "opened_by"),
                        reviewers: principals(&properties, "reviewers"),
                        text: None,
                        change_request: None,
                    })
                }),
        )
        .filter(|row| state.as_deref().is_none_or(|wanted| row.state == wanted))
        .filter(|row| target.as_deref().is_none_or(|wanted| row.target == wanted))
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    let from = (page.offset as usize).min(rows.len());
    let end = (from + page.limit as usize).min(rows.len());
    let next_cursor = (end < rows.len()).then(|| end.to_string());
    Ok(CatalogReadAnswer {
        proposals: rows[from..end].to_vec(),
        next_cursor,
        ..answer()
    })
}
