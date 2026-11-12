//! One test-only composition root for every kernel-owned store this crate opens.
//!
//! RF-RULING-004 puts the scope-grant proof authority in the composition root:
//! only it may decide that a principal is entitled to serve a given scope. The
//! crate's own tests have no such root, so they use the verifier here.
//!
//! It is not a permissive stub. It is constructed for ONE `OwnerLayout` and
//! checks that layout, the principal and the proof bytes, so a store opened for
//! the wrong layout or principal still fails closed under test.

use medulla_storage::{OwnerLayout, PhysicalStoreIdentity, ScopeGrantVerifier};

pub const TEST_PRINCIPAL: &str =
    "principal:sha256:d70d97fc35a6e2dfbef26a2bca76a96c6dd2c4142ae2a14850deaf61b478bba0";
pub const TEST_PROOF: &[u8] = b"medulla-core-test-scope-grant";

pub struct TestScopeVerifier {
    pub layout: OwnerLayout,
}

impl ScopeGrantVerifier for TestScopeVerifier {
    fn verify(
        &self,
        _physical: &PhysicalStoreIdentity,
        layout: OwnerLayout,
        identity: &medulla_mutation_types::mutation_scope::MutationScopeIdentity,
        principal: &str,
        proof: &[u8],
    ) -> Result<(), String> {
        if layout != self.layout
            || identity.tenant().as_str() != "native"
            || principal != TEST_PRINCIPAL
            || proof != TEST_PROOF
        {
            return Err("test scope authority rejected".to_string());
        }
        Ok(())
    }
}

/// Open a durable RBAC store under the test composition root.
pub fn open_rbac_store(
    dir: impl AsRef<std::path::Path>,
) -> Result<crate::rbac_persist::RbacStore, crate::rbac_persist::RbacPersistError> {
    crate::rbac_persist::RbacStore::open(
        dir,
        &TestScopeVerifier {
            layout: OwnerLayout::Rbac,
        },
        TEST_PRINCIPAL,
        TEST_PROOF,
    )
}
