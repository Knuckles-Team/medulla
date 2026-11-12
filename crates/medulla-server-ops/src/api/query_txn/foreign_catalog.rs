//! Owner-scoped foreign-source catalog (CONCEPT:MEDULLA-KG.query.query-federation, EH-373).
//!
//! `Method::RegisterForeignSource` records a named [`ForeignSourceSpec`]. A
//! `RemoteEngine` spec carries a shared `secret` and a signed request `context`, so a
//! registration is a credential. Before EH-373 the server kept ONE process-global
//! `name → spec` map and every caller's plan resolved against it: any principal holding
//! only `query:unified` could name another principal's source (`FOREIGN "<name>"`, a
//! `Named` `Op::ForeignScan`, or an NL-planned query) and make the server spend that
//! principal's credential on its behalf — a confused deputy.
//!
//! This module is the ONE place that scoping happens:
//!
//! * entries are keyed by `(owner, name)`, where the owner is
//!   [`CarrierAuthority::owner_scope`] — the verified tenant AND principal (actor). It
//!   comes only from a [`CarrierAuthority`] derived from the verified request envelope,
//!   never from a request field. One engine is bound to ONE tenant
//!   (`MEDULLA_TENANT`; every verified carrier carries it), so the principal is
//!   the boundary that matters today; the tenant stays inside the owner scope so
//!   cross-tenant stays closed if engines ever serve more than one tenant. This is the
//!   same least-privilege ownership the SQL catalog uses.
//! * [`ForeignSourceCatalog::registry_for`] is the only way to turn catalog entries into
//!   the executor's [`ForeignSourceRegistry`], and it copies only the caller's own
//!   entries. Every served plan path (`UnifiedQuery`, `Uql`, the
//!   policy-lease text path, in-txn UQL, `NlQuery`, and the wire-protocol UQL path)
//!   builds its registry through [`ForeignSourceCatalog::resolve_for_plan`].
//!
//! Sharing is explicit (EH-378, `medulla_server::foreign_share`): an administrator assigns the
//! engine-provisioned `foreign-source-use:<owner agent>/<name>` role, and the grantee then
//! addresses the source as `<owner agent>/<name>`. Without that exact grant, a plain or
//! qualified name another owner registered resolves exactly like an entirely unknown name,
//! preserving the centralized authorization contract's no-existence-leak rule. If the caller
//! does hold the exact grant but supplies the unqualified plain name, the catalog returns the
//! stable `ACCESS_DENIED` code and the caller must use the qualified shared name.
//!
//! **Foreign rows are not RLS-filtered.** A foreign source's rows come from outside
//! the local snapshot, so the row-level visibility filter that governs local graph
//! reads never sees them. Their only access control is the remote side's own
//! authorization of the registered credential, plus this owner scoping of who may
//! use that credential.
//!
//! Registrations live in memory only. `RegisterForeignSource` is a `ControlRedb`
//! session-control saga whose durable record is an opaque receipt; the endpoint
//! configuration is never persisted, so there is no stored key shape to migrate.
//!
//! **Outbound verification at registration time (`MEDULLA-UNIFIED-DATA-PLANE-R002.1`).**
//! A `Sql` spec's DSN is checked against the existing SSRF-sensitive destination gate
//! ([`medulla_plan::federation_ssrf::check_sql_dsn`], already enforced at query time by
//! `crates/medulla-plan/src/federation.rs`) BEFORE the entry is inserted, so a disallowed
//! destination is refused at `RegisterForeignSource` time rather than only failing
//! later when a plan tries to use it. Other [`ForeignSourceSpec`] variants are
//! unaffected; their own outbound gates (`HttpJson`'s `validate_http_json_target`,
//! `RemoteEngine`'s dial-time checks) run where they already ran before this change —
//! unifying every variant's registration-time gate is tracked separately as
//! `MEDULLA-UNIFIED-DATA-PLANE-R002.2`/`.3`.

use std::sync::Arc;

use dashmap::DashMap;
use medulla_catalog_types::attached_catalog::AttachedCatalogGraph;
#[cfg(feature = "federation")]
use medulla_catalog_types::connector_pack::PackSourceKind;
use medulla_catalog_types::probe_machine::{ProbeRecord, ProbeState};
use medulla_catalog_types::{
    connector_pack::ConnectorPackSourceApprovalResult, connector_pack::PackSourceApproval,
    connector_pack::PackSourceRegistration,
};
use medulla_federation::{federation::ForeignSourceRegistry, federation::SharedForeignSource};
use medulla_types::contract::Digest256;
use medulla_types::wire::ForeignSourceSpec;

use super::foreign_share::{may_use_shared, share_resource, shared_name};
use medulla_isolation::isolation::IsolationLayer;
use medulla_server_api::api::persistence::SourceApprovalStore;
use medulla_server_api::carrier::CarrierAuthority;

/// `(verified owner scope, source name)` — the catalog key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CatalogKey {
    owner_scope: String,
    name: String,
}

/// A registered spec plus the registering principal's agent id, which names the
/// source's share resource (EH-378).
struct OwnedSpec {
    owner_agent: String,
    spec: ForeignSourceSpec,
    /// The catalog probe of this source (MEDULLA-UNIFIED-DATA-PLANE-R014.2.3): the one state
    /// field and the baseline digest. Registering a source starts a fresh record.
    probe: ProbeRecord,
    /// The catalog read by the last probe that completed.
    catalog: Option<AttachedCatalogGraph>,
}

/// Host-derived identity for one request-scoped foreign-driver binding.
///
/// The descriptor carries no authority. `bind_for` compares it with both the
/// owner catalog entry and the durable approval result before it can reach the
/// trusted Rust callback seam in `medulla-plan`.
#[derive(Clone, Debug)]
pub struct ForeignSourceBindingDescriptor {
    pub name: String,
    pub spec: ForeignSourceSpec,
    pub registration: PackSourceRegistration,
    pub contract_digest: Digest256,
}

/// Every owner's registered foreign sources, partitioned by verified owner
/// (tenant+principal) scope.
#[derive(Default)]
pub struct ForeignSourceCatalog {
    entries: DashMap<CatalogKey, OwnedSpec>,
}

/// A caller's registry, built by [`ForeignSourceCatalog::registry_for`]: its own sources
/// by plain name plus any explicitly shared ones by qualified name. It carries a digest
/// of exactly what it resolved so result caches never serve rows across owners, grants,
/// revocations or re-registrations.
pub struct OwnedForeignRegistry {
    owner_scope: String,
    cache_salt: String,
    registry: ForeignSourceRegistry,
}

impl OwnedForeignRegistry {
    /// Install the EH-400 freshness proof for this query's named sources.
    #[cfg(feature = "result-cache")]
    pub fn install_fragment_cache(
        &mut self,
        sources: std::collections::HashMap<
            String,
            medulla_federation::federation_opt::SourceWatermark,
        >,
    ) {
        self.registry.set_cache_scope(std::sync::Arc::new(
            medulla_federation::federation_opt::FragmentCacheScope::new(
                self.cache_salt.clone(),
                sources,
            ),
        ));
    }
    /// The verified owner (tenant+principal) scope this registry was built for.
    pub fn owner_scope(&self) -> &str {
        &self.owner_scope
    }

    /// Digest of `(owner scope, every resolved (name, spec))`, for result-cache keys.
    pub fn cache_salt(&self) -> &str {
        &self.cache_salt
    }

    /// The executor registry holding only this owner's sources.
    pub fn registry(&self) -> &ForeignSourceRegistry {
        &self.registry
    }
}

impl ForeignSourceCatalog {
    /// Record (or replace) `owner`'s source `name`. Another owner's entry under the
    /// same name is a different key, so it can be neither read nor overwritten here.
    ///
    /// `MEDULLA-UNIFIED-DATA-PLANE-R002.1`: a `Sql` spec's DSN is checked against the
    /// outbound destination gate FIRST; a disallowed destination is refused (`Err`)
    /// and nothing is inserted, so an unverified endpoint never reaches the catalog.
    pub fn register(
        &self,
        owner: &CarrierAuthority,
        name: String,
        spec: ForeignSourceSpec,
    ) -> Result<(), String> {
        validate_search_capability(&spec)?;
        verify_outbound_before_register(&spec)?;
        let key = CatalogKey {
            owner_scope: owner.owner_scope().to_string(),
            name,
        };
        let owner_agent = owner.agent_id().to_string();
        self.entries.insert(
            key,
            OwnedSpec {
                owner_agent,
                spec,
                probe: ProbeRecord::default(),
                catalog: None,
            },
        );
        Ok(())
    }

    /// THE scoping chokepoint: the executor registry holding `caller`'s own
    /// (tenant+principal) sources by plain name, plus each other owner's source the
    /// caller holds an exact share grant for (EH-378) by `<owner agent>/<name>`.
    /// Shared entries are registered first so a caller's own name always wins.
    pub fn registry_for(
        &self,
        caller: &CarrierAuthority,
        isolation: &IsolationLayer,
    ) -> OwnedForeignRegistry {
        let owner_scope = caller.owner_scope();
        let (mut shared, mut own) = (Vec::new(), Vec::new());
        for entry in self.entries.iter() {
            let (key, value) = (entry.key(), entry.value());
            if key.owner_scope == owner_scope {
                own.push((key.name.clone(), value.spec.clone()));
            } else if may_use_shared(
                isolation,
                caller.agent_id(),
                &share_resource(&value.owner_agent, &key.name),
            ) {
                shared.push((
                    shared_name(&value.owner_agent, &key.name),
                    value.spec.clone(),
                ));
            }
        }
        shared.sort_by(|a, b| a.0.cmp(&b.0));
        own.sort_by(|a, b| a.0.cmp(&b.0));
        let cache_salt = resolved_digest(owner_scope, shared.iter().chain(&own));
        let mut registry = ForeignSourceRegistry::new();
        for (name, spec) in shared.into_iter().chain(own) {
            registry.register_spec(name, spec);
        }
        OwnedForeignRegistry {
            owner_scope: owner_scope.to_string(),
            cache_salt,
            registry,
        }
    }

    /// Build a request-scoped registry with one already-admitted driver bound.
    ///
    /// The network request never supplies the callback or the descriptor. A
    /// trusted host adapter supplies both after deriving the carrier from the
    /// verified request. This method still re-checks the durable approval and
    /// the current owner catalog entry immediately before binding so a stale
    /// receipt, another owner's catalog, or a descriptor mismatch fails closed.
    pub fn bind_for(
        &self,
        caller: &CarrierAuthority,
        store: &dyn SourceApprovalStore,
        isolation: &IsolationLayer,
        receipt: &ConnectorPackSourceApprovalResult,
        descriptor: &ForeignSourceBindingDescriptor,
        driver: SharedForeignSource,
    ) -> Result<OwnedForeignRegistry, String> {
        self.verify_binding(caller, store, receipt, descriptor)?;

        let mut scoped = self.registry_for(caller, isolation);
        scoped.registry.register_bound_trusted_host(
            descriptor.name.clone(),
            descriptor.spec.clone(),
            driver,
        );
        Ok(scoped)
    }

    /// Revalidate an already-open host execution without installing a driver.
    /// Chunks and terminal transitions must observe the same current catalog
    /// and durable approval as the eventual query bind.
    #[cfg(feature = "federation")]
    pub fn verify_registered_for(
        &self,
        caller: &CarrierAuthority,
        store: &dyn SourceApprovalStore,
        receipt: &ConnectorPackSourceApprovalResult,
        name: &str,
    ) -> Result<(), String> {
        let descriptor = self.registered_descriptor(caller, receipt, name)?;
        self.verify_binding(caller, store, receipt, &descriptor)
    }

    fn verify_binding(
        &self,
        caller: &CarrierAuthority,
        store: &dyn SourceApprovalStore,
        receipt: &ConnectorPackSourceApprovalResult,
        descriptor: &ForeignSourceBindingDescriptor,
    ) -> Result<(), String> {
        if !caller.can_read() {
            return Err(
                "ACCESS_DENIED: a foreign source binding requires verified read authority"
                    .to_string(),
            );
        }
        let (tenant, principal) = caller.log_owner();
        if tenant != receipt.tenant_id {
            return Err("ACCESS_DENIED: source approval belongs to another tenant".to_string());
        }
        if principal.is_empty() || caller.owner_scope().is_empty() {
            return Err("ACCESS_DENIED: source binding has no verified owner".to_string());
        }
        if descriptor.name.is_empty() {
            return Err("SOURCE_BINDING_INVALID: source name is empty".to_string());
        }
        if receipt.registration.approval != PackSourceApproval::Approved {
            return Err("SOURCE_APPROVAL_NOT_ADMITTED: registration is not approved".to_string());
        }
        if descriptor.registration != receipt.registration
            || descriptor.contract_digest != receipt.contract_digest
        {
            return Err(
                "SOURCE_BINDING_MISMATCH: descriptor differs from approved registration"
                    .to_string(),
            );
        }

        #[cfg(feature = "federation")]
        if !approved_operation_matches(&descriptor.name, &descriptor.spec, &receipt.registration) {
            return Err(
                "SOURCE_BINDING_MISMATCH: approved operation differs from registered source"
                    .to_string(),
            );
        }

        // The durable member/head/body and the exact stored registration are
        // authoritative. The receipt is only the candidate reference.
        store.verify_connector_pack_source_approval(receipt)?;

        let key = CatalogKey {
            owner_scope: caller.owner_scope().to_string(),
            name: descriptor.name.clone(),
        };
        let entry = self.entries.get(&key).ok_or_else(|| {
            "SOURCE_BINDING_NOT_REGISTERED: source is absent from the caller catalog".to_string()
        })?;
        if entry.value().spec != descriptor.spec {
            return Err(
                "SOURCE_BINDING_MISMATCH: descriptor differs from owner catalog".to_string(),
            );
        }
        Ok(())
    }

    /// Bind a host-driven page source after deriving the descriptor from the
    /// caller's current catalog entry. The wire request supplies only the
    /// source name and approval receipt; it cannot choose a `ForeignSourceSpec`
    /// or smuggle authority through the callback seam.
    #[cfg(feature = "federation")]
    pub fn bind_registered_for(
        &self,
        caller: &CarrierAuthority,
        store: &dyn SourceApprovalStore,
        isolation: &IsolationLayer,
        receipt: &ConnectorPackSourceApprovalResult,
        name: &str,
        driver: SharedForeignSource,
    ) -> Result<OwnedForeignRegistry, String> {
        let descriptor = self.registered_descriptor(caller, receipt, name)?;
        self.bind_for(caller, store, isolation, receipt, &descriptor, driver)
    }

    /// Bind several already-completed page sources into one owner-scoped registry.
    /// Every durable approval is checked before any driver is installed.
    #[cfg(feature = "federation")]
    pub fn bind_registered_many(
        &self,
        caller: &CarrierAuthority,
        store: &dyn SourceApprovalStore,
        isolation: &IsolationLayer,
        sources: &[(
            ConnectorPackSourceApprovalResult,
            String,
            SharedForeignSource,
        )],
    ) -> Result<OwnedForeignRegistry, String> {
        let mut descriptors = Vec::with_capacity(sources.len());
        let mut names = std::collections::HashSet::new();
        for (receipt, name, _) in sources {
            if !names.insert(name.as_str()) {
                return Err("INVALID_ARGUMENT: repeated foreign source name".to_string());
            }
            let descriptor = self.registered_descriptor(caller, receipt, name)?;
            self.verify_binding(caller, store, receipt, &descriptor)?;
            descriptors.push(descriptor);
        }
        let mut scoped = self.registry_for(caller, isolation);
        for ((_, _, driver), descriptor) in sources.iter().zip(descriptors) {
            scoped.registry.register_bound_trusted_host(
                descriptor.name,
                descriptor.spec,
                Arc::clone(driver),
            );
        }
        Ok(scoped)
    }

    fn registered_descriptor(
        &self,
        caller: &CarrierAuthority,
        receipt: &ConnectorPackSourceApprovalResult,
        name: &str,
    ) -> Result<ForeignSourceBindingDescriptor, String> {
        let key = CatalogKey {
            owner_scope: caller.owner_scope().to_string(),
            name: name.to_string(),
        };
        let spec = self
            .entries
            .get(&key)
            .map(|entry| entry.value().spec.clone())
            .ok_or_else(|| {
                "SOURCE_BINDING_NOT_REGISTERED: source is absent from the caller catalog"
                    .to_string()
            })?;
        let descriptor = ForeignSourceBindingDescriptor {
            name: name.to_string(),
            spec,
            registration: receipt.registration.clone(),
            contract_digest: receipt.contract_digest,
        };
        Ok(descriptor)
    }

    /// Resolve the registry a served plan needs: `None` when the plan names no
    /// registered source, the caller's own registry otherwise. A name-resolving plan
    /// without a verified carrier is refused rather than run with no scope.
    pub fn resolve_for_plan(
        &self,
        ops: &[medulla_exec::algebra::Op],
        caller: Option<&CarrierAuthority>,
        isolation: &IsolationLayer,
    ) -> Result<Option<OwnedForeignRegistry>, String> {
        if !crate::api::query_txn::query_support::plan_needs_foreign(ops) {
            return Ok(None);
        }
        let Some(caller) = caller else {
            medulla_kernel::metrics::process().access_denied();
            return Err(
                "ACCESS_DENIED: a named foreign source requires a verified tenant carrier"
                    .to_string(),
            );
        };
        let registry = self.registry_for(caller, isolation);
        if self.plan_names_another_owners_source(ops, caller, isolation) {
            medulla_kernel::metrics::process().access_denied();
            return Err(
                "ACCESS_DENIED: foreign source is outside the caller's owner scope".to_string(),
            );
        }
        Ok(Some(registry))
    }

    /// Return whether a plan names another owner's source by its unqualified name while the
    /// caller holds an exact share grant for that source. The grant makes the source identity
    /// independently visible, so this wrong-name path may return `ACCESS_DENIED`; without the
    /// grant, known and unknown names both remain the executor's typed not-found response.
    fn plan_names_another_owners_source(
        &self,
        ops: &[medulla_exec::algebra::Op],
        caller: &CarrierAuthority,
        isolation: &IsolationLayer,
    ) -> bool {
        ops.iter().any(|op| match op {
            medulla_exec::algebra::Op::Foreign { name } => {
                self.name_belongs_to_another_owner(name, caller, isolation)
            }
            medulla_exec::algebra::Op::ForeignScan { source, .. } => match source.as_ref() {
                medulla_types::wire::ForeignSourceSpec::Named { name } => {
                    self.name_belongs_to_another_owner(name, caller, isolation)
                }
                _ => false,
            },
            #[cfg(feature = "text")]
            medulla_exec::algebra::Op::FuseRrf { branches, .. } => branches
                .iter()
                .any(|branch| self.plan_names_another_owners_source(branch, caller, isolation)),
            _ => false,
        })
    }

    fn name_belongs_to_another_owner(
        &self,
        name: &str,
        caller: &CarrierAuthority,
        isolation: &IsolationLayer,
    ) -> bool {
        let own_key = CatalogKey {
            owner_scope: caller.owner_scope().to_string(),
            name: name.to_string(),
        };
        !self.entries.contains_key(&own_key)
            && self.entries.iter().any(|entry| {
                entry.key().name == name
                    && entry.key().owner_scope != caller.owner_scope()
                    && may_use_shared(
                        isolation,
                        caller.agent_id(),
                        &share_resource(&entry.value().owner_agent, &entry.key().name),
                    )
            })
    }

    /// The spec `owner` registered under `name`, if any.
    #[cfg(any(test, feature = "test-support"))]
    pub fn spec_for(&self, owner: &CarrierAuthority, name: &str) -> Option<ForeignSourceSpec> {
        let key = CatalogKey {
            owner_scope: owner.owner_scope().to_string(),
            name: name.to_string(),
        };
        self.entries
            .get(&key)
            .map(|entry| entry.value().spec.clone())
    }

    /// Whether no owner has registered any source.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The probe state of `owner`'s source `name` (MEDULLA-UNIFIED-DATA-PLANE-R014.2.3).
    pub fn probe_state(&self, owner: &CarrierAuthority, name: &str) -> Option<ProbeState> {
        self.entries
            .get(&CatalogKey {
                owner_scope: owner.owner_scope().to_string(),
                name: name.to_string(),
            })
            .map(|entry| entry.value().probe.state())
    }

    /// The catalog the probe of `owner`'s source `name` read. A source whose schema moved off
    /// its baseline is refused `SCHEMA_DRIFT` (re-registering the source takes a new baseline);
    /// a failed or not yet finished probe is refused `SOURCE_UNAVAILABLE`.
    pub fn catalog_read(
        &self,
        owner: &CarrierAuthority,
        name: &str,
    ) -> Result<AttachedCatalogGraph, String> {
        let key = CatalogKey {
            owner_scope: owner.owner_scope().to_string(),
            name: name.to_string(),
        };
        let entry = self
            .entries
            .get(&key)
            .ok_or_else(|| format!("federation: unknown foreign source '{name}'"))?;
        match (entry.probe.state(), &entry.catalog) {
            (ProbeState::Baselined, Some(graph)) => Ok(graph.clone()),
            (ProbeState::Drifted, _) => {
                Err("SCHEMA_DRIFT: the source schema changed since its baseline".to_string())
            }
            (state, _) => Err(format!(
                "SOURCE_UNAVAILABLE: the catalog probe is {}",
                medulla_types::transition_refused::MachineState::stored(state)
            )),
        }
    }

    /// The DSN of a MySQL-family source, if `spec` is one.
    fn mysql_family_dsn(spec: &ForeignSourceSpec) -> Option<&str> {
        match spec {
            ForeignSourceSpec::Sql { dsn, .. }
                if dsn.starts_with("mysql://") || dsn.starts_with("mariadb://") =>
            {
                Some(dsn.as_str())
            }
            _ => None,
        }
    }

    /// The DSN of a PostgreSQL source, if `spec` is one (MEDULLA-UNIFIED-DATA-PLANE-R013.1).
    fn postgres_dsn(spec: &ForeignSourceSpec) -> Option<&str> {
        match spec {
            ForeignSourceSpec::Sql { dsn, .. }
                if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") =>
            {
                Some(dsn.as_str())
            }
            _ => None,
        }
    }

    /// Whether `owner`'s source `name` is a MySQL, MariaDB or PostgreSQL source that needs a
    /// catalog probe.
    pub fn needs_catalog_probe(&self, owner: &CarrierAuthority, name: &str) -> bool {
        self.spec_for_probe(owner.owner_scope(), name)
            .is_some_and(|spec| {
                Self::mysql_family_dsn(&spec).is_some() || Self::postgres_dsn(&spec).is_some()
            })
    }

    fn spec_for_probe(&self, owner_scope: &str, name: &str) -> Option<ForeignSourceSpec> {
        self.entries
            .get(&CatalogKey {
                owner_scope: owner_scope.to_string(),
                name: name.to_string(),
            })
            .map(|entry| entry.value().spec.clone())
    }

    /// The probe job of MEDULLA-UNIFIED-DATA-PLANE-R014.2.3: claim the source
    /// (`ProbeStarted`), read its live catalog (R014.2.2) and record the outcome as the next
    /// machine event. The first success captures the baseline digest. Registration never waits
    /// for this job. Returns the state the probe ended in.
    #[cfg(feature = "federation-sql")]
    pub async fn run_catalog_probe(
        &self,
        owner_scope: &str,
        name: &str,
    ) -> Result<ProbeState, String> {
        let key = CatalogKey {
            owner_scope: owner_scope.to_string(),
            name: name.to_string(),
        };
        let spec = self
            .spec_for_probe(owner_scope, name)
            .ok_or_else(|| format!("federation: unknown foreign source '{name}'"))?;
        let postgres = Self::postgres_dsn(&spec).map(str::to_string);
        let dsn = match &postgres {
            Some(dsn) => dsn.clone(),
            None => Self::mysql_family_dsn(&spec)
                .ok_or_else(|| {
                    "CAPABILITY_UNSUPPORTED: not a MySQL, MariaDB or PostgreSQL source".to_string()
                })?
                .to_string(),
        };
        {
            let mut entry = self
                .entries
                .get_mut(&key)
                .ok_or_else(|| format!("federation: unknown foreign source '{name}'"))?;
            entry.probe.start().map_err(|refused| {
                format!(
                    "{}: {} {} on {}",
                    medulla_types::transition_refused::TRANSITION_REFUSED,
                    refused.machine,
                    refused.event,
                    refused.from
                )
            })?;
        }
        let pool_size = medulla_server_api::knob_config::knobs().foreign_mysql_pool_size as u32;
        let read = if postgres.is_some() {
            medulla_plan::postgres_catalog::read_catalog(&dsn, &[], pool_size).await
        } else {
            medulla_plan::mysql_catalog::read_catalog(&dsn, &[], pool_size)
                .await
                .map(|read| read.graph)
        };
        let mut entry = self
            .entries
            .get_mut(&key)
            .ok_or_else(|| format!("federation: unknown foreign source '{name}'"))?;
        let outcome = match read {
            Ok(graph) => {
                let digest = graph.content_hash();
                entry.catalog = Some(graph);
                entry.probe.finish_read(digest)
            }
            Err(_) => entry.probe.finish_error(),
        };
        outcome.map_err(|refused| {
            format!(
                "{}: {} {} on {}",
                medulla_types::transition_refused::TRANSITION_REFUSED,
                refused.machine,
                refused.event,
                refused.from
            )
        })
    }
}

/// `MEDULLA-UNIFIED-DATA-PLANE-R002.1` — the registration-time outbound gate. A `Sql` spec's
/// DSN must pass the same SSRF-sensitive destination check `crates/medulla-plan/src/federation.rs`
/// already runs at query time, so a disallowed destination is refused before persistence
/// rather than only when a plan later tries to use it. Other variants are untouched here;
/// their own gates still run where they ran before (see the module doc).
fn verify_outbound_before_register(spec: &ForeignSourceSpec) -> Result<(), String> {
    if let ForeignSourceSpec::SqliteFile { path, .. } = spec {
        return verify_sqlite_root(path, medulla_server_api::knob_config::allowed_roots());
    }
    #[cfg(feature = "federation-sql")]
    if let ForeignSourceSpec::Sql { dsn, .. } = spec {
        return medulla_federation::federation_ssrf::check_sql_dsn(dsn);
    }
    let _ = spec;
    Ok(())
}

/// `MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.3` — a `SqliteFile` spec's canonical path must lie
/// under one of `foreign.sqlite.allowed_roots`. The default is an empty list, so every
/// path is refused (`SQLITE_FILE_DENIED`). Canonicalisation resolves `..` and symlinks, so
/// neither can escape a root; a path that does not exist cannot be canonicalised and is
/// refused too.
fn verify_sqlite_root(path: &str, roots: &[std::path::PathBuf]) -> Result<(), String> {
    let denied = medulla_types::contract::ServerErrorCode::SqliteFileDenied;
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return Err(format!(
            "{denied}: sqlite file path is not an existing file"
        ));
    };
    let under_root = roots
        .iter()
        .any(|root| std::fs::canonicalize(root).is_ok_and(|root| canonical.starts_with(root)));
    if under_root {
        Ok(())
    } else {
        Err(format!(
            "{denied}: sqlite file is outside foreign.sqlite.allowed_roots"
        ))
    }
}

/// Compare the approved operation to the catalog spec before binding host data.
/// The registration is authenticated separately against the durable pack head.
#[cfg(feature = "federation")]
fn approved_operation_matches(
    name: &str,
    spec: &ForeignSourceSpec,
    registration: &PackSourceRegistration,
) -> bool {
    if name != registration.source_id || name.is_empty() || registration.operation.is_empty() {
        return false;
    }
    match (registration.source_kind, spec) {
        (PackSourceKind::McpTool, ForeignSourceSpec::Mcp { server, tool, .. }) => {
            server == registration.connector.as_str() && tool == &registration.operation
        }
        (PackSourceKind::HttpEndpoint, ForeignSourceSpec::Api { operation, .. }) => {
            operation == &registration.operation
        }
        (
            PackSourceKind::GraphqlQuery,
            ForeignSourceSpec::GraphQl {
                endpoint,
                operation,
                ..
            },
        ) => endpoint == &registration.source_id && operation == &registration.operation,
        (PackSourceKind::A2aSkill, ForeignSourceSpec::A2a { agent, skill, .. }) => {
            agent == registration.connector.as_str() && skill == &registration.operation
        }
        // A Named source does not carry an independently checkable operation.
        _ => false,
    }
}

/// Length-prefixed SHA-256 over the owner scope and every resolved `(name, spec)`.
fn resolved_digest<'a>(
    owner_scope: &str,
    resolved: impl Iterator<Item = &'a (String, ForeignSourceSpec)>,
) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    let mut field = |bytes: &[u8]| {
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    };
    field(owner_scope.as_bytes());
    for (name, spec) in resolved {
        field(name.as_bytes());
        field(&rmp_serde::to_vec_named(spec).unwrap_or_default());
    }
    hex::encode(digest.finalize())
}

/// The executor registry a resolved leg binds, if any — the `ServedIndexes::foreign`
/// value every served plan path passes to `run_unified`.
pub fn bound_registry(foreign: &Option<OwnedForeignRegistry>) -> Option<&ForeignSourceRegistry> {
    foreign.as_ref().map(OwnedForeignRegistry::registry)
}

/// An operation-bound spec that declares a search index must declare a valid one
/// (`SEARCH_CAPABILITY_INVALID`, MEDULLA-UNIFIED-DATA-PLANE-R038.5.1).
fn validate_search_capability(spec: &ForeignSourceSpec) -> Result<(), String> {
    let capabilities = match spec {
        ForeignSourceSpec::Api { capabilities, .. }
        | ForeignSourceSpec::Mcp { capabilities, .. }
        | ForeignSourceSpec::A2a { capabilities, .. }
        | ForeignSourceSpec::GraphQl { capabilities, .. } => capabilities,
        _ => return Ok(()),
    };
    match capabilities.search {
        Some(search) => search.validate().map_err(|refusal| refusal.to_string()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    //! EH-373 catalog-level proofs: `(owner, name)` keying, per-owner resolution, and
    //! no cross-owner overwrite. The served-path proofs (UnifiedQuery, UQL text, NL)
    //! live in `medulla_server::tests::foreign_tenancy`.
    use super::*;

    /// A verified carrier for `agent` in the deployment's one tenant.
    fn carrier(agent: &str) -> CarrierAuthority {
        CarrierAuthority::verified_for_test(agent)
    }

    fn http_spec(url: &str) -> ForeignSourceSpec {
        ForeignSourceSpec::HttpJson {
            url: url.into(),
            json_path: "data".into(),
            field_map: medulla_types::wire::HttpFieldMap {
                id: "id".into(),
                score: None,
                columns: Default::default(),
            },
        }
    }

    #[test]
    fn sqlite_file_is_registered_only_under_an_allowed_root() {
        let base = std::env::temp_dir().join(format!("sqlite-roots-{}", std::process::id()));
        let (allowed, other) = (base.join("allowed"), base.join("other"));
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let inside = allowed.join("a.db");
        let outside = other.join("b.db");
        std::fs::write(&inside, b"").unwrap();
        std::fs::write(&outside, b"").unwrap();
        let roots = vec![allowed.clone()];
        assert!(verify_sqlite_root(inside.to_str().unwrap(), &roots).is_ok());
        // The default empty list refuses every path.
        let refused = verify_sqlite_root(inside.to_str().unwrap(), &[]).unwrap_err();
        assert!(refused.starts_with("SQLITE_FILE_DENIED"), "{refused}");
        // A path outside the root, and a `..` escape out of it, are refused.
        assert!(verify_sqlite_root(outside.to_str().unwrap(), &roots).is_err());
        let escape = allowed.join("../other/b.db");
        assert!(verify_sqlite_root(escape.to_str().unwrap(), &roots).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }

    fn foreign_plan(name: &str) -> Vec<medulla_exec::algebra::Op> {
        vec![medulla_exec::algebra::Op::Foreign { name: name.into() }]
    }

    #[test]
    fn same_name_resolves_to_each_owners_own_spec() {
        let (a, b) = (carrier("agent-a"), carrier("agent-b"));
        assert_eq!(a.tenant_scope(), b.tenant_scope(), "one engine, one tenant");
        let catalog = ForeignSourceCatalog::default();
        catalog
            .register(&a, "src".into(), http_spec("http://a.invalid/"))
            .expect("http sources are not outbound-gated here");
        catalog
            .register(&b, "src".into(), http_spec("http://b.invalid/"))
            .expect("http sources are not outbound-gated here");
        assert_eq!(
            catalog.spec_for(&a, "src"),
            Some(http_spec("http://a.invalid/"))
        );
        assert_eq!(
            catalog.spec_for(&b, "src"),
            Some(http_spec("http://b.invalid/")),
            "principal B's registration must not overwrite principal A's"
        );
        for caller in [&a, &b] {
            let scoped = catalog.registry_for(caller, &IsolationLayer::new());
            assert_eq!(scoped.owner_scope(), caller.owner_scope());
            assert_eq!(scoped.registry().len(), 1, "only the caller's own entry");
        }
    }

    #[test]
    fn another_owners_name_is_concealed_without_a_share_grant() {
        let (a, b) = (carrier("agent-a"), carrier("agent-b"));
        let catalog = ForeignSourceCatalog::default();
        catalog
            .register(&a, "secret_src".into(), http_spec("http://a.invalid/"))
            .expect("http sources are not outbound-gated here");
        let scoped = catalog
            .resolve_for_plan(
                &foreign_plan("secret_src"),
                Some(&b),
                &IsolationLayer::new(),
            )
            .expect("a verified caller is never refused outright")
            .expect("a FOREIGN plan binds a registry");
        let err = scoped
            .registry()
            .resolve("secret_src")
            .expect_err("known but ungranted source must remain concealed");
        assert!(
            err.contains("no foreign source registered under name 'secret_src'")
                && !err.contains("ACCESS_DENIED"),
            "known ungranted source must look unknown: {err}"
        );
    }

    #[test]
    fn unknown_name_keeps_typed_not_found_error() {
        let catalog = ForeignSourceCatalog::default();
        let caller = carrier("agent-a");
        let scoped = catalog
            .resolve_for_plan(
                &foreign_plan("unknown_src"),
                Some(&caller),
                &IsolationLayer::new(),
            )
            .expect("unknown names still bind the caller's empty registry")
            .expect("a FOREIGN plan binds a registry");
        let err = scoped
            .registry()
            .resolve("unknown_src")
            .expect_err("unknown source must remain a typed not-found refusal");
        assert!(err.contains("no foreign source registered under name 'unknown_src'"));
    }

    #[test]
    fn plan_scoping_needs_a_verified_carrier_only_for_named_sources() {
        let catalog = ForeignSourceCatalog::default();
        let refused = catalog.resolve_for_plan(&foreign_plan("src"), None, &IsolationLayer::new());
        assert!(matches!(refused, Err(ref e) if e.starts_with("ACCESS_DENIED")));
        let local = vec![medulla_exec::algebra::Op::Scan {
            label: "Doc".into(),
        }];
        assert!(matches!(
            catalog.resolve_for_plan(&local, None, &IsolationLayer::new()),
            Ok(None)
        ));
    }

    /// `MEDULLA-UNIFIED-DATA-PLANE-R002.1`: a `Sql` spec's DSN runs through the same
    /// SSRF-sensitive destination gate `crates/medulla-plan/src/federation.rs` runs at
    /// query time, but now BEFORE the entry is inserted into the catalog.
    #[cfg(feature = "federation-sql")]
    mod outbound_verification {
        use super::*;

        fn sql_spec(dsn: &str) -> ForeignSourceSpec {
            ForeignSourceSpec::Sql {
                dsn: dsn.into(),
                query: "SELECT id FROM t".into(),
                id_field: "id".into(),
                score_field: None,
                columns: Vec::new(),
            }
        }

        // spec: MEDULLA-UNIFIED-DATA-PLANE-R002
        // spec: MEDULLA-UNIFIED-DATA-PLANE-R002.1
        #[test]
        fn a_disallowed_sql_destination_is_refused_and_never_persisted() {
            let _allow = medulla_server_api::test_support::FederationAllowGuard::blocking("");
            let a = carrier("agent-a");
            let catalog = ForeignSourceCatalog::default();
            // Same fixture `crates/medulla-plan/src/federation_ssrf.rs` already proves is
            // refused: loopback on a non-default port, no allow-list entry.
            let err = catalog
                .register(
                    &a,
                    "bad_sql".into(),
                    sql_spec("postgres://u@127.0.0.1:5433/db"),
                )
                .expect_err("a disallowed SQL destination must be refused");
            assert!(!err.is_empty());
            assert_eq!(
                catalog.spec_for(&a, "bad_sql"),
                None,
                "a refused registration must not reach the catalog"
            );
        }

        // spec: MEDULLA-UNIFIED-DATA-PLANE-R002
        // spec: MEDULLA-UNIFIED-DATA-PLANE-R002.1
        #[test]
        fn an_allowed_sql_destination_still_registers() {
            let _allow =
                medulla_server_api::test_support::FederationAllowGuard::blocking("127.0.0.1:5432");
            let a = carrier("agent-a");
            let catalog = ForeignSourceCatalog::default();
            // Same fixture `crates/medulla-plan/src/federation_ssrf.rs` already proves is
            // admitted: loopback with an explicit exact host:port allowlist entry.
            catalog
                .register(
                    &a,
                    "ok_sql".into(),
                    sql_spec("postgres://u@127.0.0.1:5432/db"),
                )
                .expect("an allowed SQL destination must still register");
            assert!(catalog.spec_for(&a, "ok_sql").is_some());
        }
    }
}
