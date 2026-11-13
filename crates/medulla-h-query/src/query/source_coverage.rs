//! `Method::SourceCoverage` (MEDULLA-SRC-AUTH-R004.6.1): which connected source is
//! authoritative for each class and facet, with the premises behind every verdict.
//!
//! The handler reads the request graph's composed schema sources and builds the
//! selection fixture from them. A source is connected only when it is in the
//! caller's own owner-scoped foreign registry and an approved mapping names it
//! (R004.6), so a source another principal registered is `Unconnected` here and the
//! reply never names it as registered. Source roles come from the schema documents
//! (R010.3). The caller's verified tenant and the request graph are the selection
//! scope. Verdicts leave as the wire form of `medulla-identity-types`; the audit is
//! built from the verdicts' premises only (R007.2). Nothing here writes.

use super::*;
use medulla_federation::federation::ConnectedSources;
use medulla_identity_types::source_coverage::{
    CoverageHop, CoverageVerdict, SourceCoverageFacet, SourceCoverageReply, MAX_HOP_BUDGET,
    SOURCE_COVERAGE_HOP_BUDGET_INVALID, SOURCE_COVERAGE_NO_CLASSES,
    SOURCE_COVERAGE_TOO_MANY_CLASSES,
};
use medulla_protocol_types::result_contract::query::SourceCoverage;
use medulla_server_ops::api::query_txn::ForeignStateExt;
use medulla_virtual_graph::source_selection::{
    audit_from, connected_sources_from_turtle, fixture_from_schema_sources, role_order,
    select_sources_with, CoverageFixture, CoverageResult, Premise, SelectionQuery,
    SourceSelectionHop,
};
use std::collections::BTreeSet;

fn render(premises: &[Premise]) -> Vec<String> {
    premises.iter().map(Premise::render).collect()
}

fn render_hop(hop: &SourceSelectionHop) -> CoverageHop {
    CoverageHop {
        from_class: hop.from_class.clone(),
        to_class: hop.to_class.clone(),
        premises: render(&hop.premises),
    }
}

/// The wire form of one internal verdict; premises are rendered, never dropped.
pub(crate) fn wire_verdict(verdict: &CoverageResult) -> CoverageVerdict {
    match verdict {
        CoverageResult::Covered {
            class,
            facet,
            source,
            mapping_name,
            alternates,
            path,
            premises,
            mode,
        } => CoverageVerdict::Covered {
            class: class.clone(),
            facet: facet.clone(),
            source: source.clone(),
            mapping_name: mapping_name.clone(),
            alternates: alternates.clone(),
            path: path.iter().map(render_hop).collect(),
            premises: render(premises),
            mode: *mode,
        },
        CoverageResult::Uncovered { class, premises } => CoverageVerdict::Uncovered {
            class: class.clone(),
            premises: render(premises),
        },
        CoverageResult::Disconnected { class } => CoverageVerdict::Disconnected {
            class: class.clone(),
        },
        CoverageResult::Abstained {
            class,
            facet,
            reason,
            source,
            premises,
        } => CoverageVerdict::Abstained {
            class: class.clone(),
            facet: facet.clone(),
            reason: *reason,
            source: source.clone(),
            premises: render(premises),
        },
    }
}

/// The request's refusals, checked before any read: `(code, detail)`.
pub(crate) fn validate(
    classes: &[String],
    hop_budget: Option<u32>,
    default_hop_budget: u64,
    max_classes: u64,
) -> Result<usize, (&'static str, String)> {
    if classes.is_empty() {
        return Err((
            SOURCE_COVERAGE_NO_CLASSES,
            "name at least one class to cover".to_string(),
        ));
    }
    if classes.len() as u64 > max_classes {
        return Err((
            SOURCE_COVERAGE_TOO_MANY_CLASSES,
            format!(
                "{} classes named; the limit is {max_classes}",
                classes.len()
            ),
        ));
    }
    let hops = hop_budget.map_or(default_hop_budget, u64::from);
    if hops > u64::from(MAX_HOP_BUDGET) {
        return Err((
            SOURCE_COVERAGE_HOP_BUDGET_INVALID,
            format!("hop budget {hops} is outside [0, {MAX_HOP_BUDGET}]"),
        ));
    }
    Ok(hops as usize)
}

/// Every authority-row source outside `connected` is declared absent from the
/// foreign catalog, so selection abstains `Unconnected` on it (R004.6).
pub(crate) fn mark_unconnected(
    fixture: CoverageFixture,
    connected: &ConnectedSources,
) -> CoverageFixture {
    let absent: BTreeSet<String> = fixture
        .rows()
        .map(|row| row.source.clone())
        .filter(|source| !connected.contains(source))
        .collect();
    absent.iter().fold(fixture, |fixture, source| {
        fixture.with_unconnected_source(source)
    })
}

/// Serve one `SourceCoverage`.
pub(crate) async fn handle_source_coverage(
    ctx: &QueryHandlerCtx<'_>,
    classes: Vec<String>,
    facets: Vec<SourceCoverageFacet>,
    hop_budget: Option<u32>,
    needs_current_state: bool,
) -> Response {
    let req_id = ctx.req_id;
    let knobs = medulla_server_api::knob_config::knobs();
    let hops = match validate(
        &classes,
        hop_budget,
        knobs.source_coverage_hop_budget,
        knobs.source_coverage_max_classes,
    ) {
        Ok(hops) => hops,
        Err((code, detail)) => return Response::err(req_id, format!("{code}: {detail}")),
    };
    let Some(carrier) = ctx.read_authority.and_then(GraphReadAuthority::carrier) else {
        return Response::err(
            req_id,
            "ACCESS_DENIED: source coverage needs a verified caller",
        );
    };
    let tenant = carrier.log_owner().0.to_string();
    let sources = ctx.core.schema_sources();
    let parsed = compute_off_lock(req_id, move || {
        let fixture = fixture_from_schema_sources(&sources)?;
        let documents: Vec<&str> = sources
            .core
            .values()
            .chain(sources.dynamic.values())
            .filter_map(|source| source.ontology_ttl.as_deref())
            .collect();
        let approved = connected_sources_from_turtle(&documents)?;
        Ok::<_, String>((fixture, approved))
    })
    .await;
    let (fixture, approved) = match parsed {
        Err(response) => return response,
        Ok(Err(error)) => return Response::err(req_id, format!("INVALID_ARGUMENT: {error}")),
        Ok(Ok(pair)) => pair,
    };
    let connected = {
        let state = ctx.state.read().await;
        let registry = state
            .foreign_sources()
            .registry_for(carrier, &state.isolation);
        ConnectedSources::from_registry(registry.registry(), &approved)
    };
    let fixture = mark_unconnected(fixture, &connected);
    let now_epoch_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let query = SelectionQuery {
        tenant: Some(tenant),
        partition: Some(ctx.graph_name.to_string()),
        now_epoch_seconds,
        needs_current_state,
        role_order: role_order(),
        facets: facets
            .into_iter()
            .map(|facet| (facet.class, facet.property))
            .collect(),
        ..SelectionQuery::default()
    };
    let verdicts = select_sources_with(&fixture, &classes, hops, &query);
    let reply = SourceCoverageReply {
        audit: audit_from(&verdicts),
        verdicts: verdicts.iter().map(wire_verdict).collect(),
    };
    Response::ok(req_id, ResultPayload::of::<SourceCoverage>(reply))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_identity_types::authority_audit::NotUsedReason;

    // spec: MEDULLA-SRC-AUTH-R004.6.1
    #[test]
    fn validation_names_each_refusal_and_resolves_the_budget() {
        let classes = vec!["c".to_string()];
        assert_eq!(validate(&classes, None, 3, 64), Ok(3));
        assert_eq!(validate(&classes, Some(0), 3, 64), Ok(0));
        assert_eq!(validate(&classes, Some(8), 3, 64), Ok(8));
        let code = |result: Result<usize, (&str, String)>| result.unwrap_err().0;
        assert_eq!(code(validate(&[], None, 3, 64)), SOURCE_COVERAGE_NO_CLASSES);
        assert_eq!(
            code(validate(&["a".into(), "b".into()], None, 3, 1)),
            SOURCE_COVERAGE_TOO_MANY_CLASSES
        );
        assert_eq!(
            code(validate(&classes, Some(9), 3, 64)),
            SOURCE_COVERAGE_HOP_BUDGET_INVALID
        );
    }

    // spec: MEDULLA-SRC-AUTH-R004.6
    #[test]
    fn a_row_source_outside_the_view_abstains_unconnected() {
        let fixture = CoverageFixture::new()
            .with_authority("C", "m-a", "a-mcp", 1, 1)
            .with_authority("C", "m-b", "b-mcp", 1, 2);
        let registry = medulla_federation::federation::ForeignSourceRegistry::new();
        let approved: BTreeSet<String> = ["a-mcp".to_string(), "b-mcp".to_string()].into();
        let connected = ConnectedSources::from_registry(&registry, &approved);
        assert_eq!(
            connected.names().count(),
            0,
            "an empty registry connects nothing"
        );
        let fixture = mark_unconnected(fixture, &connected);
        let verdicts = select_sources_with(&fixture, &["C".to_string()], 0, &Default::default());
        let wire: Vec<CoverageVerdict> = verdicts.iter().map(wire_verdict).collect();
        assert!(wire.iter().any(|verdict| matches!(
            verdict,
            CoverageVerdict::Abstained {
                reason: NotUsedReason::Unconnected,
                ..
            }
        )));
    }
}
