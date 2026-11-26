//! The `graph_intake` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Resolves graph references to their exact revision and content digest
//! (MEDULLA-DECISION-ENGINE-R163.2, R163.3, R163.4).
//!
//! The caller sends its mixed sources in the typed param `intake_sources` and
//! names the graph its graph sources resolve in with `graph`; the executor opens
//! that graph under the caller's access check and row-level filter. The family
//! resolves every source in request order and concludes
//! `ResolutionKind::Constraint`: one `resolved_set` option and one premise per
//! resolved source naming its exact revision and digest, never its content. One
//! source that does not resolve abstains the whole request with its registered
//! code (a sealed `Abstained` record). `Decide` writes nothing.

use medulla_decision_types::decision::graph_intake::{
    GraphReference, GraphResolutionError, IntakeSource, ResolvedSource, PARAM_GRAPH,
    PARAM_SOURCES,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::Digest256;
use medulla_types::evidence_class::EvidenceClass;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained, advisory, invalid, param, single_option, text_param};
use super::DecisionFamily;

/// The one declared option: resolve the request's sources.
pub const RESOLVE_OPTION: &str = "resolved_set";

/// The node type that registers an authority in the graph.
const AUTHORITY_NODE_TYPE: &str = "SourceAuthority";

/// The family of the `graph_intake` question kind.
pub struct GraphIntakeFamily;

/// Whether any source is read from the graph.
fn reads_graph(sources: &[IntakeSource]) -> bool {
    sources
        .iter()
        .any(|source| matches!(source, IntakeSource::Graph { .. }))
}

/// The graph the graph sources resolve in: the text param `graph`.
fn graph_param(request: &DecideRequest) -> Result<Option<&str>, String> {
    Ok(text_param(request, PARAM_GRAPH)?.filter(|graph| !graph.trim().is_empty()))
}

/// Open the graph the sources resolve in before any read, through the caller's
/// ACL and row-level filter (MEDULLA-DECISION-ENGINE-R163.3). `None` when every
/// source is a file; a graph source without a `graph` param refuses
/// `PARAMETER_INVALID`.
pub(in crate::decide) async fn intake_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<GraphView>, String> {
    if !reads_graph(sources(request)?) {
        return Ok(None);
    }
    let graph = graph_param(request)?
        .ok_or_else(|| invalid("a graph source requires the graph parameter"))?;
    crate::decide::stat_support::filtered_view(state, agent_id, graph)
        .await
        .map(Some)
}

/// The intake sources of the request: the typed `intake_sources` param.
fn sources(request: &DecideRequest) -> Result<&[IntakeSource], String> {
    match param(request, PARAM_SOURCES)? {
        Some(TypedValue::IntakeSources(sources)) if !sources.is_empty() => Ok(sources.as_slice()),
        Some(TypedValue::IntakeSources(_)) => Err(invalid("intake_sources is empty")),
        Some(_) => Err(invalid("intake_sources is not a list of intake sources")),
        None => Err(invalid("a graph_intake question requires intake_sources")),
    }
}

fn premise(subject: &'static str, fact: String, source: FamilyPremiseSource) -> FamilyPremise {
    FamilyPremise {
        subject: subject.into(),
        fact,
        class: PremiseClass::Claim,
        source,
    }
}

impl DecisionFamily for GraphIntakeFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, RESOLVE_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let sources = sources(request)?;
        if reads_graph(sources) && graph_param(request)?.is_none() {
            return Err(invalid("a graph source requires the graph parameter"));
        }
        Ok(())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let sources = sources(request)?;
            if reads_graph(sources) && graph_view.is_none() {
                return Err(invalid("the intake graph was not opened"));
            }
            let graph = graph_param(request)?.unwrap_or_default().to_string();
            let empty = GraphView::default();
            let view = graph_view.unwrap_or(&empty);
            let resolved = match resolve_sources(ctx.tenant_id, view, sources) {
                Ok(resolved) => resolved,
                Err(error) => return Ok(abstained(QuestionKind::GraphIntake, &error)),
            };
            let mut executed = advisory(vec![RESOLVE_OPTION.to_string()])?;
            executed.family_premises = resolved
                .iter()
                .map(|source| match source {
                    ResolvedSource::Graph {
                        authority,
                        query,
                        revision,
                        content_digest,
                    } => premise(
                        "intake_graph",
                        format!(
                            "intake:graph={authority}/{query}@{revision}#{}",
                            content_digest.to_hex()
                        ),
                        FamilyPremiseSource::Bound(PremiseProvenance::GraphRow {
                            graph: graph.clone(),
                            graph_version: *revision,
                        }),
                    ),
                    ResolvedSource::Filesystem {
                        path,
                        content_digest,
                    } => premise(
                        "intake_file",
                        format!("intake:file={path}#{}", content_digest.to_hex()),
                        FamilyPremiseSource::Request("params.intake_sources"),
                    ),
                })
                .collect();
            Ok(executed)
        })())
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Claim)
    }
}

/// The properties of one node the caller can see, or `None` when the node is
/// absent from the caller's visible snapshot or its blob does not decode.
fn visible_properties(
    tenant: &str,
    view: &GraphView,
    node_id: &str,
) -> Option<serde_json::Value> {
    let blob = view.node_properties.get(node_id)?;
    let properties = medulla_types::msgpack::decode_property_value(blob.as_slice()).ok()?;
    // A node of another tenant reads as absent.
    let foreign = properties
        .get("tenant_id")
        .and_then(|owner| owner.as_str())
        .is_some_and(|owner| owner != tenant);
    (!foreign).then_some(properties)
}

/// Resolve one graph reference against the caller's tenant-visible snapshot,
/// the same read handle `candidates.rs` reads graph candidates through
/// (MEDULLA-DECISION-ENGINE-R163.2). Returns the exact revision and content
/// digest of the node for the query, never its content.
pub fn resolve_graph_reference(
    tenant: &str,
    view: &GraphView,
    reference: &GraphReference,
) -> Result<ResolvedSource, GraphResolutionError> {
    let registered = visible_properties(tenant, view, &reference.authority).is_some_and(|props| {
        props.get("type").and_then(|v| v.as_str()) == Some(AUTHORITY_NODE_TYPE)
    });
    if !registered {
        return Err(GraphResolutionError::UnknownAuthority {
            authority: reference.authority.clone(),
        });
    }
    let failed = || GraphResolutionError::ResolutionFailed {
        query: reference.query.clone(),
    };
    let node = visible_properties(tenant, view, &reference.query).ok_or_else(failed)?;
    if node.get("authority").and_then(|v| v.as_str()) != Some(reference.authority.as_str()) {
        return Err(failed());
    }
    let missing = || GraphResolutionError::MissingRevision {
        query: reference.query.clone(),
    };
    let revision = node
        .get("revision")
        .and_then(|v| v.as_u64())
        .ok_or_else(missing)?;
    let content_digest = node
        .get("content_digest")
        .and_then(|v| v.as_str())
        .and_then(|hex| Digest256::parse(hex).ok())
        .ok_or_else(missing)?;
    if revision != reference.revision || content_digest != reference.content_digest {
        return Err(failed());
    }
    Ok(ResolvedSource::Graph {
        authority: reference.authority.clone(),
        query: reference.query.clone(),
        revision,
        content_digest,
    })
}

/// Resolve every intake source in request order: a filesystem source passes
/// through with its caller-supplied digest, a graph source resolves through
/// [`resolve_graph_reference`]. One unresolved source fails the whole set.
pub fn resolve_sources(
    tenant: &str,
    view: &GraphView,
    sources: &[IntakeSource],
) -> Result<Vec<ResolvedSource>, GraphResolutionError> {
    sources
        .iter()
        .map(|source| match source {
            IntakeSource::Filesystem {
                path,
                content_digest,
            } => Ok(ResolvedSource::Filesystem {
                path: path.clone(),
                content_digest: *content_digest,
            }),
            IntakeSource::Graph { graph_ref } => resolve_graph_reference(tenant, view, graph_ref),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn put(view: &mut GraphView, id: &str, value: serde_json::Value) {
        view.node_properties.insert(
            id.to_string(),
            Arc::new(rmp_serde::to_vec_named(&value).unwrap()),
        );
    }

    fn view() -> (GraphView, Digest256) {
        let digest = Digest256::sha256(b"paper body");
        let mut view = GraphView::default();
        put(
            &mut view,
            "graph:main",
            serde_json::json!({"type": "SourceAuthority"}),
        );
        put(
            &mut view,
            "paper:1",
            serde_json::json!({
                "authority": "graph:main",
                "revision": 12,
                "content_digest": digest.to_hex(),
                "body": "never returned"
            }),
        );
        put(
            &mut view,
            "paper:norev",
            serde_json::json!({"authority": "graph:main"}),
        );
        (view, digest)
    }

    fn reference(authority: &str, query: &str, revision: u64, digest: Digest256) -> GraphReference {
        GraphReference {
            authority: authority.into(),
            query: query.into(),
            revision,
            content_digest: digest,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R163.2
    #[test]
    fn resolves_in_request_order_and_fails_closed() {
        let (view, digest) = view();
        let file_digest = Digest256::sha256(b"a");
        let sources = vec![
            IntakeSource::Graph {
                graph_ref: reference("graph:main", "paper:1", 12, digest),
            },
            IntakeSource::Filesystem {
                path: "docs/a.md".into(),
                content_digest: file_digest,
            },
        ];
        let resolved = resolve_sources("t", &view, &sources).unwrap();
        assert_eq!(
            resolved,
            vec![
                ResolvedSource::Graph {
                    authority: "graph:main".into(),
                    query: "paper:1".into(),
                    revision: 12,
                    content_digest: digest,
                },
                ResolvedSource::Filesystem {
                    path: "docs/a.md".into(),
                    content_digest: file_digest,
                },
            ]
        );
        assert!(!serde_json::to_string(&resolved)
            .unwrap()
            .contains("never returned"));

        let unknown = reference("graph:other", "paper:1", 12, digest);
        assert_eq!(
            resolve_graph_reference("t", &view, &unknown),
            Err(GraphResolutionError::UnknownAuthority {
                authority: "graph:other".into()
            })
        );
        let missing = reference("graph:main", "paper:norev", 1, digest);
        assert_eq!(
            resolve_graph_reference("t", &view, &missing),
            Err(GraphResolutionError::MissingRevision {
                query: "paper:norev".into()
            })
        );
        for stale in [
            reference("graph:main", "paper:absent", 12, digest),
            reference("graph:main", "paper:1", 11, digest),
            reference("graph:main", "paper:1", 12, file_digest),
        ] {
            assert!(matches!(
                resolve_graph_reference("t", &view, &stale),
                Err(GraphResolutionError::ResolutionFailed { .. })
            ));
        }
        // One unresolved source fails the whole set.
        let bad = vec![
            sources[1].clone(),
            IntakeSource::Graph { graph_ref: unknown },
        ];
        assert!(resolve_sources("t", &view, &bad).is_err());
    }
}
