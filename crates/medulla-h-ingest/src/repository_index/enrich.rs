//! MEDULLA-REPO-INGEST-R600.3 — post-commit code enrichment of a scoped
//! `IndexRepository` batch.
//!
//! The index commit carries ONE durable follow-on work item: an outbox intent
//! on [`TOPIC`] whose payload is only the index digest and its scope. It is
//! part of the index envelope's own batch, so it becomes claimable exactly
//! when the index commits and is never lost between the two; the index
//! response never waits for it. The in-process consumer ([`spawn`]) claims
//! the item through the durable outbox claim path, reads the committed index
//! envelope by digest, rebuilds [`EnrichInput`] from the committed rows (the
//! symbols, their call edges, the R011 communities computed over that call
//! graph by the shared detector, and the R801 package and layer facts), runs
//! [`medulla_enrich::code::enrich_repository`] under
//! `SchedulerConfig.enrich_timeout_ms`, and commits the output as the second
//! envelope `repository-enrich:<digest>` through the public envelope
//! authority. The envelope id is the index digest, so a replayed claim
//! replays the same envelope instead of committing twice. Hooks of R601.3,
//! R602.3 and R603.3 attach inside [`run_after_commit`].

use std::collections::{BTreeMap, BTreeSet, HashMap};

use medulla_enrich::code::{
    CodeEntity, CodeKind, EdgeRelation, EnrichInput, EnrichOutput, EnrichRunReport, EnrichmentEdge,
    Rung, StructuralFacts, TestQualityThresholds,
};
use medulla_enrich::Id;
use medulla_mutation_types::mutation_scope::MutationOutboxIntent;
use medulla_protocol_types::change_envelope::ChangeEnvelope;
use medulla_protocol_types::protocol::Method;
use serde::{Deserialize, Serialize};

use super::ecosystem::{DEPENDS_ON_PACKAGE, IN_LAYER, LAYER_TYPE, PACKAGE_TYPE};
use super::IndexWriteSet;

/// The second envelope's id prefix: `repository-enrich:<index digest>`.
pub const ENVELOPE_PREFIX: &str = "repository-enrich:";
/// The outbox topic of the follow-on work item.
pub const TOPIC: &str = "repository.enrich.code";
/// The durable consumer that claims the follow-on work item.
pub const CONSUMER: &str = "repository-enrich-code-v1";
/// The fixed service actor the consumer commits under.
pub const SERVICE: &str = "repository-enrich-code-v1";
/// Node type of one enrichment run (one per index envelope).
pub const RUN_TYPE: &str = "RepositoryEnrichmentRun";
/// Node type of one feature (one call-graph community).
pub const FEATURE_TYPE: &str = "CodeFeature";
/// Node type of one design pattern.
pub const PATTERN_TYPE: &str = "DesignPattern";
const INDEX_PREFIX: &str = "repository-index:";
const FOLLOW_ON_SCHEMA: u16 = 1;
const MAX_FOLLOW_ON_BYTES: usize = 16 * 1024;

/// The scope a follow-on work item enriches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowOnScope {
    pub tenant: String,
    pub graph: String,
    pub repository_id: String,
}

/// The follow-on work item: the index digest and its scope, nothing else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowOn {
    pub schema_version: u16,
    pub index_digest: String,
    pub scope: FollowOnScope,
}

impl FollowOn {
    /// The committed index envelope this item enriches.
    pub fn index_envelope_id(&self) -> String {
        format!("{INDEX_PREFIX}{}", self.index_digest)
    }

    /// The enrichment envelope this item commits.
    pub fn enrich_envelope_id(&self) -> String {
        format!("{ENVELOPE_PREFIX}{}", self.index_digest)
    }
}

fn digest_of(index_envelope_id: &str) -> Result<&str, String> {
    index_envelope_id
        .strip_prefix(INDEX_PREFIX)
        .filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
        .ok_or_else(|| "CONFLICT: repository enrichment needs a repository-index envelope".into())
}

/// The follow-on work item the index commit carries in its own outbox.
pub fn follow_on_intent(
    index_envelope_id: &str,
    scope: FollowOnScope,
) -> Result<MutationOutboxIntent, String> {
    let item = FollowOn {
        schema_version: FOLLOW_ON_SCHEMA,
        index_digest: digest_of(index_envelope_id)?.to_string(),
        scope,
    };
    let payload = rmp_serde::to_vec_named(&item)
        .map_err(|error| format!("repository enrichment work item encoding failed: {error}"))?;
    Ok(MutationOutboxIntent {
        topic: TOPIC.to_string(),
        key: item.enrich_envelope_id(),
        payload,
        headers: BTreeMap::new(),
    })
}

/// Decode a claimed follow-on work item.
pub fn decode_follow_on(intent: &MutationOutboxIntent) -> Result<FollowOn, String> {
    if intent.topic != TOPIC || intent.payload.len() > MAX_FOLLOW_ON_BYTES {
        return Err("CONFLICT: repository enrichment work item is invalid".into());
    }
    let item: FollowOn = rmp_serde::from_slice(&intent.payload)
        .map_err(|_| "CONFLICT: repository enrichment work item payload is invalid")?;
    if item.schema_version != FOLLOW_ON_SCHEMA
        || intent.key != item.enrich_envelope_id()
        || digest_of(&item.index_envelope_id()).is_err()
    {
        return Err("CONFLICT: repository enrichment work item is invalid".into());
    }
    Ok(item)
}

/// One committed row of the index envelope.
enum Row {
    Node {
        id: String,
        properties: BTreeMap<String, String>,
    },
    Edge {
        source: String,
        target: String,
        relationship: String,
    },
}

fn decode_properties(bytes: &[u8]) -> BTreeMap<String, String> {
    rmp_serde::from_slice::<BTreeMap<String, String>>(bytes).unwrap_or_default()
}

fn committed_rows(index: &ChangeEnvelope) -> Vec<Row> {
    index
        .mutation
        .operations
        .iter()
        .filter_map(|operation| match &operation.method {
            Method::AddNode {
                node_id,
                properties_msgpack,
            } => Some(Row::Node {
                id: node_id.clone(),
                properties: decode_properties(properties_msgpack),
            }),
            Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            } => Some(Row::Edge {
                source: source_id.clone(),
                target: target_id.clone(),
                relationship: decode_properties(properties_msgpack)
                    .remove("relationship")
                    .unwrap_or_default(),
            }),
            _ => None,
        })
        .collect()
}

fn code_kind(symbol_type: &str) -> Option<CodeKind> {
    match symbol_type {
        "Module" => Some(CodeKind::Module),
        "Class" | "Struct" | "Interface" | "Trait" => Some(CodeKind::Class),
        "Function" => Some(CodeKind::Function),
        "Method" => Some(CodeKind::Method),
        _ => None,
    }
}

fn relation(edge_type: &str) -> Option<(EdgeRelation, Rung)> {
    match edge_type {
        "calls" => Some((EdgeRelation::Calls, Rung::Resolved)),
        "inherits" | "realizes" | "implements" => {
            Some((EdgeRelation::Implements, Rung::Structural))
        }
        "contains" | "hasSymbol" => Some((EdgeRelation::Contains, Rung::Structural)),
        "uses" | "imports" => Some((EdgeRelation::Uses, Rung::Lexical)),
        _ => None,
    }
}

/// The R011 communities of the committed call graph, through the shared
/// community detector (no second detector), labelled under `digest`.
fn communities(
    symbols: &BTreeSet<String>,
    calls: &[(String, String)],
    digest: &str,
) -> Result<BTreeMap<Id, String>, String> {
    let core = medulla_graph_core::graph::GraphCore::new();
    let type_props = rmp_serde::to_vec_named(&BTreeMap::from([("type", "SYMBOL")]))
        .map_err(|error| error.to_string())?;
    for symbol in symbols {
        core.add_node(symbol.clone(), type_props.clone());
    }
    for (source, target) in calls {
        if symbols.contains(source) && symbols.contains(target) {
            core.add_edge(source.clone(), target.clone(), type_props.clone())
                .map_err(|error| format!("repository enrichment call graph: {error}"))?;
        }
    }
    let view = core.analysis_snapshot();
    let groups = medulla_graph_algos::algorithms::community_detection(&view, 1.0);
    let label = &digest[..16.min(digest.len())];
    let mut assignment = BTreeMap::new();
    for (index, members) in groups.iter().enumerate() {
        if members.len() < 2 {
            continue;
        }
        for member in members {
            let id = Id::new(member.clone()).map_err(|error| error.to_string())?;
            assignment.insert(id, format!("{label}-c{index}"));
        }
    }
    Ok(assignment)
}

/// The facts the enrichment run reads, all from the committed index envelope.
pub struct CommittedFacts {
    pub input: EnrichInput,
    pub packages: BTreeSet<String>,
    pub layers: BTreeSet<String>,
    pub package_layers: BTreeMap<String, String>,
    pub package_dependencies: BTreeSet<(String, String)>,
}

/// Rebuild the enrichment input from the committed index envelope of `digest`.
pub fn committed_facts(index: &ChangeEnvelope, digest: &str) -> Result<CommittedFacts, String> {
    let rows = committed_rows(index);
    let mut entities = Vec::new();
    let mut symbols = BTreeSet::new();
    let mut qualified: Vec<(String, CodeKind, String)> = Vec::new();
    let mut packages = BTreeSet::new();
    let mut layers = BTreeSet::new();
    for row in &rows {
        let Row::Node { id, properties } = row else {
            continue;
        };
        match properties.get("type").map(String::as_str) {
            Some("SYMBOL") => {
                let Some(kind) = properties
                    .get("symbol_type")
                    .and_then(|kind| code_kind(kind))
                else {
                    continue;
                };
                let Ok(entity_id) = Id::new(id.clone()) else {
                    continue;
                };
                symbols.insert(id.clone());
                qualified.push((
                    id.clone(),
                    kind,
                    properties
                        .get("qualified_symbol")
                        .cloned()
                        .unwrap_or_default(),
                ));
                entities.push((
                    CodeEntity {
                        id: entity_id,
                        kind,
                        name: properties.get("name").cloned().unwrap_or_default(),
                        path: properties.get("file_path").cloned().unwrap_or_default(),
                        language: properties.get("language").cloned().unwrap_or_default(),
                    },
                    StructuralFacts::default(),
                ));
            }
            Some(PACKAGE_TYPE) => {
                packages.insert(id.clone());
            }
            Some(LAYER_TYPE) => {
                layers.insert(id.clone());
            }
            _ => {}
        }
    }
    // A class's methods are the method/function symbols qualified under it.
    let mut methods: HashMap<&str, Vec<String>> = HashMap::new();
    for (_, kind, name) in &qualified {
        if matches!(kind, CodeKind::Method | CodeKind::Function) {
            for separator in [".", "::"] {
                if let Some((owner, method)) = name.rsplit_once(separator) {
                    methods.entry(owner).or_default().push(method.to_string());
                }
            }
        }
    }
    for ((entity, facts), (_, _, name)) in entities.iter_mut().zip(&qualified) {
        if entity.kind == CodeKind::Class {
            facts.methods = methods.get(name.as_str()).cloned().unwrap_or_default();
        }
    }
    let mut edges = Vec::new();
    let mut calls = Vec::new();
    let mut package_layers = BTreeMap::new();
    let mut package_dependencies = BTreeSet::new();
    for row in &rows {
        let Row::Edge {
            source,
            target,
            relationship,
        } = row
        else {
            continue;
        };
        if relationship == IN_LAYER && packages.contains(source) {
            package_layers.insert(source.clone(), target.clone());
            continue;
        }
        if relationship == DEPENDS_ON_PACKAGE {
            package_dependencies.insert((source.clone(), target.clone()));
            continue;
        }
        if !(symbols.contains(source) && symbols.contains(target)) {
            continue;
        }
        let Some((relation, rung)) = relation(relationship) else {
            continue;
        };
        if relation == EdgeRelation::Calls {
            calls.push((source.clone(), target.clone()));
        }
        let (Ok(from), Ok(to)) = (Id::new(source.clone()), Id::new(target.clone())) else {
            continue;
        };
        edges.push(EnrichmentEdge {
            from,
            to,
            relation,
            rung,
        });
    }
    let communities = communities(&symbols, &calls, digest)?;
    Ok(CommittedFacts {
        input: EnrichInput {
            entities,
            tests: Vec::new(),
            edges,
            communities,
        },
        packages,
        layers,
        package_layers,
        package_dependencies,
    })
}

/// A value as the attested material rule admits it.
fn attested(value: &str) -> &str {
    if medulla_protocol_types::change_envelope::admits_attested_text(value) {
        value
    } else {
        super::REDACTED_HOST_IDENTITY
    }
}

fn props(pairs: &[(&str, String)]) -> Result<Vec<u8>, String> {
    let ordered: BTreeMap<&str, &str> = pairs.iter().map(|(k, v)| (*k, attested(v))).collect();
    rmp_serde::to_vec_named(&ordered)
        .map_err(|error| format!("repository enrichment property encoding failed: {error}"))
}

fn add_node(id: String, pairs: &[(&str, String)]) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id: id,
        properties_msgpack: props(pairs)?,
    })
}

fn add_edge(source: String, target: String, relationship: &str) -> Result<Method, String> {
    Ok(Method::AddEdge {
        source_id: source,
        target_id: target,
        properties_msgpack: props(&[("relationship", relationship.to_string())])?,
    })
}

/// Lower one run's output to the methods of the enrichment envelope.
fn lower_output(
    item: &FollowOn,
    facts: &CommittedFacts,
    output: &EnrichOutput,
    report: &EnrichRunReport,
) -> Result<Vec<Method>, String> {
    let run_id = format!("repository-enrich-run:{}", item.index_digest);
    let mut methods = vec![add_node(
        run_id.clone(),
        &[
            ("type", RUN_TYPE.to_string()),
            ("index_envelope", item.index_envelope_id()),
            ("repository_id", item.scope.repository_id.clone()),
            ("entities", report.entities.to_string()),
            ("patterns", report.patterns.to_string()),
            ("features", report.features.to_string()),
            ("tests_flagged", report.tests_flagged.to_string()),
            (
                "rung_conflicts_refused",
                report.rung_conflicts_refused.to_string(),
            ),
            ("packages", facts.packages.len().to_string()),
            ("layers", facts.layers.len().to_string()),
        ],
    )?];
    for feature in &output.features {
        let feature_id = feature.id.as_str().to_string();
        methods.push(add_node(
            feature_id.clone(),
            &[
                ("type", FEATURE_TYPE.to_string()),
                ("members", feature.members.len().to_string()),
            ],
        )?);
        methods.push(add_edge(
            run_id.clone(),
            feature_id.clone(),
            "producedFeature",
        )?);
        for member in &feature.members {
            methods.push(add_edge(
                member.as_str().to_string(),
                feature_id.clone(),
                "memberOfFeature",
            )?);
        }
        // A feature belongs to the layers of the packages its members are in;
        // R601+ hooks refine this. The run records the package facts read.
    }
    let mut patterns = BTreeSet::new();
    for hit in &output.patterns {
        let pattern = serde_json::to_value(hit.pattern)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        let pattern_id = format!("design-pattern:{pattern}");
        if patterns.insert(pattern_id.clone()) {
            methods.push(add_node(
                pattern_id.clone(),
                &[("type", PATTERN_TYPE.to_string()), ("name", pattern)],
            )?);
        }
        methods.push(add_edge(
            hit.entity.as_str().to_string(),
            pattern_id,
            "usesPattern",
        )?);
    }
    for package in &facts.packages {
        methods.push(add_edge(run_id.clone(), package.clone(), "readPackage")?);
    }
    Ok(methods)
}

/// Run the enrichment of one claimed follow-on work item over its committed
/// index envelope and lower the output as the `repository-enrich:<digest>`
/// write-set. R601.3, R602.3 and R603.3 attach their hooks here.
pub fn run_after_commit(index: &ChangeEnvelope, item: &FollowOn) -> Result<IndexWriteSet, String> {
    if index.envelope_id != item.index_envelope_id() {
        return Err("CONFLICT: repository enrichment read the wrong index envelope".into());
    }
    let facts = committed_facts(index, &item.index_digest)?;
    let (output, report) =
        medulla_enrich::code::enrich_repository(&facts.input, &TestQualityThresholds::default())
            .map_err(|error| error.to_string())?;
    let methods = lower_output(item, &facts, &output, &report)?;
    IndexWriteSet::with_digest(ENVELOPE_PREFIX, &item.index_digest, methods)
}

/// The labels the enrichment envelope is sealed under.
pub const ENRICH_LABELS: super::EnvelopeLabels = super::EnvelopeLabels {
    version_type: "repository-enrich-sha256",
    policy_version: "repository-enrich",
};

/// Spawn the in-process consumer of the follow-on work items. The timeout is
/// read and bounds-checked here, so an out-of-range value refuses startup.
pub fn spawn(
    state: std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
) -> Result<(), String> {
    let timeout_ms =
        medulla_types::scheduler_config::enrich_timeout_ms_from(|key| std::env::var(key).ok())
            .map_err(|error| error.to_string())?;
    #[cfg(all(feature = "redb", not(feature = "raft")))]
    tokio::spawn(worker::run(state, timeout_ms));
    #[cfg(not(all(feature = "redb", not(feature = "raft"))))]
    drop((state, timeout_ms));
    Ok(())
}

#[cfg(all(feature = "redb", not(feature = "raft")))]
mod worker {
    use std::sync::Arc;
    use std::time::Duration;

    use medulla_mutation_types::mutation_scope::MutationOutboxLease;
    use medulla_protocol_types::change_envelope::MaterialClass;
    use medulla_protocol_types::protocol::{Method, Request};
    use medulla_server_api::api::persistence::PersistenceBackend;
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_server_api::state::ServerState;
    use tokio::sync::RwLock;

    use super::{decode_follow_on, run_after_commit, FollowOn, CONSUMER, SERVICE, TOPIC};

    const IDLE: Duration = Duration::from_millis(500);
    const LEASE_MS: u64 = 10 * 60 * 1_000;
    const MAX_GRAPHS_PER_SWEEP: usize = 16;

    pub(super) async fn run(state: Arc<RwLock<ServerState>>, timeout_ms: u64) {
        let mut next_graph = 0_usize;
        loop {
            match sweep(&state, &mut next_graph, timeout_ms).await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(IDLE).await,
                Err(error) => {
                    tracing::warn!(code = "REPOSITORY_ENRICH_SWEEP_FAILED", %error,
                        "repository code enrichment sweep failed");
                    tokio::time::sleep(IDLE).await;
                }
            }
        }
    }

    async fn sweep(
        state: &Arc<RwLock<ServerState>>,
        next_graph: &mut usize,
        timeout_ms: u64,
    ) -> Result<bool, String> {
        let mut graphs: Vec<String> = state
            .read()
            .await
            .registry
            .list()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        if graphs.is_empty() {
            *next_graph = 0;
            return Ok(false);
        }
        graphs.sort_unstable();
        let start = *next_graph % graphs.len();
        let count = graphs.len().min(MAX_GRAPHS_PER_SWEEP);
        *next_graph = (start + count) % graphs.len();
        let mut progressed = false;
        let mut first_error = None;
        for offset in 0..count {
            let graph = &graphs[(start + offset) % graphs.len()];
            match sweep_graph(state, graph, timeout_ms).await {
                Ok(true) => progressed = true,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(progressed), Err)
    }

    async fn sweep_graph(
        state: &Arc<RwLock<ServerState>>,
        graph: &str,
        timeout_ms: u64,
    ) -> Result<bool, String> {
        let persistence = state
            .read()
            .await
            .persistence
            .clone()
            .ok_or("CONFLICT: repository enrichment persistence is unavailable")?;
        let fname = medulla_kernel::persist::sanitize(graph);
        persistence
            .subscribe_mutation_outbox(&fname, CONSUMER, TOPIC)
            .await?;
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let mut budget = medulla_transaction::OutboxClaimBudget::new(1, LEASE_MS, now_ms)?;
        let outcome = persistence
            .claim_mutation_outbox(&fname, CONSUMER, &mut budget)
            .await?;
        let mut progressed = false;
        for lease in outcome.claims {
            match claimed(state, persistence.as_ref(), graph, &lease, timeout_ms).await {
                Ok(()) => {
                    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
                    persistence.ack_mutation_outbox(&fname, &lease, now).await?;
                    progressed = true;
                }
                Err(error) => {
                    // The item stays durable: release it for a later claim.
                    persistence.release_mutation_outbox(&fname, &lease).await?;
                    return Err(error);
                }
            }
        }
        Ok(progressed)
    }

    /// Run one claimed work item: read the committed index by digest, enrich
    /// under the timeout, and commit the second envelope.
    async fn claimed(
        state: &Arc<RwLock<ServerState>>,
        persistence: &dyn PersistenceBackend,
        graph: &str,
        lease: &MutationOutboxLease,
        timeout_ms: u64,
    ) -> Result<(), String> {
        let item = decode_follow_on(&lease.record.intent)?;
        if item.scope.graph != graph {
            return Err("CONFLICT: repository enrichment work item names another graph".into());
        }
        let fname = medulla_kernel::persist::sanitize(graph);
        if persistence
            .read_change_envelope(&fname, &item.enrich_envelope_id())
            .await?
            .is_some()
        {
            return Ok(());
        }
        let index = persistence
            .read_change_envelope(&fname, &item.index_envelope_id())
            .await?
            .ok_or("CONFLICT: repository enrichment index envelope is not committed")?
            .envelope;
        let run_item = item.clone();
        let write_set = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            tokio::task::spawn_blocking(move || run_after_commit(&index, &run_item)),
        )
        .await
        .map_err(|_| format!("UNAVAILABLE: repository enrichment exceeded {timeout_ms} ms"))?
        .map_err(|error| format!("repository enrichment run failed: {error}"))??;
        commit(state, graph, &item, write_set).await
    }

    async fn service_context(
        state: &Arc<RwLock<ServerState>>,
        item: &FollowOn,
    ) -> Result<VerifiedRequestContext, String> {
        let key = item.enrich_envelope_id();
        let anonymous = state.read().await.local_anonymous;
        #[cfg(feature = "security")]
        if !anonymous {
            let mut state = state.write().await;
            if state.isolation.identity_bootstrap_pending() {
                return Err("REPOSITORY_ENRICH_POLICY_UNBOOTSTRAPPED".into());
            }
            medulla_server_api::service_grant::ensure_exact_graph_grant(
                &mut state.isolation,
                &item.scope.graph,
                &format!("service:{SERVICE}"),
                format!("repository-enrich-code:{}", item.scope.graph),
            )?;
        }
        let verified = if anonymous {
            VerifiedRequestContext::anonymous_local(Some(key))
        } else {
            let service = VerifiedRequestContext::authenticated_fixed_service_actor(
                SERVICE,
                &["graph:write", "repository:enrich:commit"],
            )?;
            VerifiedRequestContext::from_verified_claims_with_nonce(
                service.claims().clone(),
                key,
                Some(medulla_types::contract::Nonce::minted()),
            )
        };
        // The envelope apply boundary binds the compiled tenant scope to this
        // caller's own verified scope and refuses a mismatch.
        Ok(verified)
    }

    /// Commit the enrichment envelope through the public envelope authority,
    /// the same chokepoint every envelope apply enters.
    async fn commit(
        state: &Arc<RwLock<ServerState>>,
        graph: &str,
        item: &FollowOn,
        write_set: crate::repository_index::IndexWriteSet,
    ) -> Result<(), String> {
        let verified = service_context(state, item).await?;
        let graph_version = state
            .read()
            .await
            .registry
            .get(graph)
            .ok_or("CONFLICT: repository enrichment graph was retired")?
            .core
            .version();
        let ctx = crate::repository_index::IndexCommitContext {
            request_id: 0,
            graph_name: graph,
            tenant_scope: &item.scope.tenant,
            verified: &verified,
            graph_version,
            placement_epoch: 0,
            fencing_token: None,
        };
        let mut envelope = crate::repository_index::build_labelled_envelope(
            &ctx,
            write_set,
            &super::ENRICH_LABELS,
        )?;
        envelope.material_class = MaterialClass::Attested;
        envelope.validate()?;
        let response = medulla_server_api::api::reentry::dispatch_verified_request(
            state,
            Request {
                id: 0,
                graph: graph.to_string(),
                auth_token: String::new(),
                agent_id: None,
                method: Method::ApplyChangeEnvelope {
                    envelope: Box::new(envelope),
                },
            },
            verified,
        )
        .await;
        match response.refusal_text() {
            Some(refusal) => Err(refusal),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> FollowOnScope {
        FollowOnScope {
            tenant: "tenant-a".into(),
            graph: "graph-a".into(),
            repository_id: "repo".into(),
        }
    }

    // spec: MEDULLA-REPO-INGEST-R600.3
    #[test]
    fn follow_on_carries_only_the_digest_and_scope_and_names_one_envelope() {
        let index_id = format!("{INDEX_PREFIX}{}", "a".repeat(64));
        let intent = follow_on_intent(&index_id, scope()).expect("intent");
        assert_eq!(intent.topic, TOPIC);
        let item = decode_follow_on(&intent).expect("decodes");
        assert_eq!(item.index_envelope_id(), index_id);
        assert_eq!(
            item.enrich_envelope_id(),
            format!("{ENVELOPE_PREFIX}{}", "a".repeat(64))
        );
        assert!(follow_on_intent("source-ingest:x", scope()).is_err());
        let mut forged = intent.clone();
        forged.key = format!("{ENVELOPE_PREFIX}{}", "b".repeat(64));
        assert!(decode_follow_on(&forged).is_err());
    }
}
