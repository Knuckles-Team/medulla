//! Stored connector-pack rows that no longer decode.
//!
//! A head or member row written under an earlier row shape can stop decoding
//! under the current one. Reads refuse such a row with the registered
//! `PACK_RECORD_UNDECODABLE` code, naming the connector, the row and the
//! decode reason (MEDULLA-TYPED-PACKS-R041.1), never an unclassified
//! `INTERNAL`. A fresh import of the connector replaces the undecodable rows
//! in its own admitted batch and records the replacement as a receipt warning
//! (MEDULLA-TYPED-PACKS-R041.2): hard cutover, no reader for the old shape.

use redb::ReadableTable;
use serde::de::DeserializeOwned;

use medulla_agent_types::agent_library::AgentLibraryLifecycle;
use medulla_catalog_types::connector_pack::{
    ConnectorPackStatus, PackImportReceipt, PackMemberCounts, PackWarning, PackWarningCode,
};
use medulla_types::contract::{BoundedVec, ResourceId};

use super::admitted::PackOwnerWrite;
use super::{ConnectorPackBindingRow, ConnectorPackHeadRow, ConnectorPackMemberRow};
use crate::agent_library::AgentLibraryStore;

/// The registered refusal code of a stored pack row that does not decode.
pub const PACK_RECORD_UNDECODABLE: &str = "PACK_RECORD_UNDECODABLE";

/// How a read treats a stored head or member row that does not decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndecodableRows {
    /// Refuse with `PACK_RECORD_UNDECODABLE` (every served read).
    Refuse,
    /// Treat the row as absent: only the import that replaces it.
    Replace,
}

/// Whether `error` is the typed undecodable-row refusal.
pub fn is_undecodable(error: &str) -> bool {
    error.starts_with(PACK_RECORD_UNDECODABLE)
}

/// Decode one stored connector-pack row, refusing with the typed code that
/// names the row and the decode reason.
pub(crate) fn decode_stored<T: DeserializeOwned>(bytes: &[u8], what: &str) -> Result<T, String> {
    crate::agent_row::decode_with_reason(bytes).map_err(|reason| {
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
    pub fn connector_pack_status_with(
        &self,
        tenant_id: &str,
        connector: &ResourceId,
        mode: UndecodableRows,
    ) -> Result<ConnectorPackStatus, String> {
        medulla_agent_types::agent_library::validate_key(tenant_id, connector.as_str())?;
        let read = self.read()?;
        let heads = read.open_owner_table(medulla_storage::CONNECTOR_PACK_HEADS)?;
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
        let binding = self.connector_pack_binding(tenant_id, connector)?;
        let mut counts = PackMemberCounts::default();
        for member in self.connector_pack_members_with(tenant_id, connector, mode)? {
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
    pub fn connector_pack_members_with(
        &self,
        tenant_id: &str,
        connector: &ResourceId,
        mode: UndecodableRows,
    ) -> Result<Vec<ConnectorPackMemberRow>, String> {
        medulla_agent_types::agent_library::validate_key(tenant_id, connector.as_str())?;
        let read = self.read()?;
        let members = read.open_owner_table(medulla_storage::CONNECTOR_PACK_MEMBERS)?;
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

    /// The importer binding row alone; an undecodable binding always refuses,
    /// because import authority is never inferred from a missing row.
    pub(crate) fn connector_pack_binding(
        &self,
        tenant_id: &str,
        connector: &ResourceId,
    ) -> Result<Option<ConnectorPackBindingRow>, String> {
        let read = self.read()?;
        let bindings = read.open_owner_table(medulla_storage::CONNECTOR_PACK_BINDINGS)?;
        let row = bindings
            .get((tenant_id, connector.as_str()))
            .map_err(|error| error.to_string())?;
        row.map(|row| decode_pack_row(row.value(), "connector pack binding", connector.as_str()))
            .transpose()
    }
}

/// What one import replaced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Replaced {
    head: bool,
    members: usize,
}

/// Remove the connector's undecodable member rows inside the import write and
/// report them with an undecodable head (which the import then overwrites).
pub(super) fn replace_undecodable_rows(
    write: &PackOwnerWrite<'_>,
    tenant: &str,
    connector: &str,
) -> Result<Replaced, String> {
    let heads = write.open_table(medulla_storage::CONNECTOR_PACK_HEADS)?;
    let head = heads
        .get((tenant, connector))
        .map_err(|error| error.to_string())?
        .is_some_and(|row| {
            crate::agent_row::try_decode::<ConnectorPackHeadRow>(row.value()).is_err()
        });
    drop(heads);
    let mut members = write.open_table(medulla_storage::CONNECTOR_PACK_MEMBERS)?;
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
        if crate::agent_row::try_decode::<ConnectorPackMemberRow>(value.value()).is_err() {
            doomed.push(uri.to_string());
        }
    }
    for uri in &doomed {
        members
            .remove((tenant, connector, uri.as_str()))
            .map_err(|error| error.to_string())?;
    }
    Ok(Replaced {
        head,
        members: doomed.len(),
    })
}

/// Audit a replacement on the receipt the import commits and answers with.
pub(super) fn record_replacement(
    receipt: &mut PackImportReceipt,
    replaced: Replaced,
) -> Result<(), String> {
    if replaced == Replaced::default() {
        return Ok(());
    }
    let mut warnings = receipt.warnings.as_slice().to_vec();
    warnings.push(PackWarning {
        code: PackWarningCode::UndecodableRecordReplaced,
        uri: None,
        detail: format!(
            "replaced undecodable stored rows: head {}, members {}",
            replaced.head, replaced.members
        ),
    });
    receipt.warnings = BoundedVec::new(warnings)?;
    Ok(())
}

/// Plant raw bytes as a connector's stored head or member row, so tests can
/// reproduce a row written under an older shape. Never in service builds.
#[cfg(any(test, feature = "test-support"))]
impl AgentLibraryStore {
    pub fn connector_pack_plant_test_row(
        &self,
        context: &medulla_agent_types::agent_library::AgentLibraryMutationContext,
        connector: &ResourceId,
        member_uri: Option<&str>,
        bytes: &[u8],
    ) -> Result<(), String> {
        let identity = super::admitted::PackWriteIdentity {
            purpose: "connector-pack:bind",
            kind: "connector-pack-test-plant",
            slug: "connector-pack-test-plant",
            subject: connector.as_str(),
            revision: 0,
            discriminator: member_uri,
            result_schema: super::admin::CONNECTOR_PACK_BINDING_RESULT_SCHEMA_ID,
            noun: "planted connector pack row",
        };
        let effects = super::admitted::PackWriteEffects {
            operations: super::admin::admin_operation(
                "connector-pack-test-plant",
                connector,
                member_uri.unwrap_or(""),
            ),
            outbox: Vec::new(),
        };
        let tenant = context.tenant_id.as_str();
        self.commit_pack_write(context, identity, effects, |write, _| {
            match member_uri {
                Some(uri) => write
                    .open_table(medulla_storage::CONNECTOR_PACK_MEMBERS)?
                    .insert((tenant, connector.as_str(), uri), bytes)
                    .map(|_| ()),
                None => write
                    .open_table(medulla_storage::CONNECTOR_PACK_HEADS)?
                    .insert((tenant, connector.as_str()), bytes)
                    .map(|_| ()),
            }
            .map_err(|error| error.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_fixtures::{mutation_context, open_agent_store};

    /// A row in a shape the current codec does not know.
    fn foreign_shape() -> Vec<u8> {
        let row = std::collections::BTreeMap::from([("retired_shape_field", 1u8)]);
        rmp_serde::to_vec_named(&row).unwrap()
    }

    // spec: MEDULLA-TYPED-PACKS-R041.1
    #[test]
    fn an_undecodable_stored_pack_row_refuses_with_its_typed_code_and_reason() {
        let (_dir, store) = open_agent_store();
        let connector = ResourceId::new("webarchiver-mcp").unwrap();
        for (nonce, member_uri) in [(1, None), (2, Some("tool://webarchiver-mcp/fetch"))] {
            let context = mutation_context(
                &store,
                "tenant-a",
                &format!("plant-{nonce}"),
                nonce,
                0,
                "connector-pack:bind",
            );
            store
                .connector_pack_plant_test_row(&context, &connector, member_uri, &foreign_shape())
                .unwrap();
            let error = store
                .connector_pack_status("tenant-a", &connector)
                .unwrap_err();
            assert!(is_undecodable(&error), "{error}");
            assert!(error.contains("'webarchiver-mcp'"), "{error}");
            assert!(error.contains("does not decode: "), "{error}");
            assert!(
                medulla_protocol_types::error_registry::declared_error_code(
                    PACK_RECORD_UNDECODABLE
                ),
                "the refusal code must be registered"
            );
        }
        let replaced = store
            .connector_pack_status_with("tenant-a", &connector, UndecodableRows::Replace)
            .unwrap();
        assert!(replaced.head.is_none());
        assert_eq!(replaced.members, PackMemberCounts::default());
    }
}
