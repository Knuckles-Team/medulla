//! Write/read classification + isolation-ACL enforcement for graph ops.

use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::graph::GraphView;
#[cfg(feature = "security")]
use medulla_graph_core::rls::row_visibility;
use medulla_isolation::isolation::AccessLevel;
use medulla_isolation::isolation::IsolationLayer;
use medulla_types::contract::{Digest256, Nonce};
use medulla_server_api::{carrier::unauthenticated_carrier_denied, carrier::CarrierAuthority, carrier::GraphReadAuthority};
use std::sync::Arc;

#[cfg(all(any(test, feature = "test-support"), feature = "security"))]
pub fn two_agent_isolation() -> IsolationLayer {
    use medulla_types::acl::AgentIdentity;
    use medulla_types::acl::AgentRole;

    let mut isolation = IsolationLayer::new();
    for agent_id in ["alice", "bob"] {
        isolation.register_agent(AgentIdentity {
            agent_id: agent_id.to_string(),
            role: AgentRole::Agent,
            teams: Vec::new(),
            roles: Vec::new(),
        });
    }
    isolation
}


/// One owner-scoped process-global call (EH-373/EH-374): the server state, the request
/// id, and the verified carrier whose owner (tenant+principal) keys a registration
/// catalog. The carrier is derived from the verified envelope only, never a request
/// field.
#[cfg(any(feature = "wasm-udf", feature = "federation"))]
#[derive(Clone, Copy)]
pub struct OwnerScopedCall<'a> {
    pub state: &'a Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    pub req_id: u64,
    pub carrier: Option<&'a CarrierAuthority>,
}

#[cfg(any(feature = "wasm-udf", feature = "federation"))]
impl OwnerScopedCall<'_> {
    /// The verified owner, or the `ACCESS_DENIED` refusal for `method`.
    pub fn owner(
        &self,
        method: &str,
    ) -> Result<CarrierAuthority, medulla_protocol_types::protocol::Response> {
        self.carrier.cloned().ok_or_else(|| {
            medulla_kernel::metrics::access_denied();
            medulla_protocol_types::protocol::Response::err(
                self.req_id,
                format!("ACCESS_DENIED: {method} requires a verified tenant carrier"),
            )
        })
    }
}


mod write_classification;
pub use write_classification::requires_write;

/// Whether a wire `Method::Sql` statement mutates state (CONCEPT:MEDULLA-KG.query.mirrors-pgwire). Reuses the
/// SAME `medulla_sql::sql::classify` the pgwire shim routes on, so a graph-node DML
/// (INSERT/UPDATE/DELETE on `nodes`), a user-table DDL/DML (CREATE/ALTER/DROP TABLE,
/// INSERT/UPDATE/DELETE/COPY on a user table), classify as writes; a `SELECT`/`WITH`/
/// transaction-control statement, or one that does not parse, is not a write.
#[cfg(feature = "query")]
pub fn sql_is_write(query: &str) -> bool {
    use medulla_sql_lang::sql::classify::StatementKind;
    !matches!(
        medulla_sql_lang::sql::classify::classify(query),
        Ok(StatementKind::Read)
            // SQL:2023 `GRAPH_TABLE` is a read over base relations. It is a
            // separate classification only because it needs an authoritative
            // catalog resolution before it can lower; the ACCESS LEVEL it needs
            // is a read's, on every route -- this function is what
            // `requires_write` and the read-only KnowledgeStream gate consult.
            | Ok(StatementKind::GraphTableReadRequiresCatalogAdmission(_))
            | Ok(StatementKind::Begin)
            | Ok(StatementKind::Commit)
            | Ok(StatementKind::Rollback)
            | Err(_)
    )
}

/// Whether a GraphQL document is a MUTATION (a write) rather than a `query`/
/// `subscription` (CONCEPT:MEDULLA-KG.query.mirrors-pgwire). Uses medulla-graphql's own `parse_operation`, so the
/// classification matches the executor exactly; an unparseable document is treated as
/// a non-write (the handler surfaces the parse error on a read snapshot).
#[cfg(feature = "graphql")]
pub fn graphql_is_mutation(query: &str) -> bool {
    matches!(
        medulla_graphql::parse_operation(query),
        Ok(medulla_graphql::Operation::Mutation(_))
    )
}

/// Enforce the isolation ACL for a graph-targeted operation.
///
/// A provisioned identity/RBAC policy is mandatory. Once rules exist,
/// `check_access` decides: peer agent graphs are denied,
/// managers reach subordinate graphs, team graphs are member-read/manager-write,
/// the `__commons__` stays open to all authenticated agents.
pub fn check_graph_access(
    isolation: &IsolationLayer,
    caller: Option<&str>,
    graph_name: &str,
    graph_type: medulla_protocol_types::protocol::GraphType,
    graph_owner: Option<&str>,
    access: AccessLevel,
) -> Result<(), String> {
    check_graph_access_with_policy(
        isolation,
        caller,
        graph_name,
        graph_type,
        graph_owner,
        access,
    )
}

fn check_graph_access_with_policy(
    isolation: &IsolationLayer,
    caller: Option<&str>,
    graph_name: &str,
    graph_type: medulla_protocol_types::protocol::GraphType,
    graph_owner: Option<&str>,
    access: AccessLevel,
) -> Result<(), String> {
    let agent = require_verified_caller(caller)?;
    require_provisioned_policy(isolation)?;
    let query = medulla_isolation::isolation::AccessQuery {
        agent_id: agent,
        graph_name,
        graph_type,
        graph_owner,
        access,
        now_ms: medulla_isolation::isolation::access_clock_ms(),
    };
    if decide_graph_access(isolation, &query) {
        Ok(())
    } else {
        medulla_kernel::metrics::access_denied();
        #[cfg(feature = "security")]
        medulla_server_api::denial_sample::offer(agent, graph_name, "GRAPH_ACCESS_DENIED");
        Err(format!(
            "ACCESS_DENIED: verified principal lacks {access:?} access to graph '{graph_name}'"
        ))
    }
}

/// The served graph-access decision: the isolation chokepoint's answer, with
/// every access that only a just-in-time elevation allowed (EH-404) written
/// to the elevation audit target -- each use, not just the grant.
pub fn decide_graph_access(
    isolation: &IsolationLayer,
    query: &medulla_isolation::isolation::AccessQuery<'_>,
) -> bool {
    let basis = isolation.access_basis(query);
    if let medulla_isolation::isolation::AccessBasis::Elevation(elevation_id) = &basis {
        tracing::info!(
            target: "medulla::rbac::elevation::audit",
            elevation_id = %elevation_id,
            actor = query.agent_id,
            graph = query.graph_name,
            access = ?query.access,
            "graph access allowed by rbac elevation"
        );
    }
    basis.is_allowed()
}

/// `CheckAccess` (EH-416): would `agent_id`'s own request of `access` on this
/// graph be admitted now? The same decision [`check_graph_access`] applies to
/// that principal's requests, as a boolean; an unregistered or revoked
/// principal (or an unprovisioned policy) is `false`.
pub fn principal_may_access(
    isolation: &IsolationLayer,
    agent_id: &str,
    graph_name: &str,
    graph_type: medulla_protocol_types::protocol::GraphType,
    graph_owner: Option<&str>,
    access: medulla_types::acl::AccessCheck,
) -> bool {
    let level = match access {
        medulla_types::acl::AccessCheck::Read => AccessLevel::Read,
        medulla_types::acl::AccessCheck::Write => AccessLevel::Write,
    };
    !agent_id.is_empty()
        && isolation.has_rules()
        && isolation.check_access(agent_id, graph_name, graph_type, graph_owner, level)
}

/// Deny an unregistered/unauthenticated caller (or an unprovisioned isolation
/// policy) BEFORE the target graph's existence is resolved.
///
/// [`IsolationLayer::check_access`] denies an unregistered agent unconditionally —
/// `self.agents.get(agent_id)` is the very first thing it checks, before it ever
/// looks at the graph's type/owner — so this mirrors just that identity-only slice
/// of the decision. Calling it ahead of the graph-existence lookup means a caller
/// who could never pass ACL for ANY graph is denied with the SAME `ACCESS_DENIED`
/// message they would eventually get anyway, instead of first learning whether the
/// target graph exists via a distinguishable "Graph '...' not found" response — an
/// unregistered caller must not be able to probe graph names this way. A caller who
/// IS registered always falls through unchanged to the existing
/// existence-then-`check_graph_access` sequence, which still makes the real,
/// graph-type/owner-aware decision.
pub fn check_caller_is_known(
    isolation: &IsolationLayer,
    caller: Option<&str>,
    graph_name: &str,
    access: AccessLevel,
) -> Result<(), String> {
    let agent = require_verified_caller(caller)?;
    require_provisioned_policy(isolation)?;
    if isolation.is_registered(agent) {
        return Ok(());
    }
    medulla_kernel::metrics::access_denied();
    Err(format!(
        "ACCESS_DENIED: verified principal lacks {access:?} access to graph '{graph_name}'"
    ))
}

/// Resolve the authenticated ACL actor. Transport objects may exist before
/// authentication, but an absent or empty identity must never reach ACL, quota,
/// admission, or durable state as a synthetic bucket.
fn require_verified_caller(caller: Option<&str>) -> Result<&str, String> {
    caller
        .filter(|agent| !agent.trim().is_empty())
        .ok_or_else(|| {
            medulla_kernel::metrics::access_denied();
            "ACCESS_DENIED: verified caller identity is required".to_string()
        })
}

fn require_provisioned_policy(isolation: &IsolationLayer) -> Result<(), String> {
    if isolation.has_rules() {
        return Ok(());
    }
    medulla_kernel::metrics::access_denied();
    Err("ACCESS_DENIED: a provisioned identity/RBAC policy is required".to_string())
}

/// L10 (MEDULLA-P0-6 security finding): every broker/stream mutating op that
/// `wal.rs::is_durable_mutation` already durable-logs must ALSO be classified a
/// write by `requires_write`, so a Read-only caller can never invoke it. Each case
/// below previously FAILED this assertion (classified `Read`, letting a Read-access
/// caller mutate the control graph); this test locks in the fix.
#[cfg(feature = "broker")]
#[cfg(test)]
mod l10_broker_stream_write_tests {
    use super::*;
    use medulla_protocol_types::protocol::Method;

    #[test]
    fn stream_declare_requires_write() {
        assert!(requires_write(&Method::StreamDeclare {
            stream: "s1".into(),
            max_messages: None,
            max_age_ms: None,
        }));
    }

    #[test]
    fn stream_publish_requires_write() {
        assert!(requires_write(&Method::StreamPublish {
            stream: "s1".into(),
            payload: vec![1, 2, 3],
            now_ms: 0,
        }));
    }

    #[test]
    fn stream_trim_requires_write() {
        assert!(requires_write(&Method::StreamTrim {
            stream: "s1".into(),
            now_ms: 0,
        }));
    }

    #[test]
    fn stream_commit_offset_requires_write() {
        assert!(requires_write(&Method::StreamCommitOffset {
            stream: "s1".into(),
            group: "g1".into(),
            offset: 0,
        }));
    }

    #[test]
    fn publish_confirmed_requires_write() {
        assert!(requires_write(&Method::PublishConfirmed {
            exchange: "ex".into(),
            routing_key: "rk".into(),
            payload: vec![],
            priority: 0,
            delay_ms: None,
            ttl_ms: None,
            now_ms: None,
        }));
    }

    #[test]
    fn publish_idempotent_requires_write() {
        assert!(requires_write(&Method::PublishIdempotent {
            exchange: "ex".into(),
            routing_key: "rk".into(),
            payload: vec![],
            producer_id: None,
            seq: 0,
            priority: 0,
            delay_ms: None,
            ttl_ms: None,
            now_ms: None,
        }));
    }

    #[test]
    fn broker_ack_tag_requires_write() {
        assert!(requires_write(&Method::BrokerAckTag {
            delivery_tag: 1,
            consumer: "c1".into(),
        }));
    }

    #[test]
    fn broker_nack_tag_requires_write() {
        assert!(requires_write(&Method::BrokerNackTag {
            delivery_tag: 1,
            consumer: "c1".into(),
            requeue: true,
            now_ms: 0,
        }));
    }

    #[test]
    fn broker_renew_tag_requires_write() {
        assert!(requires_write(&Method::BrokerRenewTag {
            delivery_tag: 1,
            consumer: "c1".into(),
            now_ms: 0,
            lease_ms: 1,
        }));
    }

    /// Cross-check with the durability classifier: every one of these 9 ops is
    /// ALSO durable, so the ACL-write and WAL-durable classifications agree exactly
    /// (mirrors `durability_closure_tests::assert_write_implies_durable` below).
    #[test]
    fn all_nine_are_also_durable() {
        use medulla_server_api::mutation_apply::is_durable_mutation;
        let methods = vec![
            Method::StreamDeclare {
                stream: "s1".into(),
                max_messages: None,
                max_age_ms: None,
            },
            Method::StreamPublish {
                stream: "s1".into(),
                payload: vec![],
                now_ms: 0,
            },
            Method::StreamTrim {
                stream: "s1".into(),
                now_ms: 0,
            },
            Method::StreamCommitOffset {
                stream: "s1".into(),
                group: "g1".into(),
                offset: 0,
            },
            Method::PublishConfirmed {
                exchange: "ex".into(),
                routing_key: "rk".into(),
                payload: vec![],
                priority: 0,
                delay_ms: None,
                ttl_ms: None,
                now_ms: None,
            },
            Method::PublishIdempotent {
                exchange: "ex".into(),
                routing_key: "rk".into(),
                payload: vec![],
                producer_id: None,
                seq: 0,
                priority: 0,
                delay_ms: None,
                ttl_ms: None,
                now_ms: None,
            },
            Method::BrokerAckTag {
                delivery_tag: 1,
                consumer: "c1".into(),
            },
            Method::BrokerNackTag {
                delivery_tag: 1,
                consumer: "c1".into(),
                requeue: true,
                now_ms: 0,
            },
            Method::BrokerRenewTag {
                delivery_tag: 1,
                consumer: "c1".into(),
                now_ms: 0,
                lease_ms: 1,
            },
        ];
        for m in &methods {
            assert!(requires_write(m), "{m:?} must require write");
            assert!(is_durable_mutation(m), "{m:?} must be durable");
        }
    }
}

/// Admin-scope enforcement (CONCEPT:MEDULLA-KG.compute.feature, MEDULLA-P0-6): is `action` (an
/// `medulla_capabilities::MethodPolicy::authz_action` string) one of the system-wide
/// administrative actions that requires the caller to hold admin capability, rather
/// than the ordinary per-graph Read/Write ACL? Driven ENTIRELY off the ledger's
/// `authz_action` string — never a parallel hand-maintained method-name list — so a
/// future `Method` variant that declares one of these action strings in
/// `medulla_capabilities::policy` is gated automatically, with no dispatch.rs edit.
///
/// Today this covers exactly the methods the MEDULLA-P0-1 divergence report flagged as
/// the "Zero-Trust Consensus" + M3 cluster-admin + DR family: `RegisterIdentity` /
/// `RbacAdmin` / `ApplyMultisigMutation` (`"security:admin"`), `Reshard` /
/// `CatalogAssign` / `CatalogReassign` / `CatalogRemove` (`"admin:cluster"`),
/// `RebalanceExecute` (`"admin:cluster"`), `CatalogList` / `RebalancePlan`
/// (`"admin:cluster-read"`), and `Backup` / `Restore` (`"admin:backup"`) — the exact
/// set `src/server/dispatch.rs`'s M3-admin arm + Zero-Trust-Consensus arms route.
pub fn is_admin_authz_action(action: &str) -> bool {
    action == "security:admin" || action.starts_with("admin:")
}

/// Enforce admin-scope for a method whose `authz_action` [`is_admin_authz_action`].
///
/// A provisioned identity/RBAC policy is mandatory. The caller must hold admin capability
/// ([`IsolationLayer::has_admin_capability`] — `System` role, or an explicit RBAC
/// `Admin` grant) — there is no coarse-ACL fallback for admin actions the way graph
/// Read/Write has one, so an agent with no admin grant is DENIED, not defaulted
/// open.
pub fn require_admin_capability(
    isolation: &IsolationLayer,
    caller: Option<&str>,
    action: &'static str,
) -> Result<(), String> {
    require_admin_capability_with_policy(isolation, caller, action)
}

fn require_admin_capability_with_policy(
    isolation: &IsolationLayer,
    caller: Option<&str>,
    action: &'static str,
) -> Result<(), String> {
    let agent = require_verified_caller(caller)?;
    if !isolation.has_rules() {
        medulla_kernel::metrics::access_denied();
        return Err(format!(
            "ACCESS_DENIED: a provisioned identity/RBAC policy is required for '{action}'"
        ));
    }
    if isolation.has_admin_capability(agent) {
        Ok(())
    } else {
        medulla_kernel::metrics::access_denied();
        Err(format!(
            "ACCESS_DENIED: verified principal lacks admin capability required for '{action}'"
        ))
    }
}

#[cfg(test)]
mod secure_empty_policy_tests {
    use super::*;
    use medulla_protocol_types::protocol::GraphType;

    #[cfg(feature = "security")]
    #[test]
    fn served_graph_access_refuses_a_system_identity_demoted_by_another_clone() {
        use medulla_types::acl::AgentIdentity;
        use medulla_types::acl::AgentRole;
        let mut writer = IsolationLayer::new();
        let mut identity = AgentIdentity {
            agent_id: "revoked-system".into(),
            role: AgentRole::System,
            teams: vec![],
            roles: vec![],
        };
        writer.try_register_agent(identity.clone()).unwrap();
        let stale = writer.clone();
        let check = || {
            check_graph_access_with_policy(
                &stale,
                Some("revoked-system"),
                "graph",
                GraphType::Agent,
                None,
                AccessLevel::Read,
            )
        };
        assert!(check().is_ok());
        assert!(require_admin_capability_with_policy(
            &stale,
            Some("revoked-system"),
            "security:admin"
        )
        .is_ok());
        identity.role = AgentRole::Agent;
        writer.try_register_agent(identity).unwrap();
        assert!(check().unwrap_err().contains("ACCESS_DENIED"));
        assert!(require_admin_capability_with_policy(
            &stale,
            Some("revoked-system"),
            "security:admin"
        )
        .unwrap_err()
        .contains("ACCESS_DENIED"));
    }

    #[test]
    fn secure_graph_access_fails_closed_when_identity_store_is_empty() {
        let isolation = IsolationLayer::new();
        let result = check_graph_access_with_policy(
            &isolation,
            Some("agent:a"),
            "agent:a",
            GraphType::Agent,
            Some("agent:a"),
            AccessLevel::Read,
        );
        assert!(result
            .unwrap_err()
            .contains("provisioned identity/RBAC policy"));
    }

    #[test]
    fn secure_admin_access_fails_closed_when_identity_store_is_empty() {
        let isolation = IsolationLayer::new();
        let result =
            require_admin_capability_with_policy(&isolation, Some("agent:a"), "security:admin");
        assert!(result
            .unwrap_err()
            .contains("provisioned identity/RBAC policy"));
    }

    #[test]
    fn graph_access_rejects_absent_or_empty_verified_identity() {
        let isolation = IsolationLayer::new();
        for caller in [None, Some(""), Some("   ")] {
            let error = check_graph_access_with_policy(
                &isolation,
                caller,
                "__commons__",
                GraphType::Commons,
                None,
                AccessLevel::Read,
            )
            .unwrap_err();
            assert_eq!(error, "ACCESS_DENIED: verified caller identity is required");
        }
    }

    #[test]
    fn admin_access_rejects_absent_or_empty_verified_identity() {
        let isolation = IsolationLayer::new();
        for caller in [None, Some(""), Some("   ")] {
            let error = require_admin_capability_with_policy(&isolation, caller, "security:admin")
                .unwrap_err();
            assert_eq!(error, "ACCESS_DENIED: verified caller identity is required");
        }
    }
}

/// MEDULLA-P0-3 (WAL durability closure): `requires_write` (this file) classifies a
/// method as a mutation for the isolation-ACL check; `crate::mutation_apply::is_durable_mutation`
/// classifies it as needing a WAL record so it survives a crash. The two MUST agree
/// for every method that can actually mutate durable data — a method acknowledged as
/// a write but never logged is silently lost on crash. This lives here (not in
/// `tests/`) because `requires_write` is `pub(crate)`: an external integration-test
/// crate cannot name it, so the invariant can only be asserted from inside the crate.
///
/// Covers the 5 methods audited under MEDULLA-P0-3 (all previously FAILED this
/// assertion): `MineSequence`, `MineForecast`, the `MineText` non-tfidf variant, the
/// `MineSubgraph` gspan variant, and `AddEmbedding`.
#[cfg(test)]
mod durability_closure_tests {
    use super::*;
    use medulla_server_api::mutation_apply::is_durable_mutation;
    use medulla_protocol_types::protocol::Method;

    /// The one assertion every case below exercises: whenever `requires_write`
    /// says a method mutates (and therefore requires Write ACL + a write lock),
    /// `is_durable_mutation` must ALSO say it belongs in the WAL. (The converse
    /// need not hold — e.g. a durable method reached only via a self-routing
    /// surface like KV is fine — so this is a one-directional implication, not
    /// an equality.)
    fn assert_write_implies_durable(m: &Method) {
        assert!(
            !requires_write(m) || is_durable_mutation(m),
            "MEDULLA-P0-3: {m:?} is classified a write by `access::requires_write` but \
             NOT durable by `mutation_apply::is_durable_mutation` — it would be acknowledged \
             then silently lost on crash"
        );
    }

    #[cfg(feature = "mining")]
    #[test]
    fn mine_sequence_writeback_is_durable() {
        use medulla_protocol_types::protocol::MineSeqAlgorithm;
        let m = Method::MineSequence {
            sequences: vec![vec!["a".into(), "b".into()]],
            source: None,
            min_support: 0.5,
            algorithm: MineSeqAlgorithm::Prefixspan,
            writeback: true,
            #[cfg(feature = "epistemic")]
            as_claim: false,
        };
        assert!(
            requires_write(&m),
            "writeback=true must classify as a write"
        );
        assert_write_implies_durable(&m);
    }

    #[cfg(feature = "mining")]
    #[test]
    fn mine_forecast_writeback_is_durable() {
        use medulla_protocol_types::protocol::ForecastAlgorithm;
        let m = Method::MineForecast {
            values: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            algorithm: ForecastAlgorithm::Arima,
            horizon: 2,
            p: 1,
            d: 0,
            q: 0,
            period: 0,
            alpha: 0.3,
            beta: 0.1,
            gamma: 0.1,
            confidence: 0.95,
            series_id: "s1".into(),
            writeback: true,
            #[cfg(feature = "epistemic")]
            as_claim: false,
        };
        assert!(
            requires_write(&m),
            "writeback=true must classify as a write"
        );
        assert_write_implies_durable(&m);
    }

    /// `MineText` writeback only mutates for `lda`/`nmf` — the durable set must
    /// mirror that EXACT condition, not just "writeback=true" (a `tfidf` write
    /// would otherwise be logged for an op that never touches the graph).
    #[cfg(feature = "mining")]
    #[test]
    fn mine_text_lda_writeback_is_durable_but_tfidf_is_not() {
        use medulla_protocol_types::protocol::TextAlgorithm;
        let base = |algorithm: TextAlgorithm| Method::MineText {
            docs: vec![vec!["a".into(), "b".into()]],
            source: None,
            algorithm,
            k: 2,
            alpha: 0.1,
            beta: 0.01,
            iterations: 10,
            seed: 1,
            top_n: 5,
            writeback: true,
            #[cfg(feature = "epistemic")]
            as_claim: false,
        };
        let lda = base(TextAlgorithm::Lda);
        assert!(
            requires_write(&lda),
            "lda writeback=true must classify as a write"
        );
        assert_write_implies_durable(&lda);

        // tfidf never mutates regardless of `writeback` — both classifiers must
        // agree it is NOT a write (never durable, never write-locked).
        let tfidf = base(TextAlgorithm::Tfidf);
        assert!(
            !requires_write(&tfidf),
            "tfidf must never classify as a write, even with writeback=true"
        );
        assert!(
            !is_durable_mutation(&tfidf),
            "tfidf must never be durable, even with writeback=true"
        );
    }

    /// `MineSubgraph` writeback only mutates for `gspan` — mirrors the `MineText`
    /// exact-condition contract above (`motif` is a pure census, never a write).
    #[cfg(feature = "mining")]
    #[test]
    fn mine_subgraph_gspan_writeback_is_durable_but_motif_is_not() {
        use medulla_protocol_types::protocol::SubgraphAlgorithm;
        let base = |algorithm: SubgraphAlgorithm| Method::MineSubgraph {
            label: None,
            min_support: 0.1,
            max_edges: 2,
            algorithm,
            writeback: true,
            #[cfg(feature = "epistemic")]
            as_claim: false,
        };
        let gspan = base(SubgraphAlgorithm::Gspan);
        assert!(
            requires_write(&gspan),
            "gspan writeback=true must classify as a write"
        );
        assert_write_implies_durable(&gspan);

        let motif = base(SubgraphAlgorithm::Motif);
        assert!(
            !requires_write(&motif),
            "motif must never classify as a write, even with writeback=true"
        );
        assert!(
            !is_durable_mutation(&motif),
            "motif must never be durable, even with writeback=true"
        );
    }

    #[test]
    fn add_embedding_is_durable() {
        let m = Method::AddEmbedding {
            node_id: "n1".into(),
            embedding: vec![0.1, 0.2, 0.3],
        };
        assert!(requires_write(&m));
        assert_write_implies_durable(&m);
    }
}

#[cfg(all(test, feature = "security"))]
mod universal_row_read_tests {
    use super::*;
    use medulla_types::acl::AgentIdentity;
    use medulla_types::acl::AgentRole;

    fn properties(values: &[(&str, &str)]) -> Vec<u8> {
        let map: std::collections::BTreeMap<String, String> = values
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        rmp_serde::to_vec_named(&map).unwrap()
    }

    /// BUG-193: the "public" row below is tagged `_owner_id` (the BUG-052/GOC-61
    /// canonical convention), NOT a bare `_visibility: "public"` with no owner —
    /// that shape is exactly the 21,064-row BUG-064 incident population, and
    /// `row_visibility`'s BUG-192 middle branch correctly denies it (see
    /// `isolation::tests::rls::default_deny::unowned_bare_visibility_tag_alone_is_no_longer_trusted`).
    /// A real production write through the mutation gateway now stamps
    /// `_owner_id` from the caller's identity when absent
    /// (`stamp_owner_id_if_absent`, wired in `medulla_h_graph::graph_ops::
    /// try_handle_gateway`); this fixture models that shape rather than the
    /// unstamped one no production write path can produce anymore. With no
    /// `_visibility`/`_shared_scope` key set, the row keeps the pre-existing
    /// bare-absent-default (visible beyond its owner), so it stays visible to
    /// both `alice` and `bob` exactly as this fixture's tests require.
    fn shared_graph() -> (Arc<GraphCore>, IsolationLayer) {
        let core = Arc::new(GraphCore::new());
        core.add_node(
            "alice-private".to_string(),
            properties(&[
                ("_owner", "alice"),
                ("_visibility", "private"),
                ("type", "Thing"),
            ]),
        );
        core.add_node(
            "bob-private".to_string(),
            properties(&[
                ("_owner", "bob"),
                ("_visibility", "private"),
                ("type", "Thing"),
            ]),
        );
        core.add_node(
            "public".to_string(),
            properties(&[("_owner_id", "system-writer"), ("type", "Thing")]),
        );
        // A topology row with no valid property metadata is hidden in strict mode.
        core.add_node("untagged".to_string(), Vec::new());
        core.add_edge("alice-private".into(), "public".into(), Vec::new())
            .unwrap();
        core.add_edge("bob-private".into(), "public".into(), Vec::new())
            .unwrap();
        core.add_edge("public".into(), "untagged".into(), Vec::new())
            .unwrap();
        {
            let mut semantic = core.semantic_store.write();
            semantic
                .add_embedding("alice-private".into(), vec![1.0, 0.0])
                .unwrap();
            semantic
                .add_embedding("bob-private".into(), vec![0.0, 1.0])
                .unwrap();
            semantic
                .add_embedding("public".into(), vec![0.7, 0.7])
                .unwrap();
            semantic
                .add_embedding("untagged".into(), vec![-1.0, 0.0])
                .unwrap();
        }

        let isolation = super::two_agent_isolation();
        (core, isolation)
    }

    /// The equivalence oracle for the `HasNode`/`HasNodesBatch` point-lookup fast
    /// path (`GraphReadAuthority::node_visible`).
    ///
    /// That fast path exists so a boolean membership question stops paying for a
    /// whole `O(V log V + E log E + V*d)` projection. The ONLY thing that makes it
    /// safe is that it must answer identically to the projection it skips — this
    /// crate enforces row-level isolation, so a fast path that disagrees is a
    /// data-leak bug, not a performance bug.
    ///
    /// Asserting equivalence (rather than hardcoding expected booleans) is
    /// deliberate: it pins the fast path TO the bulk path, so a future change to
    /// RLS policy that moves one also has to move the other or fail here. The
    /// fixture deliberately spans every visibility class the predicate
    /// distinguishes — another actor's private row, one's own private row, a
    /// public row, a topology row with NO property blob (`untagged`, which
    /// `filter_view` judges as `default_public` rather than implicitly visible),
    /// and an id that is absent entirely.
    #[test]
    fn has_node_fast_path_answers_exactly_as_the_projection_it_skips() {
        let (core, isolation) = shared_graph();
        for agent_id in ["alice", "bob"] {
            let context = medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test(agent_id);
            let authority = GraphReadAuthority::from_verified(&context, &isolation).unwrap();
            let projected = authority.project_core(&core);
            for node_id in [
                "alice-private",
                "bob-private",
                "public",
                "untagged",
                "does-not-exist",
            ] {
                assert_eq!(
                    authority.node_visible(&core, node_id),
                    projected.has_node(node_id),
                    "point-lookup fast path disagreed with the projection for \
                     actor={agent_id} node={node_id}: node_visible() is the \
                     short-circuit `HasNode`/`HasNodesBatch` take instead of \
                     building a filtered core, so any disagreement is an RLS \
                     divergence between the two read paths"
                );
            }
        }
    }

    #[test]
    fn alice_and_bob_same_tenant_shared_graph_cannot_observe_each_others_rows() {
        let (core, isolation) = shared_graph();
        let alice_context = medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test("alice");
        let bob_context = medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test("bob");
        let alice = GraphReadAuthority::from_verified(&alice_context, &isolation).unwrap();
        let bob = GraphReadAuthority::from_verified(&bob_context, &isolation).unwrap();
        assert_eq!(alice.verified_actor().unwrap(), "alice");
        assert_eq!(bob.verified_actor().unwrap(), "bob");
        let alice_core = alice.project_core(&core);
        let bob_core = bob.project_core(&core);

        // Existence, point, batch, ids, and aggregate-count side channels.
        assert!(alice_core.has_node("alice-private"));
        assert!(!alice_core.has_node("bob-private"));
        assert!(!alice_core.has_node("untagged"));
        assert_eq!(alice_core.node_count(), 2);
        assert_eq!(bob_core.node_count(), 2);
        assert!(alice_core.get_node_properties("bob-private").is_none());
        let batch = ["alice-private", "bob-private", "public"]
            .map(|id| alice_core.get_node_properties(id).is_some());
        assert_eq!(batch, [true, false, true]);
        let alice_ids: std::collections::BTreeSet<_> = alice_core.node_ids().into_iter().collect();
        assert_eq!(
            alice_ids,
            ["alice-private".to_string(), "public".to_string()]
                .into_iter()
                .collect()
        );

        // Edge existence, degree, neighborhoods, topology, and path computation
        // all run on the projection, not on a post-filtered response.
        assert_eq!(alice_core.edge_count(), 1);
        assert!(alice_core.has_edge("alice-private", "public"));
        assert!(!alice_core.has_edge("bob-private", "public"));
        assert_eq!(alice_core.in_degree("public").unwrap(), 1);
        let neighbors: std::collections::BTreeSet<_> = alice_core
            .get_neighbors("public")
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            neighbors,
            ["alice-private".to_string()].into_iter().collect()
        );
        let topology = alice_core.topology_snapshot();
        assert_eq!(
            medulla_graph_algos::algorithms::get_shortest_path(
                &topology,
                "alice-private",
                "public"
            ),
            Some(vec!["alice-private".to_string(), "public".to_string()])
        );
        assert!(medulla_graph_algos::algorithms::get_shortest_path(
            &topology,
            "bob-private",
            "public"
        )
        .is_none());

        // Semantic candidates are rebuilt from visible embeddings only; querying
        // exactly for Bob's hidden vector still cannot return Bob's id to Alice.
        let semantic = alice_core
            .semantic_store
            .read()
            .semantic_search(&[0.0, 1.0], 10);
        assert!(!semantic.is_empty());
        assert!(semantic.iter().all(|(id, _)| id != "bob-private"));
        assert!(semantic
            .iter()
            .all(|(id, _)| alice_ids.contains(id.as_str())));

        assert!(bob_core.has_node("bob-private"));
        assert!(!bob_core.has_node("alice-private"));
    }

    /// D-OP-1 / D-OB-20: `project_core` now caches its (expensive) materialization
    /// per (actor, `GraphCore::version()`). This is the direct proof that caching
    /// does not turn `project_core` stale: a mutation between two calls MUST be
    /// visible on the second call, not silently served from a version-0 cache entry.
    /// The version check is what makes this a correctness-preserving amortization
    /// rather than a relaxation of `project_core`'s guarantee (see that method's doc).
    #[test]
    fn project_core_reflects_a_mutation_after_the_previous_projection_was_cached() {
        let (core, isolation) = shared_graph();
        let alice_context = medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test("alice");
        let alice = GraphReadAuthority::from_verified(&alice_context, &isolation).unwrap();

        // First call: populates the cache at the graph's current version. Alice's
        // "public" node is visible; a brand-new node does not exist yet.
        let first = alice.project_core(&core);
        assert!(first.has_node("public"));
        assert!(!first.has_node("alice-new-public-node"));

        // Mutate the SOURCE graph, then call again with the SAME actor. The raw
        // `GraphCore::add_node` primitive does NOT itself bump `version()` — in the
        // real server, `mark_dirty()` is called separately by the mutation commit
        // path (`src/server/mutation.rs::commit_finalize`) right after a successful
        // write, which is what actually advances the OCC version every committed
        // write goes through. Call it explicitly here to reproduce a real committed
        // write, not just a raw topology poke. A stale cache would still show the
        // old snapshot; the fix must miss on the version mismatch and rebuild.
        core.add_node(
            "alice-new-public-node".to_string(),
            properties(&[("_owner_id", "system-writer"), ("type", "Thing")]),
        );
        core.mark_dirty();
        let second = alice.project_core(&core);
        assert!(
            second.has_node("alice-new-public-node"),
            "a committed write between two project_core calls for the SAME actor \
             must be visible on the next call — the cache must invalidate on the \
             graph's version advancing, not serve a stale (actor, old-version) entry"
        );

        // And a THIRD call at the now-stable (post-mutation) version must hit the
        // freshly rebuilt cache entry rather than rebuilding yet again — same
        // observable content as `second`, proving the cache re-populated correctly
        // after the miss (not just that the miss itself produced a correct one-off).
        let third = alice.project_core(&core);
        assert!(third.has_node("alice-new-public-node"));
        assert_eq!(third.node_count(), second.node_count());
    }

    /// D-OP-1 / D-OB-20 benchmark harness: a READ MIX shaped like what a
    /// production grounding delegation actually issues — many sequential
    /// point-ish reads (`HasNode` + a `GetNodeProperties`-equivalent) by the SAME
    /// actor against a STABLE graph version (no writes in the burst, mirroring a
    /// read-only grounding phase), not one isolated call. This is what turns a
    /// per-call floor into the reported "grounding alone was 90.14s against a
    /// 10.0s production budget" — N reads each paying the full O(V) rebuild.
    /// Prints wall-clock numbers (`cargo test -- --nocapture`) so a before/after
    /// comparison does not depend on the pass/fail boundary alone.
    #[test]
    fn project_core_read_burst_mirrors_grounding_read_mix() {
        const NODE_COUNT: usize = 20_000;
        const EMBEDDING_DIM: usize = 1024;
        const BURST_READS: usize = 25;

        let core = Arc::new(GraphCore::new());
        {
            let mut semantic = core.semantic_store.write();
            for i in 0..NODE_COUNT {
                let id = format!("n{i}");
                core.add_node(
                    id.clone(),
                    properties(&[("_owner_id", "system-writer"), ("type", "Thing")]),
                );
                semantic
                    .add_embedding(id, vec![(i % 7) as f32 * 0.01; EMBEDDING_DIM])
                    .unwrap();
            }
        }

        let mut isolation = IsolationLayer::new();
        isolation.register_agent(AgentIdentity {
            agent_id: "grounding-service".to_string(),
            role: AgentRole::Agent,
            teams: Vec::new(),
            roles: Vec::new(),
        });
        let context =
            medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test("grounding-service");
        let authority = GraphReadAuthority::from_verified(&context, &isolation).unwrap();

        // Warm-up call: excludes one-time allocator/page-fault effects, matching a
        // live server's Nth request for this actor, not its 1st.
        let _ = authority.project_core(&core);

        let mut per_call = Vec::with_capacity(BURST_READS);
        let burst_started = std::time::Instant::now();
        for i in 0..BURST_READS {
            let call_started = std::time::Instant::now();
            let projected = authority.project_core(&core);
            let target = format!("n{}", i % NODE_COUNT);
            assert!(projected.has_node(&target));
            let _ = projected.get_node_properties(&target);
            per_call.push(call_started.elapsed());
        }
        let total = burst_started.elapsed();
        let avg = total / BURST_READS as u32;
        let max = per_call.iter().max().copied().unwrap_or_default();
        let min = per_call.iter().min().copied().unwrap_or_default();
        eprintln!(
            "D-OP-1 read-mix burst: reads={BURST_READS} total={total:?} avg={avg:?} \
             min={min:?} max={max:?} (node_count={NODE_COUNT}, embedding_dim={EMBEDDING_DIM})"
        );

        assert!(
            total < std::time::Duration::from_millis(500),
            "D-OP-1 read-mix burst of {BURST_READS} sequential reads by the SAME \
             actor at a STABLE graph version took {total:?} (avg {avg:?}/call, \
             max {max:?}) — budget 500ms total. This mirrors a grounding \
             delegation's read pattern; each call paying project_core's full \
             O(V) rebuild is exactly the reported 90.14s-against-10s-budget \
             production failure (D-OB-20)."
        );
    }

    #[test]
    fn carrier_ownership_separates_same_tenant_and_cross_tenant_callers() {
        let alice = CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
                "alice", "tenant-a",
            ),
        )
        .unwrap();
        let bob = CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
                "bob", "tenant-a",
            ),
        )
        .unwrap();
        let alice_other_tenant = CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
                "alice", "tenant-b",
            ),
        )
        .unwrap();

        assert_eq!(alice.tenant_scope(), bob.tenant_scope());
        assert_ne!(alice.actor_scope(), bob.actor_scope());
        assert_ne!(alice.owner_scope(), bob.owner_scope());
        assert_ne!(alice.tenant_scope(), alice_other_tenant.tenant_scope());
        assert_ne!(
            alice.namespace("kv-namespace", "shared"),
            bob.namespace("kv-namespace", "shared")
        );
        assert!(!bob.owns(alice.tenant_scope(), alice.actor_scope()));
        assert!(!alice_other_tenant.owns(alice.tenant_scope(), alice.actor_scope()));
    }

    /// D-IDENT-AUDIT-1: `commit_cross_modal_txn`'s RBAC recheck (and every other
    /// `check_graph_access` call site) is keyed by `CarrierAuthority::agent_id` --
    /// the SAME identity space `IsolationLayer::register_agent` registers under.
    /// `actor_scope` is a DIFFERENT identity space (an opaque durable-ownership
    /// hash used for KV/coordinator namespacing, ownership stamping, and
    /// MutationBatch provenance fingerprints) and is never a registered RBAC
    /// identity. Passing it into an RBAC check does not error -- it silently
    /// denies, because the hash never matches any grant.
    ///
    /// This is the EXACT confusion that produced three real bugs in one day:
    /// `cbcabdf` (`commit_graph_methods`/`commit_txn_state` in
    /// `src/server/wire/mod.rs` broke every wire-native graph write --
    /// pgwire/MySQL/MSSQL -- with ACCESS_DENIED regardless of RBAC grants) and
    /// `6417f31` (`commit_graphql_cross_modal` in
    /// `src/server/handlers/txn.rs` broke every GraphQL cross-modal commit the
    /// same way). This test locks in the fix at the shared `check_graph_access`
    /// boundary all three call sites route through, independent of any one
    /// surface's own (heavier) integration fixtures. Reverting either fix --
    /// passing `authority.actor_scope()` instead of `authority.agent_id()` as
    /// `commit_cross_modal_txn`'s `caller` -- reproduces exactly the failure
    /// this test's second assertion checks for.
    /// EH-416: `CheckAccess` answers the engine's own decision for a named
    /// principal -- a registered reader is allowed, a stranger is not, and
    /// re-registering the principal without the role (revocation) turns the
    /// answer to `false` at the next check.
    #[cfg(feature = "security")]
    #[test]
    fn principal_may_access_follows_registration_and_revocation() {
        use medulla_types::acl::{
            AccessCheck, Grant, GrantEffect, RbacAction, ResourceSelector, Role,
        };
        let reader = |roles: Vec<String>| AgentIdentity {
            agent_id: "alice".to_string(),
            role: AgentRole::Agent,
            teams: Vec::new(),
            roles,
        };
        let mut isolation = IsolationLayer::new();
        isolation.add_role(Role::new("finance-reader"));
        isolation.add_grant(Grant {
            role: "finance-reader".to_string(),
            resource: ResourceSelector::Graph("tenant-a".to_string()),
            action: RbacAction::Read,
            effect: GrantEffect::Allow,
        });
        isolation.register_agent(reader(vec!["finance-reader".to_string()]));
        let may = |isolation: &IsolationLayer, agent: &str, access: AccessCheck| {
            principal_may_access(
                isolation,
                agent,
                "tenant-a",
                medulla_protocol_types::protocol::GraphType::Agent,
                None,
                access,
            )
        };
        assert!(may(&isolation, "alice", AccessCheck::Read));
        assert!(
            !may(&isolation, "alice", AccessCheck::Write),
            "read grants no write"
        );
        assert!(
            !may(&isolation, "mallory", AccessCheck::Read),
            "unregistered is denied"
        );
        assert!(
            !may(&isolation, "", AccessCheck::Read),
            "an empty principal is denied"
        );
        isolation.register_agent(reader(Vec::new()));
        assert!(
            !may(&isolation, "alice", AccessCheck::Read),
            "revocation takes effect at the next check"
        );
    }

    #[test]
    fn rbac_check_is_keyed_by_agent_id_not_actor_scope() {
        let alice = CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
                "alice", "tenant-a",
            ),
        )
        .unwrap();
        // `agent_id()` and `actor_scope()` must actually differ for this test to
        // mean anything -- `verified_for_test_in_tenant`'s principal is
        // `principal:alice` (not already an opaque `principal:sha256:...`
        // digest), so `CarrierAuthority::from_verified` re-hashes it into an
        // opaque `principal:sha256:...` scope, distinct from the raw "alice"
        // agent_id.
        assert_ne!(alice.agent_id(), alice.actor_scope());

        let mut isolation = IsolationLayer::new();
        // `AgentRole::System` grants full access to any REGISTERED identity with
        // no RBAC roles/grants needed -- isolates this test to the one thing it's
        // proving (which string is the lookup key), not RBAC policy evaluation
        // (covered elsewhere, e.g. `l10_broker_stream_write_tests`).
        isolation.register_agent(AgentIdentity {
            agent_id: alice.agent_id().to_string(),
            role: AgentRole::System,
            teams: Vec::new(),
            roles: Vec::new(),
        });

        // The correct identity space: the registered agent_id resolves and is
        // allowed.
        assert!(check_graph_access(
            &isolation,
            Some(alice.agent_id()),
            "agent:alice",
            medulla_protocol_types::protocol::GraphType::Agent,
            Some("agent:alice"),
            AccessLevel::Write,
        )
        .is_ok());

        // The confused identity space: actor_scope() was never registered, so the
        // lookup misses and the check fails CLOSED -- exactly the silent
        // ACCESS_DENIED the three fixed bugs produced for every caller regardless
        // of their real RBAC grants.
        let denied = check_graph_access(
            &isolation,
            Some(alice.actor_scope()),
            "agent:alice",
            medulla_protocol_types::protocol::GraphType::Agent,
            Some("agent:alice"),
            AccessLevel::Write,
        );
        assert!(denied.is_err());
        assert!(denied.unwrap_err().contains("ACCESS_DENIED"));
    }

    /// A18: `unauthenticated_carrier_denied` used to be `{ true }` unconditionally
    /// — every caller, verified or not, was denied. It must now discriminate: a
    /// genuine `CarrierAuthority` allows, its absence denies. This is the direct,
    /// minimal proof of the fixed invariant; the per-surface integration proofs
    /// (S3 SigV4, KV-cache bearer/JWT) live alongside each surface's own tests.
    #[test]
    fn a18_carrier_authority_present_allows_absent_denies() {
        let carrier = CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test("alice"),
        )
        .unwrap();
        assert!(
            !unauthenticated_carrier_denied(Some(&carrier)),
            "a real, verified carrier must be ALLOWED, not denied"
        );
        assert!(
            unauthenticated_carrier_denied(None),
            "no carrier at all must still be denied (fail-closed)"
        );
    }

    /// D-OP-1 (orchestrator lane, 2026-07-31): `project_core` materialises an
    /// entire second graph on EVERY call when RLS is active — sort every node id,
    /// clone every node's properties, sort every edge key, clone every edge's
    /// properties, and copy every visible embedding out of the semantic store.
    /// That is `O(V log V + E log E + V*d)` PER READ, independent of what was
    /// asked for, with no cache/memo/`OnceCell`/lazy path anywhere in this file.
    ///
    /// Live measurement (orchestrator, 2026-07-31, `__commons__`): 25,075 nodes,
    /// 1024-dim embeddings -> ~103 MB memcpy'd per call; `HasNode` (ONE hash
    /// lookup) measured 2.7-3.0s over dozens of live samples with ZERO under
    /// 1s, while `CypherQuery` (which keeps its snapshot-level filter instead of
    /// calling `project_core`) answered in <0.0001s on 31 samples.
    ///
    /// This reproduces that shape in-process at a smaller scale (large enough
    /// that the O(V) cost dominates any fixed per-call overhead, small enough to
    /// run in a normal `cargo test` pass) and asserts a budget for a single
    /// `has_node` point lookup THROUGH `project_core`, the exact shape
    /// `try_handle` pays on every terminal read handler
    /// (`src/server/handlers/graph_ops.rs`, `let core = read_authority
    /// .project_core(&core);` before the `match`).
    ///
    /// **Fixed**: `project_core` now caches the projected core per (actor,
    /// `GraphCore::version()`), invalidated when the version advances — see
    /// `docs/architecture/d-op-1-projection-cache.md` for the full design and
    /// `crate::rls_projection_cache` for the cache itself. This test's warm-up +
    /// measured-call shape (same actor, no write in between) is exactly a cache
    /// hit, so it now passes at microsecond cost. It is intentionally strict
    /// enough that an eager-rebuild regression cannot pass it by accident: the
    /// budget is generously above a real cached point-lookup's cost
    /// (microseconds) and generously below the O(V) rebuild's cost at this node
    /// count, so there is no ambiguous middle ground.
    #[test]
    fn project_core_of_a_single_has_node_call_must_not_rebuild_the_whole_graph() {
        const NODE_COUNT: usize = 20_000;
        const EMBEDDING_DIM: usize = 1024;
        const BUDGET: std::time::Duration = std::time::Duration::from_millis(50);

        let core = Arc::new(GraphCore::new());
        {
            let mut semantic = core.semantic_store.write();
            for i in 0..NODE_COUNT {
                let id = format!("n{i}");
                core.add_node(
                    id.clone(),
                    properties(&[("_owner_id", "system-writer"), ("type", "Thing")]),
                );
                // A real embedding-shaped vector, not a zero-cost stand-in — the
                // memcpy cost `project_core` pays is proportional to this, not to
                // a placeholder.
                semantic
                    .add_embedding(id, vec![(i % 7) as f32 * 0.01; EMBEDDING_DIM])
                    .unwrap();
            }
        }

        let mut isolation = IsolationLayer::new();
        isolation.register_agent(AgentIdentity {
            agent_id: "alice".to_string(),
            role: AgentRole::Agent,
            teams: Vec::new(),
            roles: Vec::new(),
        });
        let alice_context = medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test("alice");
        let alice = GraphReadAuthority::from_verified(&alice_context, &isolation).unwrap();

        // Warm-up call: excludes one-time allocator/page-fault effects so the
        // measured call is a fair repeat-call comparison, matching how a live
        // server serves this same actor's Nth request, not its 1st.
        let _ = alice.project_core(&core);

        let started = std::time::Instant::now();
        let projected = alice.project_core(&core);
        let elapsed = started.elapsed();
        assert!(
            projected.has_node("n0"),
            "sanity: the projection must be usable"
        );

        assert!(
            elapsed < BUDGET,
            "project_core() for a single point lookup took {elapsed:?} against a \
             {NODE_COUNT}-node / {EMBEDDING_DIM}-dim graph (budget {BUDGET:?}). \
             This is the D-OP-1 regression: every RLS-active read rebuilds the \
             entire visible graph (sort + clone every node/edge, copy every \
             embedding) instead of reusing a cached projection keyed on \
             (actor, graph version). Fix: cache in project_core() / \
             GraphReadAuthority, invalidated on GraphCore::version() advancing \
             (see docs/architecture/d-op-1-projection-cache.md)."
        );
    }

    /// D-OP-1 — the REMAINING structural cost after the (actor, version) memoization
    /// fix above: a cache MISS (the version advanced since the last projection for
    /// this actor — i.e. any write happened, even one unrelated node) still pays the
    /// full `O(V log V + E log E + V*d)` rebuild, because the fix is a memo on top of
    /// an eager full-materialization build, not an incremental/copy-on-write
    /// projection. This is NOT a regression to fix in this test — it is the
    /// documented remainder D-OP-1 is still open for (see `docs/architecture/
    /// d-op-1-projection-cache.md`, "remaining work"): closing it needs the
    /// projection itself to be maintained incrementally alongside writes (or lazily
    /// per-row rather than eagerly for the whole visible set), which is a genuine
    /// data-structure change to RLS-filtered read serving, not a caching layer, and
    /// needs a security review before landing (this crate enforces row-level
    /// isolation; an incremental-maintenance bug here is a data-leak bug). This test
    /// exists to keep a REAL, current number attached to that remaining gap instead
    /// of a stale one from a prior report.
    #[test]
    fn project_core_cache_miss_after_any_write_still_pays_the_full_rebuild_d_op_1() {
        const NODE_COUNT: usize = 20_000;
        const EMBEDDING_DIM: usize = 1024;

        let core = Arc::new(GraphCore::new());
        {
            let mut semantic = core.semantic_store.write();
            for i in 0..NODE_COUNT {
                let id = format!("n{i}");
                core.add_node(
                    id.clone(),
                    properties(&[("_owner_id", "system-writer"), ("type", "Thing")]),
                );
                semantic
                    .add_embedding(id, vec![(i % 7) as f32 * 0.01; EMBEDDING_DIM])
                    .unwrap();
            }
        }

        let mut isolation = IsolationLayer::new();
        isolation.register_agent(AgentIdentity {
            agent_id: "alice".to_string(),
            role: AgentRole::Agent,
            teams: Vec::new(),
            roles: Vec::new(),
        });
        let alice_context = medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test("alice");
        let alice = GraphReadAuthority::from_verified(&alice_context, &isolation).unwrap();

        // Warm the cache for `alice` at the CURRENT version (a genuine hit baseline).
        let _ = alice.project_core(&core);
        let hit_started = std::time::Instant::now();
        let _ = alice.project_core(&core);
        let hit_elapsed = hit_started.elapsed();

        // ONE unrelated write bumps `GraphCore::version()`, invalidating every cached
        // projection for every actor — this is the realistic shape of a live server
        // under any write traffic at all, not a contrived worst case.
        core.add_node(
            "unrelated-write".to_string(),
            properties(&[("_owner_id", "system-writer"), ("type", "Thing")]),
        );

        let miss_started = std::time::Instant::now();
        let projected = alice.project_core(&core);
        let miss_elapsed = miss_started.elapsed();
        assert!(
            projected.has_node("n0"),
            "sanity: the post-write projection must still be usable"
        );

        println!(
            "[D-OP-1] {NODE_COUNT}-node / {EMBEDDING_DIM}-dim graph: cache-HIT project_core() \
             = {hit_elapsed:?}; cache-MISS (after one unrelated write) project_core() = \
             {miss_elapsed:?} — {:.0}x slower than the hit path, still O(V) per miss.",
            miss_elapsed.as_secs_f64() / hit_elapsed.as_secs_f64().max(1e-9)
        );

        // Deliberately NOT a pass/fail budget assertion (unlike the hit-path test
        // above): there is no fix landed yet for this path, so a strict budget here
        // would just be a permanently-red or permanently-loosened gate. The point of
        // this test is the printed measurement, kept current on every run.
    }
}

mod read_classification;

#[cfg(test)]
use read_classification::{
    NON_ROW_SCOPED, NOT_YET_AUDITED, REASON_ASR_PURE_COMPUTE, REASON_EPISTEMIC_CAUSAL_PURE_COMPUTE,
    REASON_QUANTUM_PURE_COMPUTE, REASON_VIZ_CARRIER_SCOPED, RLS_ROUTED,
};
#[cfg(test)]
mod read_rls_coverage_tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    /// Every non-mutating (`mutates == false`) protocol method routes through
    /// the audited RLS primitive, or is a documented, named exception. `UNDOCUMENTED`
    /// (a method whose name matches none of `RLS_ROUTED`/`NON_ROW_SCOPED`/
    /// `NOT_YET_AUDITED`) is the only hard failure -- mirrors
    /// `medulla_server::mutation::tests::gateway_routed_set_matches_mutating_policy_surface`'s
    /// exhaustive-partition idiom on the read side.
    ///
    /// A handful of methods (e.g. `KnowledgeStream`, feature `knowledge-batch`)
    /// are themselves `#[cfg(feature = ...)]`-gated in their domain-owned
    /// policy declarations, so `all_read` legitimately varies by
    /// build. This test therefore checks every method ACTUALLY PRESENT in the
    /// CURRENT build's `all_read` against the three lists (which are a
    /// superset spanning every feature combination) rather than requiring
    /// every listed name to be present -- run with `--features full` (or at
    /// least `knowledge-batch`/`jobs`/`tensor`/…) for the widest single-run
    /// coverage; CI's per-feature test matrix covers the rest.
    #[test]
    fn every_read_method_routes_through_rls_or_is_a_documented_exception() {
        let all_read: BTreeSet<&'static str> = medulla_capabilities::method_policy_entries()
            .filter(|(_, p, _)| !p.mutates)
            .map(|(name, _, _)| name)
            .collect();

        let routed_set: BTreeSet<&'static str> = RLS_ROUTED.iter().copied().collect();
        assert_eq!(
            routed_set.len(),
            RLS_ROUTED.len(),
            "RLS_ROUTED contains a duplicate entry"
        );
        let non_row: HashMap<&'static str, &'static str> = NON_ROW_SCOPED.iter().copied().collect();
        assert_eq!(
            non_row.len(),
            NON_ROW_SCOPED.len(),
            "NON_ROW_SCOPED contains a duplicate method name"
        );
        let not_audited: HashMap<&'static str, &'static str> =
            NOT_YET_AUDITED.iter().copied().collect();
        assert_eq!(
            not_audited.len(),
            NOT_YET_AUDITED.len(),
            "NOT_YET_AUDITED contains a duplicate method name"
        );

        // The three lists must be pairwise disjoint -- a method double-listed
        // (e.g. both RLS_ROUTED and NON_ROW_SCOPED) is a self-contradiction
        // regardless of which methods the current build actually compiled in.
        for name in non_row.keys().chain(not_audited.keys()) {
            assert!(
                !routed_set.contains(name),
                "'{name}' is listed in both RLS_ROUTED and NON_ROW_SCOPED/NOT_YET_AUDITED"
            );
        }
        for name in non_row.keys() {
            assert!(
                !not_audited.contains_key(name),
                "'{name}' is listed in both NON_ROW_SCOPED and NOT_YET_AUDITED"
            );
        }

        // Every method ACTUALLY PRESENT in this build's non-mutating surface
        // must be classified -- the hard failure. A documented name that this
        // build happens not to compile in (a narrower `--features` selection
        // than `full`) is not checked here; it is still verified whenever a
        // build that includes it runs this test.
        let undocumented: Vec<&&'static str> = all_read
            .iter()
            .filter(|name| {
                !routed_set.contains(*name)
                    && !non_row.contains_key(*name)
                    && !not_audited.contains_key(*name)
            })
            .collect();
        assert!(
            undocumented.is_empty(),
            "UNDOCUMENTED read methods (silently unclassified, not allowed): {undocumented:?} -- \
             add each to RLS_ROUTED, NON_ROW_SCOPED, or NOT_YET_AUDITED in medulla_server::access"
        );

        let present_routed = all_read.intersection(&routed_set).count();
        let present_non_row = all_read.iter().filter(|n| non_row.contains_key(*n)).count();
        let present_not_audited = all_read
            .iter()
            .filter(|n| not_audited.contains_key(*n))
            .count();
        println!(
            "L-RLS-1 read coverage (this build): {present_routed} RLS-routed / \
             {present_non_row} non-row-scoped (justified) / {present_not_audited} not-yet-audited \
             / {} non-mutating methods total present ({} / {} / {} known across all builds)",
            all_read.len(),
            RLS_ROUTED.len(),
            NON_ROW_SCOPED.len(),
            NOT_YET_AUDITED.len(),
        );
    }

    /// The exact, task-named gap this gate exists to close: `timeseries.rs`'s 5
    /// read methods (the original 4 query primitives plus `TsListSeries`, the
    /// tenant/graph-scoped series enumeration retention needs to discover what
    /// to sweep) must be a DOCUMENTED, justified `NON_ROW_SCOPED` exception --
    /// never `RLS_ROUTED` (they never construct a `GraphView`) and never
    /// silently absent from every bucket.
    #[test]
    fn timeseries_read_methods_are_a_documented_non_row_scoped_exception() {
        let non_row: HashMap<&'static str, &'static str> = NON_ROW_SCOPED.iter().copied().collect();
        for method in [
            "TsRange",
            "TsAsofJoin",
            "TsWindow",
            "TsGapFill",
            "TsListSeries",
        ] {
            assert!(
                non_row.contains_key(method),
                "'{method}' must be a documented NON_ROW_SCOPED exception"
            );
            assert!(
                !RLS_ROUTED.contains(&method),
                "'{method}' must not also claim to be RLS_ROUTED"
            );
        }
    }

    /// Self-routed modality/compute handlers have no graph row target to
    /// project. Their negative control is therefore the verified carrier's
    /// owner boundary: a foreign tenant cannot mint or claim the same
    /// owner-scoped handle. Viz applies this boundary to its persistent
    /// dataset/result handles in `handlers::viz`; the three pure-compute
    /// handlers have no server-held handle at all, so there is no row or
    /// non-row resource for them to read across tenants.
    fn assert_self_routed_cross_tenant_control(method: &str, expected_reason: &'static str) {
        let owner = CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
                "owner", "tenant-a",
            ),
        )
        .unwrap();
        let foreign = CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
                "foreign", "tenant-b",
            ),
        )
        .unwrap();
        let owner_handle = owner.namespace("read-rls-control", method);
        let foreign_handle = foreign.namespace("read-rls-control", method);
        let non_row: HashMap<&'static str, &'static str> = NON_ROW_SCOPED.iter().copied().collect();

        assert_eq!(
            non_row.get(method).copied(),
            Some(expected_reason),
            "{method} must remain in the documented non-row exception bucket"
        );
        assert!(!RLS_ROUTED.contains(&method));
        assert_ne!(
            owner_handle, foreign_handle,
            "foreign tenants must not receive the same {method} owner handle"
        );
        assert!(
            !foreign.owns(owner.tenant_scope(), owner.actor_scope()),
            "foreign tenant must be denied ownership of the {method} scope"
        );
    }

    #[test]
    fn asr_is_non_row_scoped_and_foreign_tenant_control_is_denied() {
        assert_self_routed_cross_tenant_control("Asr", REASON_ASR_PURE_COMPUTE);
    }

    #[test]
    fn quantum_is_non_row_scoped_and_foreign_tenant_control_is_denied() {
        assert_self_routed_cross_tenant_control("Quantum", REASON_QUANTUM_PURE_COMPUTE);
    }

    #[test]
    fn viz_is_owner_scoped_non_row_and_foreign_tenant_control_is_denied() {
        let non_row: HashMap<&'static str, &'static str> = NON_ROW_SCOPED.iter().copied().collect();
        assert_eq!(
            non_row.get("Viz").copied(),
            Some(REASON_VIZ_CARRIER_SCOPED),
            "Viz must be documented as an owner-scoped non-row surface"
        );
        assert!(!RLS_ROUTED.contains(&"Viz"));

        let owner = CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
                "owner", "tenant-a",
            ),
        )
        .unwrap();
        let foreign = CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
                "foreign", "tenant-b",
            ),
        )
        .unwrap();
        assert!(
            !foreign.owns(owner.tenant_scope(), owner.actor_scope()),
            "foreign tenant must be denied the owner of Viz's persistent side-store"
        );
    }

    // ── L-RLS-1 follow-up: the 5 formerly-`NOT_YET_AUDITED` epistemic read methods ──
    //
    // One focused test per method (task-required granularity), each pinning its
    // resolved routing bucket AND asserting it is absent from the other two -- so a
    // future regression on any ONE of these 5 names fails at ITS OWN assertion, not
    // just the exhaustive partition test above. `ResolveConflict`/`ExplainEvidence`
    // were traced to a real `core.analysis_snapshot()` read in
    // `src/server/handlers/query.rs` (their handler arms now `rls.filter_view` it
    // before use, see those arms' doc comments) -- `RLS_ROUTED`. `CausalEstimate`/
    // `CausalCounterfactual`/`RankByProvenance` were traced to the SAME file's
    // `causal_estimate_wire`/`causal_counterfactual_wire`/`rank_by_provenance_wire`,
    // none of which reference `core`/`state`/`GraphView` at all -- `NON_ROW_SCOPED`.

    #[test]
    fn resolve_conflict_is_rls_routed() {
        assert!(
            RLS_ROUTED.contains(&"ResolveConflict"),
            "ResolveConflict's handler builds a BeliefGraph off core.analysis_snapshot() \
             (src/server/handlers/query.rs's Method::ResolveConflict arm) and must \
             rls.filter_view it before medulla_epistemic::BeliefGraph::from_graph_view sees it -- \
             RLS_ROUTED"
        );
        let non_row: HashMap<&'static str, &'static str> = NON_ROW_SCOPED.iter().copied().collect();
        let not_audited: HashMap<&'static str, &'static str> =
            NOT_YET_AUDITED.iter().copied().collect();
        assert!(!non_row.contains_key("ResolveConflict"));
        assert!(!not_audited.contains_key("ResolveConflict"));
    }

    #[test]
    fn explain_evidence_is_rls_routed() {
        assert!(
            RLS_ROUTED.contains(&"ExplainEvidence"),
            "ExplainEvidence's handler builds a BeliefGraph off core.analysis_snapshot() \
             (src/server/handlers/query.rs's Method::ExplainEvidence arms) and must \
             rls.filter_view it before medulla_epistemic::BeliefGraph::from_graph_view sees it -- \
             RLS_ROUTED"
        );
        let non_row: HashMap<&'static str, &'static str> = NON_ROW_SCOPED.iter().copied().collect();
        let not_audited: HashMap<&'static str, &'static str> =
            NOT_YET_AUDITED.iter().copied().collect();
        assert!(!non_row.contains_key("ExplainEvidence"));
        assert!(!not_audited.contains_key("ExplainEvidence"));
    }

    #[test]
    fn causal_estimate_is_a_documented_non_row_scoped_exception() {
        let non_row: HashMap<&'static str, &'static str> = NON_ROW_SCOPED.iter().copied().collect();
        assert_eq!(
            non_row.get("CausalEstimate").copied(),
            Some(REASON_EPISTEMIC_CAUSAL_PURE_COMPUTE),
            "CausalEstimate's handler (causal_estimate_wire) never reads core/state/GraphView -- \
             pure compute over the request's own variables/do_values/mode"
        );
        assert!(!RLS_ROUTED.contains(&"CausalEstimate"));
        let not_audited: HashMap<&'static str, &'static str> =
            NOT_YET_AUDITED.iter().copied().collect();
        assert!(!not_audited.contains_key("CausalEstimate"));
    }

    #[test]
    fn causal_counterfactual_is_a_documented_non_row_scoped_exception() {
        let non_row: HashMap<&'static str, &'static str> = NON_ROW_SCOPED.iter().copied().collect();
        assert_eq!(
            non_row.get("CausalCounterfactual").copied(),
            Some(REASON_EPISTEMIC_CAUSAL_PURE_COMPUTE),
            "CausalCounterfactual's handler (causal_counterfactual_wire) never reads \
             core/state/GraphView -- pure compute over the request's own \
             variables/actual/do_values"
        );
        assert!(!RLS_ROUTED.contains(&"CausalCounterfactual"));
        let not_audited: HashMap<&'static str, &'static str> =
            NOT_YET_AUDITED.iter().copied().collect();
        assert!(!not_audited.contains_key("CausalCounterfactual"));
    }

    #[test]
    fn rank_by_provenance_is_a_documented_non_row_scoped_exception() {
        let non_row: HashMap<&'static str, &'static str> = NON_ROW_SCOPED.iter().copied().collect();
        assert_eq!(
            non_row.get("RankByProvenance").copied(),
            Some(REASON_EPISTEMIC_CAUSAL_PURE_COMPUTE),
            "RankByProvenance's handler (rank_by_provenance_wire) never reads \
             core/state/GraphView -- pure compute over the request's own candidates/weights"
        );
        assert!(!RLS_ROUTED.contains(&"RankByProvenance"));
        let not_audited: HashMap<&'static str, &'static str> =
            NOT_YET_AUDITED.iter().copied().collect();
        assert!(!not_audited.contains_key("RankByProvenance"));
    }

    /// Cross-route parity: `sql_is_write` is the single chokepoint that
    /// `requires_write`, the read-only KnowledgeStream gate and the wire's own
    /// access check all resolve a `Method::Sql` statement through. A
    /// `GRAPH_TABLE` read must be a READ at every one of them; classifying it
    /// as a write denied a Read-only caller a pure read and made
    /// KnowledgeStream reject it outright.
    #[cfg(feature = "query")]
    #[test]
    fn graph_table_is_a_read_at_the_shared_write_classification_chokepoint() {
        let graph_table = "SELECT * FROM GRAPH_TABLE (shop MATCH (c:customer) COLUMNS (c.name))";
        assert!(matches!(
            medulla_sql_lang::sql::classify::classify(graph_table),
            Ok(medulla_sql_lang::sql::classify::StatementKind::GraphTableReadRequiresCatalogAdmission(_))
        ));
        assert!(!sql_is_write(graph_table));
        assert!(!requires_write(
            &medulla_protocol_types::protocol::Method::Sql {
                query: graph_table.to_string(),
                params_msgpack: Vec::new(),
            }
        ));

        // Property-graph DDL stays a write on every route.
        let ddl = "CREATE PROPERTY GRAPH shop VERTEX TABLES (customers KEY (customer_id))";
        assert!(sql_is_write(ddl));
    }
}
