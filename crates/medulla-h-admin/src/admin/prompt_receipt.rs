//! `AgentComponent.verify_render`: check a prompt render receipt
//! (MEDULLA-TYPED-PACKS-R002.13).
//!
//! The check recomputes the SHA-256 of the framed pins with the one shared
//! [`receipt_digest`] the renderer issued the receipt with, then reads the live
//! head and the verified scope. It writes nothing.

use medulla_agent_types::agent_component::{
    receipt_digest, AgentComponentEntry, PromptBlueprintIdentity, PromptReceiptVerdict,
    PromptRenderReceipt, PROMPT_RECEIPT_INVALID,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;

fn invalid(why: &str) -> String {
    format!("{PROMPT_RECEIPT_INVALID}: {why}")
}

/// The pure half of the check: the receipt scope is the verified tenant and
/// the digest equals the one recomputed from the framed pins.
fn check_binding(tenant: &str, receipt: &PromptRenderReceipt) -> Result<(), String> {
    let identity = &receipt.identity;
    if receipt.scope != tenant || identity.tenant_id != receipt.scope {
        return Err(invalid("the receipt scope is not the verified tenant"));
    }
    let recomputed = receipt_digest(identity, &receipt.variables_digest, &receipt.scope);
    if recomputed != receipt.receipt_digest {
        return Err(invalid("the receipt digest does not match its pins"));
    }
    Ok(())
}

/// The live head must be the revision the receipt names, pins included.
fn check_head(
    head: Option<AgentComponentEntry>,
    identity: &PromptBlueprintIdentity,
) -> Result<(), String> {
    let head = head.ok_or_else(|| invalid("the component has no published revision"))?;
    if PromptBlueprintIdentity::of_entry(&head).as_ref() != Some(identity) {
        return Err(invalid(
            "the live head is not the revision the receipt names",
        ));
    }
    Ok(())
}

/// Verify one receipt: the scope is the verified tenant, the digest matches the
/// framed pins, and the live head of the component is the revision the receipt
/// names. Any mismatch refuses `PROMPT_RECEIPT_INVALID`.
pub(crate) fn verify_receipt(
    store: &AgentLibraryStore,
    verified: &VerifiedRequestContext,
    receipt: &PromptRenderReceipt,
) -> Result<PromptReceiptVerdict, String> {
    check_binding(verified.tenant(), receipt)?;
    let identity = &receipt.identity;
    let head = store.current_component(&identity.tenant_id, &identity.component_id)?;
    check_head(head, identity)?;
    Ok(PromptReceiptVerdict { ok: true })
}

/// The `VerifyRender` arm of `handle_agent_component`.
pub(crate) fn serve_verify_render(
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    receipt: &PromptRenderReceipt,
) -> Response {
    match verify_receipt(store, verified, receipt) {
        Ok(verdict) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentVerifyRender,
            >(&verdict),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_component::ComponentProvenance;

    fn receipt() -> PromptRenderReceipt {
        let identity = PromptBlueprintIdentity {
            tenant_id: "tenant-a".to_string(),
            component_id: "prompt:a".to_string(),
            entry_revision: 2,
            schema_revision: 1,
            body_sha256: "a".repeat(64),
            provenance: ComponentProvenance::Native,
        };
        let variables_digest = "b".repeat(64);
        PromptRenderReceipt {
            receipt_digest: receipt_digest(&identity, &variables_digest, "tenant-a"),
            identity,
            variables_digest,
            scope: "tenant-a".to_string(),
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R002.13
    #[test]
    fn receipt_verification() {
        let good = receipt();
        assert_eq!(check_binding("tenant-a", &good), Ok(()));

        // Another verified scope, a changed pin and a changed variables digest all refuse.
        assert!(check_binding("tenant-b", &good)
            .unwrap_err()
            .starts_with(PROMPT_RECEIPT_INVALID));
        let mut pin = good.clone();
        pin.identity.entry_revision = 3;
        assert!(check_binding("tenant-a", &pin)
            .unwrap_err()
            .starts_with(PROMPT_RECEIPT_INVALID));
        let mut variables = good.clone();
        variables.variables_digest = "c".repeat(64);
        assert!(check_binding("tenant-a", &variables)
            .unwrap_err()
            .starts_with(PROMPT_RECEIPT_INVALID));

        // A head that is absent or names another revision refuses.
        assert!(check_head(None, &good.identity)
            .unwrap_err()
            .starts_with(PROMPT_RECEIPT_INVALID));
    }
}
