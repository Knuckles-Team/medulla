//! Wallet key trust record (MEDULLA-FINANCE-PRIMITIVES-R904).
//!
//! Medulla stores the fact that a wallet key is bound to an account, wallet and
//! network at a trust revision. The SDK verifies signatures against
//! `public_key`; Medulla decides nothing about trust. Writes go through the
//! existing `AddNode` gateway route (label [`WALLET_KEY_TRUST_LABEL`]) and are
//! shape-checked by `WalletKeyTrustShape` in `finance-v1.shapes.ttl`.

use serde::{Deserialize, Serialize};

/// The graph node label a wallet key trust record is written under.
pub const WALLET_KEY_TRUST_LABEL: &str = "WalletKeyTrust";

/// Byte length of `public_key` (an Ed25519-class 32-byte key).
pub const WALLET_PUBLIC_KEY_BYTES: usize = 32;

/// One wallet key trust record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WalletKeyTrust {
    pub tenant_id: String,
    pub account_id: String,
    pub wallet_id: String,
    pub network_id: String,
    pub key_id: String,
    /// Lower-case hex of exactly [`WALLET_PUBLIC_KEY_BYTES`] bytes.
    pub public_key: String,
    pub signer_ref: String,
    pub trust_revision: u64,
    pub valid_from: String,
    #[serde(default)]
    pub valid_until: Option<String>,
    #[serde(default)]
    pub revoked: bool,
}

/// Why a stored record is not a valid wallet key trust record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletKeyTrustInvalid {
    PublicKeyLength(usize),
    PublicKeyNotHex,
    EmptyField(&'static str),
}

impl WalletKeyTrust {
    /// The decoded 32-byte public key, or why it is not one.
    pub fn public_key_bytes(&self) -> Result<[u8; WALLET_PUBLIC_KEY_BYTES], WalletKeyTrustInvalid> {
        let bytes =
            hex::decode(&self.public_key).map_err(|_| WalletKeyTrustInvalid::PublicKeyNotHex)?;
        <[u8; WALLET_PUBLIC_KEY_BYTES]>::try_from(bytes.as_slice())
            .map_err(|_| WalletKeyTrustInvalid::PublicKeyLength(bytes.len()))
    }

    /// Check the identifying fields are present and the key is 32 bytes.
    pub fn validate(&self) -> Result<(), WalletKeyTrustInvalid> {
        for (name, value) in [
            ("tenant_id", &self.tenant_id),
            ("account_id", &self.account_id),
            ("wallet_id", &self.wallet_id),
            ("network_id", &self.network_id),
            ("key_id", &self.key_id),
            ("signer_ref", &self.signer_ref),
            ("valid_from", &self.valid_from),
        ] {
            if value.trim().is_empty() {
                return Err(WalletKeyTrustInvalid::EmptyField(name));
            }
        }
        self.public_key_bytes().map(|_| ())
    }

    /// Whether this record is for the `(account, wallet, network)` binding.
    pub fn binds(&self, account_id: &str, wallet_id: &str, network_id: &str) -> bool {
        self.account_id == account_id
            && self.wallet_id == wallet_id
            && self.network_id == network_id
    }
}

/// The current record of a binding among `records` for `tenant_id`: the valid
/// record with the highest `trust_revision`. Records of another tenant are
/// never candidates, so a cross-tenant read finds nothing.
pub fn current_record<'a>(
    records: impl IntoIterator<Item = &'a WalletKeyTrust>,
    tenant_id: &str,
    account_id: &str,
    wallet_id: &str,
    network_id: &str,
) -> Option<&'a WalletKeyTrust> {
    records
        .into_iter()
        .filter(|r| r.tenant_id == tenant_id && r.binds(account_id, wallet_id, network_id))
        .filter(|r| r.validate().is_ok())
        .max_by_key(|r| r.trust_revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(tenant: &str, revision: u64, revoked: bool) -> WalletKeyTrust {
        WalletKeyTrust {
            tenant_id: tenant.into(),
            account_id: "acct".into(),
            wallet_id: "w1".into(),
            network_id: "net".into(),
            key_id: format!("k{revision}"),
            public_key: "ab".repeat(32),
            signer_ref: "signer:1".into(),
            trust_revision: revision,
            valid_from: "2026-01-01T00:00:00Z".into(),
            valid_until: None,
            revoked,
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R904
    #[test]
    fn current_record_is_highest_revision_of_the_tenant_only() {
        let rs = [
            record("t1", 1, false),
            record("t1", 2, true),
            record("t2", 9, false),
        ];
        let cur = current_record(&rs, "t1", "acct", "w1", "net").unwrap();
        assert_eq!(cur.trust_revision, 2);
        assert!(cur.revoked);
        assert!(current_record(&rs, "t3", "acct", "w1", "net").is_none());
    }

    #[test]
    fn public_key_must_be_32_bytes() {
        let mut r = record("t1", 1, false);
        r.public_key = "ab".repeat(31);
        assert_eq!(
            r.validate(),
            Err(WalletKeyTrustInvalid::PublicKeyLength(31))
        );
        r.public_key = "zz".repeat(32);
        assert_eq!(r.validate(), Err(WalletKeyTrustInvalid::PublicKeyNotHex));
    }
}
