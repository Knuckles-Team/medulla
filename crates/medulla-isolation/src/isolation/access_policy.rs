use super::*;
#[cfg(feature = "security")]
use std::collections::HashSet;

/// One graph-access question, as the chokepoint receives it. `now_ms` is the
/// clock an elevation's hard expiry is checked against; the ownership fields
/// matter only to the non-RBAC ACL build.
#[derive(Debug, Clone, Copy)]
pub struct AccessQuery<'a> {
    pub agent_id: &'a str,
    pub graph_name: &'a str,
    pub graph_type: GraphType,
    pub graph_owner: Option<&'a str>,
    pub access: AccessLevel,
    pub now_ms: u64,
}

/// Why the chokepoint allowed (or refused) one graph access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessBasis {
    Denied,
    /// A System identity, a standing RBAC grant, or (non-security builds) the
    /// ownership ACL.
    Standing,
    /// Allowed only by the named just-in-time elevation (EH-404). Callers
    /// audit every such use.
    Elevation(String),
}

impl AccessBasis {
    pub fn is_allowed(&self) -> bool {
        !matches!(self, Self::Denied)
    }
}

/// Wall-clock milliseconds: the clock an elevation's hard expiry is checked
/// against on every access decision.
pub fn access_clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The source-group member sets of one view, built once per filtering pass.
#[cfg(feature = "security")]
pub(crate) type SourceGroupMemo =
    std::cell::OnceCell<std::sync::Arc<medulla_graph_core::rls::SourceGroups>>;

/// The one source access decision (MEDULLA-IDENTITY-R706), shared by the layer's own
/// filter and the policy lease's mirror of it.
#[cfg(feature = "security")]
pub(crate) fn source_gate(
    agent_id: &str,
    is_system: bool,
    roles: &[String],
    view: &medulla_graph_core::graph::GraphView,
    id: &str,
    groups: &SourceGroupMemo,
) -> bool {
    use medulla_graph_core::rls;
    let Some(blob) = view.node_properties.get(id) else {
        return true;
    };
    // Cheap byte probe so ungoverned nodes never pay a property decode.
    let key = rls::SOURCE_ACCESS_PUBLIC_KEY.as_bytes();
    if !blob.windows(key.len()).any(|window| window == key) {
        return true;
    }
    let Some(access) = rls::source_access(blob) else {
        return true;
    };
    if is_system {
        return true;
    }
    rls::source_access_verdict(
        &access,
        agent_id,
        roles,
        &mut || {
            groups
                .get_or_init(|| std::sync::Arc::new(rls::SourceGroups::from_view(view)))
                .clone()
        },
        access_clock_ms(),
        rls::source_acl_stale_after_s(),
    )
    .allows()
}

impl IsolationLayer {
    pub fn check_access(
        &self,
        agent_id: &str,
        graph_name: &str,
        graph_type: GraphType,
        graph_owner: Option<&str>,
        access: AccessLevel,
    ) -> bool {
        self.access_basis(&AccessQuery {
            agent_id,
            graph_name,
            graph_type,
            graph_owner,
            access,
            now_ms: access_clock_ms(),
        })
        .is_allowed()
    }

    /// The access decision, naming what allowed it. An elevation is consulted
    /// only when no standing grant decided: an explicit RBAC `Deny` is never
    /// overridden by an elevation.
    pub fn access_basis(&self, query: &AccessQuery<'_>) -> AccessBasis {
        #[cfg(feature = "security")]
        {
            let Some(store) = self.persist.as_ref() else {
                return AccessBasis::Denied;
            };
            let Ok(authority) = store.authority_snapshot() else {
                return AccessBasis::Denied;
            };
            let Some(identity) = authority.identities().get(query.agent_id) else {
                return AccessBasis::Denied;
            };
            if identity.role == AgentRole::System {
                return AccessBasis::Standing;
            }
            Self::check_non_system_access(authority.policy(), identity, query)
        }
        #[cfg(not(feature = "security"))]
        {
            let Some(identity) = self.agents.get(query.agent_id) else {
                return AccessBasis::Denied;
            };
            if identity.role == AgentRole::System {
                return AccessBasis::Standing;
            }
            self.check_non_system_access(identity, query)
        }
    }

    #[cfg(feature = "security")]
    fn check_non_system_access(
        policy: &crate::rbac::RbacPolicy,
        identity: &AgentIdentity,
        query: &AccessQuery<'_>,
    ) -> AccessBasis {
        // Mandatory RBAC means no pre-RBAC ACL fall-through on empty/no-match.
        let context = medulla_types::acl::ResourceContext::graph(query.graph_name);
        let (action, elevation_action) = match query.access {
            AccessLevel::Read => (
                medulla_types::acl::RbacAction::Read,
                medulla_identity_types::rbac_elevation::ElevationAction::Read,
            ),
            AccessLevel::Write => (
                medulla_types::acl::RbacAction::Write,
                medulla_identity_types::rbac_elevation::ElevationAction::Write,
            ),
        };
        match policy.evaluate(&identity.roles, &context, action) {
            Some(medulla_types::acl::GrantEffect::Allow) => AccessBasis::Standing,
            Some(medulla_types::acl::GrantEffect::Deny) => AccessBasis::Denied,
            None => policy
                .elevations()
                .permitting(
                    query.agent_id,
                    query.graph_name,
                    elevation_action,
                    query.now_ms,
                )
                .map_or(AccessBasis::Denied, |id| {
                    AccessBasis::Elevation(id.to_string())
                }),
        }
    }

    #[cfg(not(feature = "security"))]
    fn check_non_system_access(
        &self,
        identity: &AgentIdentity,
        query: &AccessQuery<'_>,
    ) -> AccessBasis {
        let allowed = match query.graph_type {
            GraphType::Commons => true,
            GraphType::Global => query.access == AccessLevel::Read,
            GraphType::Agent => {
                query.graph_owner == Some(query.agent_id)
                    || query
                        .graph_owner
                        .is_some_and(|owner| self.is_manager_of(query.agent_id, owner))
            }
            GraphType::Team => {
                let team_name = query
                    .graph_name
                    .strip_prefix("team:")
                    .unwrap_or(query.graph_name);
                identity.teams.contains(&team_name.to_string())
                    && (query.access == AccessLevel::Read
                        || matches!(identity.role, AgentRole::Manager { .. }))
            }
        };
        if allowed {
            AccessBasis::Standing
        } else {
            AccessBasis::Denied
        }
    }

    fn is_manager_of(&self, agent_id: &str, subordinate_id: &str) -> bool {
        self.agents.get(agent_id).is_some_and(|identity| {
            matches!(
                &identity.role,
                AgentRole::Manager { subordinates }
                    if subordinates.iter().any(|id| id == subordinate_id)
            )
        })
    }

    #[cfg(feature = "security")]
    pub fn is_system(&self, agent_id: &str) -> bool {
        self.agents
            .get(agent_id)
            .is_some_and(|identity| identity.role == AgentRole::System)
    }

    #[cfg(feature = "security")]
    pub fn can_see_row(&self, agent_id: &str, visibility: &RowVisibility) -> bool {
        let Some(owner) = visibility.owner.as_deref() else {
            return self.is_system(agent_id)
                || visibility.schema
                || (visibility.tagged && visibility.public);
        };
        self.is_system(agent_id)
            || visibility.schema
            || visibility.public
            || owner == agent_id
            || visibility.grants.iter().any(|grant| grant == agent_id)
            || self.is_manager_of(agent_id, owner)
    }

    #[cfg(feature = "security")]
    pub fn can_see_node(
        &self,
        agent_id: &str,
        view: &medulla_graph_core::graph::GraphView,
        id: &str,
    ) -> bool {
        self.can_see_node_with_groups(agent_id, view, id, &std::cell::OnceCell::new())
    }

    /// The row decision followed by the source access gate (MEDULLA-IDENTITY-R706).
    /// `groups` memoizes the view's source-group member sets for one filtering pass.
    #[cfg(feature = "security")]
    fn can_see_node_with_groups(
        &self,
        agent_id: &str,
        view: &medulla_graph_core::graph::GraphView,
        id: &str,
        groups: &SourceGroupMemo,
    ) -> bool {
        let mut visibility = view
            .visibility_index
            .get(id)
            .cloned()
            .or_else(|| {
                view.node_properties
                    .get(id)
                    .map(|blob| row_visibility(blob))
            })
            .unwrap_or_else(RowVisibility::default_public);
        visibility.schema = view.schema_node_ids.contains(id);
        if !self.can_see_row(agent_id, &visibility) {
            return false;
        }
        self.source_gate_allows(agent_id, view, id, groups)
    }

    /// The source access gate: a governed source record is visible only to a caller its
    /// access properties admit; group references expand through the view's `source_group`
    /// nodes. A System caller bypasses it, as it bypasses every row rule.
    #[cfg(feature = "security")]
    fn source_gate_allows(
        &self,
        agent_id: &str,
        view: &medulla_graph_core::graph::GraphView,
        id: &str,
        groups: &SourceGroupMemo,
    ) -> bool {
        let roles: &[String] = self
            .agents
            .get(agent_id)
            .map_or(&[], |identity| identity.roles.as_slice());
        source_gate(agent_id, self.is_system(agent_id), roles, view, id, groups)
    }

    #[cfg(feature = "security")]
    pub fn filter_view(&self, agent_id: &str, view: &mut medulla_graph_core::graph::GraphView) {
        let groups = std::cell::OnceCell::new();
        let hidden: HashSet<String> = view
            .node_map
            .keys()
            .filter(|id| !self.can_see_node_with_groups(agent_id, view, id, &groups))
            .cloned()
            .collect();
        for id in &hidden {
            if let Some(index) = view.node_map.remove(id) {
                view.graph.remove_node(index);
            }
            view.node_properties.remove(id);
        }
        view.edge_properties
            .retain(|(source, target), _| !hidden.contains(source) && !hidden.contains(target));
    }

    pub fn has_admin_capability(&self, agent_id: &str) -> bool {
        #[cfg(feature = "security")]
        {
            let Some(store) = self.persist.as_ref() else {
                return false;
            };
            let Ok(authority) = store.authority_snapshot() else {
                return false;
            };
            let Some(identity) = authority.identities().get(agent_id) else {
                return false;
            };
            if identity.role == AgentRole::System {
                return true;
            }
            let context = medulla_types::acl::ResourceContext::graph("__admin__");
            matches!(
                authority.policy().evaluate(
                    &identity.roles,
                    &context,
                    medulla_types::acl::RbacAction::Admin,
                ),
                Some(medulla_types::acl::GrantEffect::Allow)
            )
        }
        #[cfg(not(feature = "security"))]
        self.agents
            .get(agent_id)
            .is_some_and(|identity| identity.role == AgentRole::System)
    }
}
