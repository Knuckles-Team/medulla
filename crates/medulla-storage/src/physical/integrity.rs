/// Existing canonical AEAD/integrity authority supplied by the composition
/// root. The storage kernel never implements or derives a second crypto key.
pub trait PrivatePayloadIntegrity: Send + Sync {
    fn authenticate(&self, sealed: &[u8], expected_plaintext_digest: &str) -> Result<(), String>;
}

pub(crate) fn authenticate_with(
    integrity: Option<&dyn PrivatePayloadIntegrity>,
    sealed: &[u8],
    digest: &str,
) -> Result<(), String> {
    integrity
        .ok_or_else(|| "private recovery integrity authority is unavailable".to_string())?
        .authenticate(sealed, digest)
        .map_err(|_| "private recovery payload failed canonical authentication".to_string())
}

/// The at-rest cipher an operator command supplies to an offline upgrade that must rewrite
/// sealed row values (MEDULLA-PLACEMENT-R781.4). It is the same key source the engine uses; the
/// storage kernel holds no key of its own. `unseal` fails closed for a row the cipher cannot open.
pub trait StoredRowCipher: Send + Sync {
    fn unseal(&self, stored: &[u8]) -> Result<Vec<u8>, String>;
    fn seal(&self, plaintext: &[u8]) -> Vec<u8>;
}
