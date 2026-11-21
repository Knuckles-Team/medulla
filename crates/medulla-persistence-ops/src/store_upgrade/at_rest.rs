//! The at-rest key source of an offline store upgrade (MEDULLA-PLACEMENT-R781.4).
//!
//! `store-upgrade` resolves the same configured at-rest key the engine opens its stores with
//! (`ValueCipher::from_env_checked`) and hands the storage kernel a [`StoredRowCipher`] whose
//! seal and open go through [`DurableCrypto`], the one row sealing path of the engine. Without a
//! configured key no cipher is supplied and sealed rows are refused by name.

use std::sync::Arc;

use medulla_storage::StoredRowCipher;

/// The configured at-rest cipher as a storage-kernel row cipher, or `None` when the deployment
/// has no at-rest key. Malformed key metadata fails the command.
pub(super) fn row_cipher() -> Result<Option<Arc<dyn StoredRowCipher>>, String> {
    #[cfg(feature = "security")]
    {
        Ok(medulla_kernel::crypto::ValueCipher::from_env_checked()?
            .map(|cipher| Arc::new(EngineRowCipher(cipher)) as Arc<dyn StoredRowCipher>))
    }
    #[cfg(not(feature = "security"))]
    {
        Ok(None)
    }
}

#[cfg(feature = "security")]
struct EngineRowCipher(medulla_kernel::crypto::ValueCipher);

#[cfg(feature = "security")]
impl StoredRowCipher for EngineRowCipher {
    fn unseal(&self, stored: &[u8]) -> Result<Vec<u8>, String> {
        medulla_store::redb_store::DurableCrypto::new(Some(&self.0)).unseal(stored)
    }

    fn seal(&self, plaintext: &[u8]) -> Vec<u8> {
        medulla_store::redb_store::DurableCrypto::new(Some(&self.0))
            .seal(plaintext)
            .into_owned()
    }
}
