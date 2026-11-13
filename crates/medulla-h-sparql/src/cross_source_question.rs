//! `Method::CrossSourceQuestion` (MEDULLA-CQ-XSRC-R008.4): serve the three
//! cross-source competency questions from one body.
//!
//! The handler reads the request graph's composed schema sources, builds the
//! source-selection fixture from their `vg:SourceAuthority` rows, marks every
//! authority source that no approved connection declares as unconnected, plans
//! the question's ontology classes through
//! [`medulla_virtual_graph::source_selection::select_sources_with`], and
//! assembles the typed [`CrossSourceReport`]. Sources come only from authority
//! rows, never from the question text. A class whose authoritative source is
//! not connected becomes a typed abstention, never a silent fallback. The
//! method writes nothing.

use std::collections::BTreeSet;

use medulla_graph_core::graph::GraphCore;
use medulla_identity_types::authority_audit::AuditSourceMode;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_source_types::cross_source_report::{
    assemble_cross_source_report, CrossSourceError, CrossSourceItem, CrossSourceQuestionId,
    CrossSourceReport, RelatedValue,
};
use medulla_source_types::supply_chain_report::SourceUsed;
use medulla_virtual_graph::source_selection::{
    connected_sources_from_turtle, fixture_from_schema_sources, select_sources_with,
    CoverageFixture, CoverageResult, Premise, SelectionQuery,
};
use medulla_virtual_graph::supply_chain_impact::abstentions_from;

/// Hop budget of one question's plan: the longest chain of the three questions
/// (image to vulnerability to service to contract) is three object properties.
const QUESTION_HOP_BUDGET: usize = 3;

/// The local name of a class IRI (`...#Service` and `.../Service` give `Service`).
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// The fixture classes the question names, matched by local name; a name no
/// authority row mentions stays as written so the planner reports it.
fn roots_of(fixture: &CoverageFixture, question: CrossSourceQuestionId) -> Vec<String> {
    let known: BTreeSet<&str> = fixture.rows().map(|row| row.class.as_str()).collect();
    question
        .root_classes()
        .iter()
        .map(|name| {
            known
                .iter()
                .find(|class| local_name(class) == *name)
                .map_or_else(|| (*name).to_string(), |class| (*class).to_string())
        })
        .collect()
}

/// Most items one answer carries; a graph holding more subjects answers the first page.
const MAX_ITEMS: usize = 1000;

/// The node property that names the source that served an item's subject.
const SOURCE_PROPERTY: &str = "source";

/// The subject label of a question's answer items. Closed table beside the
/// question ids: the label is the question's ontology subject class.
fn subject_label(question: CrossSourceQuestionId) -> &'static str {
    match question {
        CrossSourceQuestionId::ExposedServices => "Service",
        CrossSourceQuestionId::HostLimitCapabilities => "BusinessCapability",
        CrossSourceQuestionId::ContractIncidents => "Incident",
    }
}

/// One answer item per visible subject node of the question: the node's own
/// properties are the related facts and its `source` property names the source
/// that served it. A subject with no source refuses the whole answer; it is
/// never defaulted.
fn items_of(
    core: &GraphCore,
    read_authority: Option<&GraphReadAuthority>,
    question: CrossSourceQuestionId,
) -> Result<Vec<CrossSourceItem>, CrossSourceError> {
    let mut items = Vec::new();
    for (id, blob) in core.get_nodes_by_label(subject_label(question), MAX_ITEMS) {
        if let Some(authority) = read_authority {
            if !authority.node_visible(core, &id) {
                continue;
            }
        }
        let value = medulla_types::msgpack::decode_property_value(&blob).unwrap_or_default();
        let properties = value.as_object().cloned().unwrap_or_default();
        let source = properties
            .get(SOURCE_PROPERTY)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let related = properties
            .iter()
            .filter(|(name, _)| name.as_str() != SOURCE_PROPERTY)
            .filter_map(|(name, value)| {
                let text = match value {
                    serde_json::Value::String(text) => text.clone(),
                    serde_json::Value::Number(number) => number.to_string(),
                    serde_json::Value::Bool(flag) => flag.to_string(),
                    _ => return None,
                };
                Some(RelatedValue {
                    relation: name.clone(),
                    value: text,
                })
            })
            .collect();
        items.push(CrossSourceItem::try_new(&id, related, source)?);
    }
    Ok(items)
}

/// Plan one question over the request graph's authority rows and assemble its
/// report. Pure over the schema sources, so it is testable without a server.
pub fn answer(
    sources: &medulla_graph_core::graph::GraphSchemaSources,
    question: CrossSourceQuestionId,
    items: Vec<CrossSourceItem>,
) -> Result<CrossSourceReport, String> {
    let mut fixture = fixture_from_schema_sources(sources)?;
    let documents: Vec<&str> = sources
        .core
        .values()
        .chain(sources.dynamic.values())
        .filter_map(|source| source.ontology_ttl.as_deref())
        .collect();
    let connected = connected_sources_from_turtle(&documents)?;
    let authority_sources: BTreeSet<String> =
        fixture.rows().map(|row| row.source.clone()).collect();
    for source in authority_sources.difference(&connected) {
        fixture = fixture.with_unconnected_source(source);
    }
    let roots = roots_of(&fixture, question);
    let results = select_sources_with(
        &fixture,
        &roots,
        QUESTION_HOP_BUDGET,
        &SelectionQuery::default(),
    );
    let mut sources_used = Vec::new();
    for result in &results {
        let CoverageResult::Covered { premises, .. } = result else {
            continue;
        };
        for premise in premises {
            if let Premise::Authority {
                row,
                source,
                class,
                rank,
                ..
            } = premise
            {
                let authority_premise = format!("{row} (rank {rank})");
                // The planner serves live when no copy has a watermark.
                let used = SourceUsed::try_new(
                    source,
                    local_name(class),
                    &authority_premise,
                    AuditSourceMode::Live,
                    None,
                    None,
                )
                .map_err(|error| error.to_string())?;
                sources_used.push(used);
            }
        }
    }
    let abstentions = abstentions_from(&results);
    Ok(assemble_cross_source_report(
        question,
        items,
        sources_used,
        abstentions,
    ))
}

/// Serve one `CrossSourceQuestion` for the request graph.
pub fn handle(
    req_id: u64,
    core: &GraphCore,
    read_authority: Option<&GraphReadAuthority>,
    question: &str,
) -> Response {
    let question = match CrossSourceQuestionId::parse(question) {
        Ok(question) => question,
        Err(refusal) => return Response::err(req_id, refusal.to_string()),
    };
    let sources = medulla_server_ops::api::admin::schema_read::disclosed_sources(core);
    let items = match items_of(core, read_authority, question) {
        Ok(items) => items,
        Err(refusal) => return Response::err(req_id, refusal.to_string()),
    };
    match answer(&sources, question, items) {
        Ok(report) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::query::CrossSourceQuestion>(
                report,
            ),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-CQ-XSRC-R008.4
    #[test]
    fn local_name_takes_the_last_iri_segment() {
        assert_eq!(
            local_name("https://knuckles-team.github.io/ontology/core#Service"),
            "Service"
        );
        assert_eq!(local_name("https://example.org/ns/Host"), "Host");
        assert_eq!(local_name("Plain"), "Plain");
    }
}
