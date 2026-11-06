//! Stored connector-pack rows that no longer decode (BUG-008, 2.27.11 port).
//!
//! A head or member row written under an earlier row shape can stop decoding
//! under the current one. Reads refuse such a row with the registered
//! `PACK_RECORD_UNDECODABLE` code, naming the connector, the row and the
//! bounded decode reason, never an unclassified `INTERNAL`. A fresh import of
//! the connector replaces the undecodable rows in its own admitted batch: hard
//! cutover, no reader for the old shape.

use redb::ReadableTable;
use serde::de::DeserializeOwned;

use eg_types::agent_library::AgentLibraryLifecycle;
use eg_types::connector_pack::{ConnectorPackStatus, PackMemberCounts};
use eg_types::contract::ResourceId;

use super::admitted::PackOwnerWrite;
use super::{ConnectorPackBindingRow, ConnectorPackHeadRow, ConnectorPackMemberRow};
use crate::server::persistence::agent_library::AgentLibraryStore;

/// The registered refusal code of a stored pack row that does not decode.
pub(crate) const PACK_RECORD_UNDECODABLE: &str = "PACK_RECORD_UNDECODABLE";

/// How a read treats a stored head or member row that does not decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UndecodableRows {
    /// Refuse with `PACK_RECORD_UNDECODABLE` (every served read).
    Refuse,
    /// Treat the row as absent: only the import that replaces it.
    Replace,
}

/// Whether `error` is the typed undecodable-row refusal.
#[cfg(test)]
pub(crate) fn is_undecodable(error: &str) -> bool {
    error.starts_with(PACK_RECORD_UNDECODABLE)
}

/// Decode one stored connector-pack row, refusing with the typed code that
/// names the row and the decode reason.
pub(crate) fn decode_stored<T: DeserializeOwned>(bytes: &[u8], what: &str) -> Result<T, String> {
    crate::server::persistence::agent_row::decode_with_reason(bytes).map_err(|reason| {
        format!("{PACK_RECORD_UNDECODABLE}: stored {what} does not decode: {reason}")
    })
}

/// Decode one stored row of `connector`, naming the connector in a refusal.
pub(crate) fn decode_pack_row<T: DeserializeOwned>(
    bytes: &[u8],
    what: &str,
    connector: &str,
) -> Result<T, String> {
    decode_stored(bytes, &format!("{what} of connector '{connector}'"))
}

fn decode_or_skip<T: DeserializeOwned>(
    bytes: &[u8],
    what: &str,
    connector: &str,
    mode: UndecodableRows,
) -> Result<Option<T>, String> {
    match decode_pack_row(bytes, what, connector) {
        Ok(row) => Ok(Some(row)),
        Err(_) if mode == UndecodableRows::Replace => Ok(None),
        Err(error) => Err(error),
    }
}

impl AgentLibraryStore {
    /// Read one connector's catalog state, treating undecodable rows by `mode`.
    ///
    /// One read snapshot, keyed by `(tenant, connector)`: the head and binding
    /// are point gets and the member range is bounded to this connector, so the
    /// cost is this connector's member count, never the whole catalog.
    pub(crate) fn connector_pack_status_with(
        &self,
        tenant_id: &str,
        connector: &ResourceId,
        mode: UndecodableRows,
    ) -> Result<ConnectorPackStatus, String> {
        eg_types::agent_library::validate_key(tenant_id, connector.as_str())?;
        let read = self.read()?;
        let heads = read.open_owner_table(eg_storage::CONNECTOR_PACK_HEADS)?;
        let members = read.open_owner_table(eg_storage::CONNECTOR_PACK_MEMBERS)?;
        let bindings = read.open_owner_table(eg_storage::CONNECTOR_PACK_BINDINGS)?;
        let head = match heads
            .get((tenant_id, connector.as_str()))
            .map_err(|error| error.to_string())?
        {
            Some(row) => decode_or_skip::<ConnectorPackHeadRow>(
                row.value(),
                "connector pack head",
                connector.as_str(),
                mode,
            )?,
            None => None,
        };
        // An undecodable binding always refuses: import authority is never
        // inferred from a missing row.
        let binding = bindings
            .get((tenant_id, connector.as_str()))
            .map_err(|error| error.to_string())?
            .map(|row| {
                decode_pack_row::<ConnectorPackBindingRow>(
                    row.value(),
                    "connector pack binding",
                    connector.as_str(),
                )
            })
            .transpose()?;
        let mut counts = PackMemberCounts::default();
        for row in members
            .range((tenant_id, connector.as_str(), "")..)
            .map_err(|error| error.to_string())?
        {
            let (key, value) = row.map_err(|error| error.to_string())?;
            let (row_tenant, row_connector, _) = key.value();
            if row_tenant != tenant_id || row_connector != connector.as_str() {
                break;
            }
            let Some(member) = decode_or_skip::<ConnectorPackMemberRow>(
                value.value(),
                "connector pack member",
                connector.as_str(),
                mode,
            )?
            else {
                continue;
            };
            match member.lifecycle {
                AgentLibraryLifecycle::Published => counts.published += 1,
                AgentLibraryLifecycle::Withdrawn => counts.withdrawn += 1,
                AgentLibraryLifecycle::Retired => counts.retired += 1,
            }
        }
        super::status_from_rows(tenant_id, connector, head, binding, counts)
    }

    /// Current member rows of one connector in URI order, treating
    /// undecodable rows by `mode`.
    pub(crate) fn connector_pack_members_with(
        &self,
        tenant_id: &str,
        connector: &ResourceId,
        mode: UndecodableRows,
    ) -> Result<Vec<ConnectorPackMemberRow>, String> {
        eg_types::agent_library::validate_key(tenant_id, connector.as_str())?;
        let read = self.read()?;
        let members = read.open_owner_table(eg_storage::CONNECTOR_PACK_MEMBERS)?;
        let mut rows = Vec::new();
        for row in members
            .range((tenant_id, connector.as_str(), "")..)
            .map_err(|error| error.to_string())?
        {
            let (key, value) = row.map_err(|error| error.to_string())?;
            let (row_tenant, row_connector, _) = key.value();
            if row_tenant != tenant_id || row_connector != connector.as_str() {
                break;
            }
            rows.extend(decode_or_skip(
                value.value(),
                "connector pack member",
                connector.as_str(),
                mode,
            )?);
        }
        Ok(rows)
    }
}

/// What one import replaced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Replaced {
    pub(super) head: bool,
    pub(super) members: usize,
}

/// Remove the connector's undecodable member rows inside the import write and
/// report them with an undecodable head (which the import then overwrites).
pub(super) fn replace_undecodable_rows(
    write: &PackOwnerWrite<'_>,
    tenant: &str,
    connector: &str,
) -> Result<Replaced, String> {
    let heads = write.open_table(eg_storage::CONNECTOR_PACK_HEADS)?;
    let head = heads
        .get((tenant, connector))
        .map_err(|error| error.to_string())?
        .is_some_and(|row| {
            crate::server::persistence::agent_row::try_decode::<ConnectorPackHeadRow>(row.value())
                .is_err()
        });
    drop(heads);
    let mut members = write.open_table(eg_storage::CONNECTOR_PACK_MEMBERS)?;
    let mut doomed = Vec::new();
    for row in members
        .range((tenant, connector, "")..)
        .map_err(|error| error.to_string())?
    {
        let (key, value) = row.map_err(|error| error.to_string())?;
        let (row_tenant, row_connector, uri) = key.value();
        if row_tenant != tenant || row_connector != connector {
            break;
        }
        if crate::server::persistence::agent_row::try_decode::<ConnectorPackMemberRow>(
            value.value(),
        )
        .is_err()
        {
            doomed.push(uri.to_string());
        }
    }
    for uri in &doomed {
        members
            .remove((tenant, connector, uri.as_str()))
            .map_err(|error| error.to_string())?;
    }
    let replaced = Replaced {
        head,
        members: doomed.len(),
    };
    if replaced != Replaced::default() {
        tracing::warn!(
            code = PACK_RECORD_UNDECODABLE,
            connector,
            head = replaced.head,
            members = replaced.members,
            "connector pack import replaced undecodable stored rows"
        );
    }
    Ok(replaced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::persistence::agent_fixtures::{mutation_context, open_agent_store};

    const TENANT: &str = "tenant-a";

    /// A row in a shape the current codec does not know.
    fn foreign_shape() -> Vec<u8> {
        let row = std::collections::BTreeMap::from([("retired_shape_field", 1u8)]);
        rmp_serde::to_vec_named(&row).unwrap()
    }

    fn connector() -> ResourceId {
        ResourceId::new("webarchiver-mcp").unwrap()
    }

    /// Write raw bytes as the connector's head (`None`) or member row through
    /// the admitted pack write, reproducing a row of an older shape.
    fn plant(store: &AgentLibraryStore, nonce: u8, member_uri: Option<&str>, bytes: &[u8]) {
        let context = mutation_context(
            store,
            TENANT,
            &format!("plant-{nonce}"),
            nonce,
            0,
            "connector-pack:bind",
        );
        let connector = connector();
        let identity = super::super::admitted::PackWriteIdentity {
            purpose: "connector-pack:bind",
            kind: "connector-pack-test-plant",
            slug: "connector-pack-test-plant",
            subject: connector.as_str(),
            revision: 0,
            discriminator: member_uri,
            result_schema: super::super::admin::CONNECTOR_PACK_BINDING_RESULT_SCHEMA_ID,
            noun: "planted connector pack row",
        };
        let effects = super::super::admitted::PackWriteEffects {
            operations: super::super::admin::admin_operation(
                "connector-pack-test-plant",
                &connector,
                member_uri.unwrap_or(""),
            ),
            outbox: Vec::new(),
        };
        store
            .commit_pack_write(&context, identity, effects, |write, _| {
                match member_uri {
                    Some(uri) => write
                        .open_table(eg_storage::CONNECTOR_PACK_MEMBERS)?
                        .insert((TENANT, connector.as_str(), uri), bytes)
                        .map(|_| ()),
                    None => write
                        .open_table(eg_storage::CONNECTOR_PACK_HEADS)?
                        .insert((TENANT, connector.as_str()), bytes)
                        .map(|_| ()),
                }
                .map_err(|error| error.to_string())
            })
            .unwrap();
    }

    fn replace(store: &AgentLibraryStore, nonce: u8) -> Replaced {
        let context = mutation_context(
            store,
            TENANT,
            &format!("replace-{nonce}"),
            nonce,
            0,
            "connector-pack:bind",
        );
        let connector = connector();
        let identity = super::super::admitted::PackWriteIdentity {
            purpose: "connector-pack:bind",
            kind: "connector-pack-test-replace",
            slug: "connector-pack-test-replace",
            subject: connector.as_str(),
            revision: 0,
            discriminator: None,
            result_schema: super::super::admin::CONNECTOR_PACK_BINDING_RESULT_SCHEMA_ID,
            noun: "replaced connector pack rows",
        };
        let effects = super::super::admitted::PackWriteEffects {
            operations: super::super::admin::admin_operation(
                "connector-pack-test-replace",
                &connector,
                "",
            ),
            outbox: Vec::new(),
        };
        let mut replaced = Replaced::default();
        store
            .commit_pack_write(&context, identity, effects, |write, _| {
                replaced = replace_undecodable_rows(write, TENANT, connector.as_str())?;
                Ok(())
            })
            .unwrap();
        replaced
    }

    // BUG-008: a served read refuses with the typed, registered code, naming
    // the connector and the decode reason; never an unclassified INTERNAL.
    #[test]
    fn an_undecodable_stored_pack_row_refuses_with_its_typed_code_and_reason() {
        let (_dir, store) = open_agent_store();
        for (nonce, member_uri) in [(1, None), (2, Some("tool://webarchiver-mcp/fetch"))] {
            plant(&store, nonce, member_uri, &foreign_shape());
            let error = store.connector_pack_status(TENANT, &connector()).unwrap_err();
            assert!(is_undecodable(&error), "{error}");
            assert!(error.contains("'webarchiver-mcp'"), "{error}");
            assert!(error.contains("does not decode: "), "{error}");
        }
        assert!(
            eg_types::contract::declared_error_code(PACK_RECORD_UNDECODABLE),
            "the refusal code must be registered"
        );
        let replaced = store
            .connector_pack_status_with(TENANT, &connector(), UndecodableRows::Replace)
            .unwrap();
        assert!(replaced.head.is_none());
        assert_eq!(replaced.members, PackMemberCounts::default());
        // Import authority reads the binding alone, so the undecodable head
        // never blocks the import that replaces it.
        assert_eq!(
            store.connector_pack_importer(TENANT, &connector()).unwrap(),
            None
        );
    }

    // BUG-008: the import's admitted write removes undecodable members once,
    // and a served status then answers instead of looping on the row.
    #[test]
    fn the_import_write_replaces_undecodable_members_once() {
        let (_dir, store) = open_agent_store();
        plant(&store, 1, Some("tool://webarchiver-mcp/a"), &foreign_shape());
        plant(&store, 2, Some("tool://webarchiver-mcp/b"), &foreign_shape());
        assert!(store.connector_pack_status(TENANT, &connector()).is_err());
        assert_eq!(
            replace(&store, 3),
            Replaced {
                head: false,
                members: 2
            }
        );
        let status = store.connector_pack_status(TENANT, &connector()).unwrap();
        assert_eq!(status.members, PackMemberCounts::default());
        assert_eq!(replace(&store, 4), Replaced::default());
    }
}
