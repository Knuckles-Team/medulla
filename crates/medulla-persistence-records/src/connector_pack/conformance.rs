//! MEDULLA-TYPED-PACKS-R807: reads and revokes a connector's conformance evidence.
//!
//! The evidence record lives on the connector's head row, so the import commit that
//! writes a head writes its evidence in the same admitted batch and a later head replaces
//! it: the record always covers the current immutable revision. The only other writer is
//! [`AgentLibraryStore::revoke_connector_conformance_evidence`], which applies the
//! `Revoke` event of `ConformanceEvidenceMachine` and stores the result.

use medulla_agent_types::agent_library::AgentLibraryMutationContext;
use medulla_catalog_types::connector_pack::conformance_evidence::{
    ConformanceEvidence, ConformanceEvidenceEvent,
};
use medulla_mutation_types::mutation_scope::DurabilityDomain;
use medulla_mutation_types::mutation_scope::MutationSurface;
use medulla_protocol_types::mutation_batch::MutationOperation;
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::{Digest256, ResourceId, ServerErrorCode};
use redb::ReadableTable;

use super::admitted::{PackWriteEffects, PackWriteIdentity};
use super::ConnectorPackHeadRow;
use crate::agent_library::AgentLibraryStore;

const CONFORMANCE_REVOKE_RESULT_SCHEMA_ID: &str = "connector-pack-conformance-revoke-result.v1";

fn absent(detail: &str) -> String {
    format!(
        "{}: {detail}",
        ServerErrorCode::ConformanceEvidenceAbsent.as_str()
    )
}

impl AgentLibraryStore {
    /// The connector's current conformance evidence at `now_ms`, or the typed
    /// `CONFORMANCE_EVIDENCE_ABSENT` refusal when none stands: the connector has no head, the
    /// head carries none, the record is revoked, or it is past its deadline. The row is read
    /// under the verified tenant's key, so another tenant's record is never reachable.
    pub fn connector_pack_conformance_evidence(
        &self,
        tenant_id: &str,
        connector: &ResourceId,
        now_ms: u64,
    ) -> Result<ConformanceEvidence, String> {
        medulla_agent_types::agent_library::validate_key(tenant_id, connector.as_str())?;
        let read = self.read()?;
        let heads = read.open_owner_table(medulla_storage::CONNECTOR_PACK_HEADS)?;
        let row = heads
            .get((tenant_id, connector.as_str()))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| absent("connector has no pack head"))?;
        let head: ConnectorPackHeadRow =
            super::undecodable::decode_stored(row.value(), "connector pack head")?;
        let evidence = head
            .conformance_evidence
            .ok_or_else(|| absent("the current pack head carries no evidence"))?;
        if !evidence.is_current(now_ms) {
            return Err(absent("the evidence is revoked or expired"));
        }
        Ok(evidence)
    }

    /// Revoke the connector's current conformance evidence in one admitted write. A revoked
    /// or missing record refuses `CONFORMANCE_EVIDENCE_ABSENT`; the typed transition refusal
    /// is not surfaced because a revoked record is, to every caller, absent.
    pub fn revoke_connector_conformance_evidence(
        &self,
        context: AgentLibraryMutationContext,
        connector: &ResourceId,
    ) -> Result<ConformanceEvidence, String> {
        medulla_agent_types::agent_library::validate_key(&context.tenant_id, connector.as_str())?;
        medulla_catalog_types::connector_pack::validate_connector(connector)?;
        let discriminator = Digest256::framed(
            b"eg/connector-pack-conformance-revoke/v1",
            &[connector.as_str().as_bytes()],
        )
        .map(Digest256::to_hex)?;
        let identity = PackWriteIdentity {
            purpose: "connector-pack:conformance-revoke",
            kind: "connector-pack-conformance-revoke",
            slug: "connector-pack-conformance-revoke",
            subject: connector.as_str(),
            revision: 0,
            discriminator: Some(&discriminator),
            result_schema: CONFORMANCE_REVOKE_RESULT_SCHEMA_ID,
            noun: "connector pack conformance revoke",
        };
        let effects = PackWriteEffects {
            operations: vec![MutationOperation {
                ordinal: 0,
                surface: MutationSurface::Lifecycle,
                domain: DurabilityDomain::ControlPlane,
                method: Method::ApplyMutation {
                    event_type: "connector_pack_conformance_revoke".to_string(),
                    query: format!("{}:{discriminator}", connector.as_str()),
                },
            }],
            outbox: Vec::new(),
        };
        let tenant = context.tenant_id.clone();
        let at_ms = context.created_at_ms;
        self.commit_pack_write(&context, identity, effects, |write, _staged| {
            let mut heads = write.open_table(medulla_storage::CONNECTOR_PACK_HEADS)?;
            let current = heads
                .get((tenant.as_str(), connector.as_str()))
                .map_err(|error| error.to_string())?
                .ok_or_else(|| absent("connector has no pack head"))?;
            let mut head: ConnectorPackHeadRow =
                super::undecodable::decode_stored(current.value(), "connector pack head")?;
            drop(current);
            let mut evidence = head
                .conformance_evidence
                .take()
                .ok_or_else(|| absent("the current pack head carries no evidence"))?;
            evidence
                .apply(ConformanceEvidenceEvent::Revoke, at_ms)
                .map_err(|_| absent("the evidence is already revoked"))?;
            head.conformance_evidence = Some(evidence.clone());
            let bytes = medulla_storage::encode_bounded(&head, "connector pack head")?;
            heads
                .insert((tenant.as_str(), connector.as_str()), bytes.as_slice())
                .map_err(|error| error.to_string())?;
            Ok(evidence)
        })
    }
}
