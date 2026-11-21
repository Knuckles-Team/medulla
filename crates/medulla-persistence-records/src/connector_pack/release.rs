//! MEDULLA-TYPED-PACKS-R107.3: the active release summary and the one pending
//! breaking release of a connector pack.
//!
//! Both live on the connector's head row. The import commit writes the active
//! summary with the head it moves. A breaking release never moves the head: it
//! is stored here as the head's pending release, by the one write
//! [`AgentLibraryStore::hold_connector_pack_release`], and the next import
//! commit clears it.

use medulla_agent_types::agent_library::AgentLibraryMutationContext;
use medulla_catalog_types::connector_pack::contract_version::{PackRelease, PendingPackRelease};
use medulla_catalog_types::connector_pack::PackHeadRef;
use medulla_mutation_types::mutation_scope::DurabilityDomain;
use medulla_mutation_types::mutation_scope::MutationSurface;
use medulla_protocol_types::mutation_batch::MutationOperation;
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::{Digest256, ResourceId};
use redb::ReadableTable;

use super::admitted::{PackWriteEffects, PackWriteIdentity};
use super::ConnectorPackHeadRow;
use crate::agent_library::AgentLibraryStore;

const RELEASE_HOLD_RESULT_SCHEMA_ID: &str = "connector-pack-release-hold-result.v1";
const RELEASE_ACTIVATE_RESULT_SCHEMA_ID: &str = "connector-pack-release-activate-result.v1";

impl AgentLibraryStore {
    /// The release summary the connector's current head activated, read under
    /// the verified tenant's key. A connector with no head, or a head written
    /// before summaries existed, has none: the next import has no baseline.
    pub fn connector_pack_release(
        &self,
        tenant_id: &str,
        connector: &ResourceId,
    ) -> Result<Option<PackRelease>, String> {
        Ok(self
            .connector_pack_head_row(tenant_id, connector)?
            .and_then(|head| head.release))
    }

    /// The breaking release held behind a change request, if any.
    pub fn connector_pack_pending_release(
        &self,
        tenant_id: &str,
        connector: &ResourceId,
    ) -> Result<Option<PendingPackRelease>, String> {
        Ok(self
            .connector_pack_head_row(tenant_id, connector)?
            .and_then(|head| head.pending_release))
    }

    fn connector_pack_head_row(
        &self,
        tenant_id: &str,
        connector: &ResourceId,
    ) -> Result<Option<ConnectorPackHeadRow>, String> {
        medulla_agent_types::agent_library::validate_key(tenant_id, connector.as_str())?;
        let read = self.read()?;
        let heads = read.open_owner_table(medulla_storage::CONNECTOR_PACK_HEADS)?;
        let Some(row) = heads
            .get((tenant_id, connector.as_str()))
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        super::undecodable::decode_stored(row.value(), "connector pack head").map(Some)
    }

    /// Store `pending` on the connector's head row in one admitted write. The
    /// write is a compare-and-set on the head: a head that moved since the diff
    /// read it refuses `PACK_HEAD_CONFLICT`, and nothing but the pending field
    /// of the row changes, so the active release keeps serving.
    pub fn hold_connector_pack_release(
        &self,
        context: AgentLibraryMutationContext,
        connector: &ResourceId,
        expected_head: &PackHeadRef,
        pending: PendingPackRelease,
    ) -> Result<PendingPackRelease, String> {
        medulla_agent_types::agent_library::validate_key(&context.tenant_id, connector.as_str())?;
        medulla_catalog_types::connector_pack::validate_connector(connector)?;
        pending.validate().map_err(|error| error.to_string())?;
        let discriminator = Digest256::framed(
            b"eg/connector-pack-release-hold/v1",
            &[
                connector.as_str().as_bytes(),
                pending.pack_digest.to_hex().as_bytes(),
            ],
        )
        .map(Digest256::to_hex)?;
        let identity = PackWriteIdentity {
            purpose: "connector-pack:release-hold",
            kind: "connector-pack-release-hold",
            slug: "connector-pack-release-hold",
            subject: connector.as_str(),
            revision: expected_head.binding_revision,
            discriminator: Some(&discriminator),
            result_schema: RELEASE_HOLD_RESULT_SCHEMA_ID,
            noun: "connector pack release hold",
        };
        let effects = PackWriteEffects {
            operations: vec![MutationOperation {
                ordinal: 0,
                surface: MutationSurface::Lifecycle,
                domain: DurabilityDomain::ControlPlane,
                method: Method::ApplyMutation {
                    event_type: "connector_pack_release_hold".to_string(),
                    query: format!("{}:{discriminator}", connector.as_str()),
                },
            }],
            outbox: Vec::new(),
        };
        let tenant = context.tenant_id.clone();
        self.commit_pack_write(&context, identity, effects, |write, _staged| {
            let mut heads = write.open_table(medulla_storage::CONNECTOR_PACK_HEADS)?;
            let current = heads
                .get((tenant.as_str(), connector.as_str()))
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "PACK_HEAD_CONFLICT: connector pack head is absent".to_string())?;
            let mut head: ConnectorPackHeadRow =
                super::undecodable::decode_stored(current.value(), "connector pack head")?;
            drop(current);
            if head.head.binding_revision != expected_head.binding_revision
                || head.head.pack_digest != expected_head.pack_digest
            {
                return Err("PACK_HEAD_CONFLICT: connector pack head changed".to_string());
            }
            head.pending_release = Some(pending.clone());
            let bytes = medulla_storage::encode_bounded(&head, "connector pack head")?;
            heads
                .insert((tenant.as_str(), connector.as_str()), bytes.as_slice())
                .map_err(|error| error.to_string())?;
            Ok(pending.clone())
        })
    }
    /// Make the held release the connector's active one, in one admitted write
    /// (MEDULLA-TYPED-PACKS-R107.6). The write is a compare-and-set on the held
    /// release: it refuses `PACK_RELEASE_NOT_HELD` when no release is held or the held
    /// one's change request proposes another digest than `proposed_digest`. The
    /// caller decided the activation through the release machine and passes the
    /// activated summary; the held slot is cleared and nothing else of the row moves.
    pub fn activate_connector_pack_release(
        &self,
        context: AgentLibraryMutationContext,
        connector: &ResourceId,
        proposed_digest: &str,
        activated: PackRelease,
    ) -> Result<PackRelease, String> {
        medulla_agent_types::agent_library::validate_key(&context.tenant_id, connector.as_str())?;
        medulla_catalog_types::connector_pack::validate_connector(connector)?;
        activated.validate().map_err(|error| error.to_string())?;
        let head = self
            .connector_pack_head_row(&context.tenant_id, connector)?
            .ok_or_else(|| "PACK_RELEASE_NOT_HELD: connector pack head is absent".to_string())?;
        let discriminator = Digest256::framed(
            b"eg/connector-pack-release-activate/v1",
            &[connector.as_str().as_bytes(), proposed_digest.as_bytes()],
        )
        .map(Digest256::to_hex)?;
        let identity = PackWriteIdentity {
            purpose: "connector-pack:release-activate",
            kind: "connector-pack-release-activate",
            slug: "connector-pack-release-activate",
            subject: connector.as_str(),
            revision: head.head.binding_revision,
            discriminator: Some(&discriminator),
            result_schema: RELEASE_ACTIVATE_RESULT_SCHEMA_ID,
            noun: "connector pack release activation",
        };
        let effects = PackWriteEffects {
            operations: vec![MutationOperation {
                ordinal: 0,
                surface: MutationSurface::Lifecycle,
                domain: DurabilityDomain::ControlPlane,
                method: Method::ApplyMutation {
                    event_type: "connector_pack_release_activate".to_string(),
                    query: format!("{}:{discriminator}", connector.as_str()),
                },
            }],
            outbox: Vec::new(),
        };
        let tenant = context.tenant_id.clone();
        self.commit_pack_write(&context, identity, effects, |write, _staged| {
            let mut heads = write.open_table(medulla_storage::CONNECTOR_PACK_HEADS)?;
            let current = heads
                .get((tenant.as_str(), connector.as_str()))
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "PACK_RELEASE_NOT_HELD: connector pack head is absent".to_string())?;
            let mut head: ConnectorPackHeadRow =
                super::undecodable::decode_stored(current.value(), "connector pack head")?;
            drop(current);
            let held = head
                .pending_release
                .as_ref()
                .filter(|held| held.change_request.proposed_digest == proposed_digest)
                .ok_or_else(|| {
                    "PACK_RELEASE_NOT_HELD: no release is held behind this change request"
                        .to_string()
                })?;
            if held.release.version.version != activated.version.version {
                return Err("PACK_RELEASE_NOT_HELD: the activated version is not the held one".to_string());
            }
            head.release = Some(activated.clone());
            head.pending_release = None;
            let bytes = medulla_storage::encode_bounded(&head, "connector pack head")?;
            heads
                .insert((tenant.as_str(), connector.as_str()), bytes.as_slice())
                .map_err(|error| error.to_string())?;
            Ok(activated.clone())
        })
    }
}
