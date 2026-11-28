//! Namespace durability declarations (MEDULLA-DURABLE-KERNEL-R038.3).
//!
//! A namespace declares its [`DurabilityClass`] once; the declaration is a row of
//! `kv.redb` written through the same admitted MutationBatch as every other served
//! KV write, so it survives a restart and replays by operation identity. An
//! undeclared namespace is `sync`. A namespace declared `ephemeral` is served from
//! process memory and never touches the durable log (R038.2's
//! `refuse_if_durable_log_write` rule); its declaration is the only durable row.
//!
//! The declaration rows live under one reserved namespace. Served callers can never
//! name it: a served namespace is always the opaque `kv-namespace:<sha256>` /
//! `kv-shared-namespace:<sha256>` key of the verified tenant, so a declaration is
//! keyed by tenant and owner by construction.

use super::wire::{RowAccess, WireWrite};
use super::*;
use medulla_protocol_types::storage_wire::KvNamespaceClass;
use medulla_types::contract::ServerErrorCode;
use medulla_types::durability_class::DurabilityClass;

/// The reserved namespace that holds one declaration row per served namespace.
const DECLARATION_NAMESPACE: &str = "\u{1}kv-declaration";

/// The in-process rows of every namespace declared `ephemeral`, and the clock that
/// stamps their versions (seeded from wall time so a version never repeats across
/// a restart, which discards the rows).
pub(super) struct EphemeralRows {
    pub(super) rows: Mutex<BTreeMap<(String, String), Vec<u8>>>,
    clock: std::sync::atomic::AtomicU64,
}

impl EphemeralRows {
    pub(super) fn new() -> Self {
        Self {
            rows: Mutex::new(BTreeMap::new()),
            clock: std::sync::atomic::AtomicU64::new(
                medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
            ),
        }
    }

    /// The next version: strictly increasing for the life of the process.
    pub(super) fn next_stamp(&self) -> u64 {
        self.clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1)
    }
}

fn class_reply(class: DurabilityClass, declared: bool) -> KvNamespaceClass {
    KvNamespaceClass {
        class: class.as_str().to_string(),
        declared,
        loss_window_ms: class.declared_loss_window_ms(),
    }
}

fn decode_class(row: &[u8]) -> Result<DurabilityClass, String> {
    let name = std::str::from_utf8(row)
        .map_err(|_| "stored namespace declaration is not a class name".to_string())?;
    DurabilityClass::parse_declared(name).map_err(|e| e.to_string())
}

/// Parse the class name a declaration request carries; an unknown name is refused,
/// never defaulted.
pub(super) fn parse_requested(name: &str) -> Result<DurabilityClass, String> {
    DurabilityClass::parse_declared(name)
        .map_err(|e| format!("{}: {e}", ServerErrorCode::DurabilityClassUnknown.as_str()))
}

/// Declare `requested` for `namespace` inside one write. The one place a declaration
/// row is written, so every change goes through [`DurabilityClass::admit_redeclaration`].
fn declare_in(
    rows: &mut dyn RowAccess,
    namespace: &str,
    requested: DurabilityClass,
) -> Result<KvNamespaceClass, String> {
    let stored = match rows.row(DECLARATION_NAMESPACE, namespace)? {
        Some(row) => Some(decode_class(&row)?),
        // Rows written before any declaration were `sync`: declaring a weaker class
        // over them is a downgrade of data that was acknowledged as durable.
        None if rows.has_rows(namespace)? => Some(DurabilityClass::Sync),
        None => None,
    };
    let class = match stored {
        Some(current) => current.admit_redeclaration(requested).map_err(|mismatch| {
            format!(
                "{}: {mismatch}",
                ServerErrorCode::DurabilityClassMismatch.as_str()
            )
        })?,
        None => requested,
    };
    rows.store(
        DECLARATION_NAMESPACE,
        namespace,
        Some(class.as_str().as_bytes()),
    )?;
    Ok(class_reply(class, true))
}

impl KvStore {
    /// The class `namespace` declared, or `None` when it never declared one.
    pub(crate) fn declared_class(
        &self,
        namespace: &str,
    ) -> Result<Option<DurabilityClass>, String> {
        self.get(DECLARATION_NAMESPACE, namespace)?
            .map(|row| decode_class(&row))
            .transpose()
    }

    /// `KvNamespaceDeclaration`: the declared class, or `sync` with `declared: false`.
    pub(crate) fn namespace_class(&self, namespace: &str) -> Result<KvNamespaceClass, String> {
        Ok(match self.declared_class(namespace)? {
            Some(class) => class_reply(class, true),
            None => class_reply(DurabilityClass::default(), false),
        })
    }

    /// `KvDeclareNamespace`: store the declaration as one admitted MutationBatch.
    pub(super) fn declare_class(
        &self,
        namespace: &str,
        requested: DurabilityClass,
        write: &WireWrite<'_>,
    ) -> Result<KvNamespaceClass, String> {
        validate_key(namespace, "")?;
        match &self.backend {
            Backend::Redb(store) => store.admit_batch(write.batch, write.at_ms, |owner_write| {
                let mut table = owner_write.open_table(KV)?;
                declare_in(&mut table, namespace, requested)
            }),
            Backend::Memory(m) => declare_in(&mut *m.lock(), namespace, requested),
        }
    }

    /// The in-process rows `namespace` is served from when it is declared `ephemeral`.
    /// `None` for every durable namespace, and for the in-memory backend, whose rows
    /// are already process-local.
    pub(super) fn ephemeral_rows(&self, namespace: &str) -> Result<Option<&EphemeralRows>, String> {
        if !matches!(self.backend, Backend::Redb(_)) {
            return Ok(None);
        }
        Ok(
            (self.declared_class(namespace)? == Some(DurabilityClass::Ephemeral))
                .then_some(&self.ephemeral),
        )
    }

    /// Declare `namespace`'s class in-process, for the embedded profile. It is the same
    /// declaration row and the same [`DurabilityClass::admit_redeclaration`] rule as the
    /// served `KvDeclareNamespace`, written as one ledgered maintenance mutation.
    pub fn declare_namespace(
        &self,
        namespace: &str,
        requested: DurabilityClass,
    ) -> Result<KvNamespaceClass, String> {
        validate_key(namespace, "")?;
        match &self.backend {
            Backend::Redb(store) => store.maintain(
                "kv_declare",
                &row_subject(DECLARATION_NAMESPACE, namespace),
                |owner_write| {
                    let mut table = owner_write.open_table(KV)?;
                    declare_in(&mut table, namespace, requested).map(|class| (class, true))
                },
            ),
            Backend::Memory(m) => declare_in(&mut *m.lock(), namespace, requested),
        }
    }

    /// Store a value honouring the namespace's declared class: an `ephemeral` namespace
    /// lives in process memory, every other namespace takes the durable [`Self::put`].
    pub fn put_in_class(&self, namespace: &str, key: &str, value: Vec<u8>) -> Result<(), String> {
        let Some(ephemeral) = self.ephemeral_rows(namespace)? else {
            return self.put(namespace, key, value);
        };
        validate_key(namespace, key)?;
        validate_value(&value)?;
        ephemeral
            .rows
            .lock()
            .insert((namespace.to_string(), key.to_string()), value);
        Ok(())
    }

    /// Fetch a value honouring the namespace's declared class.
    pub fn get_in_class(&self, namespace: &str, key: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(ephemeral) = self.ephemeral_rows(namespace)? else {
            return self.get(namespace, key);
        };
        validate_key(namespace, key)?;
        Ok(ephemeral
            .rows
            .lock()
            .get(&(namespace.to_string(), key.to_string()))
            .cloned())
    }
}

#[cfg(test)]
mod reserved_namespace_tests {
    use super::super::dispatch_handlers::refuse_reserved_namespace;
    use super::*;
    use medulla_mutation_types::pg_replication::REPLICATION_SLOT_NAMESPACE;

    // spec: MEDULLA-DURABLE-KERNEL-R035.7.1
    #[test]
    fn served_writes_to_the_slot_namespace_refuse() {
        let namespace = REPLICATION_SLOT_NAMESPACE.to_string();
        for method in [
            Method::KvPut {
                namespace: namespace.clone(),
                key: "tenant-a/slot".to_string(),
                value: b"row".to_vec(),
                shared: false,
            },
            Method::KvCas {
                namespace: namespace.clone(),
                key: "tenant-a/slot".to_string(),
                expected_version: 0,
                new: Some(b"row".to_vec()),
                shared: true,
            },
            Method::KvDelete {
                namespace: namespace.clone(),
                key: "tenant-a/slot".to_string(),
                shared: false,
            },
            Method::KvDeclareNamespace {
                namespace: namespace.clone(),
                class: "sync".to_string(),
                shared: false,
            },
        ] {
            let refusal = refuse_reserved_namespace(&method, "agent-a").unwrap_err();
            assert!(refusal.starts_with("INVALID_ARGUMENT"), "{refusal}");
        }
        // Any other namespace is served, and a read of the reserved name is harmless.
        assert!(refuse_reserved_namespace(
            &Method::KvPut {
                namespace: "orders".to_string(),
                key: "a".to_string(),
                value: Vec::new(),
                shared: false,
            },
            "agent-a"
        )
        .is_ok());
        assert!(refuse_reserved_namespace(
            &Method::KvGet {
                namespace: namespace.clone(),
                key: "a".to_string(),
                shared: false,
            },
            "agent-a"
        )
        .is_ok());
        // The replication connection's fixed service identity alone may write the namespace.
        let write = Method::KvCas {
            namespace,
            key: "tenant-a/slot".to_string(),
            expected_version: 0,
            new: Some(b"row".to_vec()),
            shared: true,
        };
        assert!(refuse_reserved_namespace(
            &write,
            medulla_identity_core_types::identity::REPLICATION_SLOT_AGENT
        )
        .is_ok());
        assert!(refuse_reserved_namespace(&write, "service:other").is_err());
    }
}
