//! The sealed coordinator-decision certificate of a cross-shard transaction
//! (MEDULLA-DECISION-ENGINE-R036.105.1).
//!
//! A certificate binds one transaction to the request it was decided for and
//! to the deadline it must be decided by. It lives in its own control-shard
//! table (`xshard_decision_certificate`), never under a reserved key of the
//! prepare or decision tables. This module is the stored type and its
//! sealing; the writer on the commit path is R036.105.2.
//!
//! Encoding: a version byte, then `txn_id`, `tenant`, `coordinator_scope` and
//! `request_digest` as a big-endian `u32` length and the UTF-8 bytes, then
//! `deadline_ms` and `decided_ms` as big-endian `u64`, then the SHA-256 of
//! everything before it. The result goes through the same [`DurableCrypto`]
//! that seals `xshard_prepare` values, so with a configured key the AEAD tag
//! also covers it. The trailing digest is the integrity check when no key is
//! configured: any altered byte fails `verify` either way.

use medulla_store::redb_store::shard::Shard;
use medulla_store::redb_store::DurableCrypto;
use sha2::{Digest, Sha256};

const FORMAT_VERSION: u8 = 1;
const DIGEST_BYTES: usize = 32;
const MAX_FIELD_BYTES: usize = 4096;

/// Every refusal code of this module.
pub const REFUSALS: &[&str] = &[
    "CERTIFICATE_SEAL_INVALID",
    "CERTIFICATE_EXPIRED",
    "CERTIFICATE_INTENT_CHANGED",
    "CERTIFICATE_LEGACY_ROW",
];

/// The request a cross-shard transaction was decided for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionCertificate {
    /// The transaction this certificate belongs to.
    pub txn_id: String,
    /// The tenant of the verified request.
    pub tenant: String,
    /// The scope of the coordinator that decides the transaction.
    pub coordinator_scope: String,
    /// SHA-256 over the request the transaction was decided for.
    pub request_digest: String,
    /// The time a COMMIT must be decided by (milliseconds since the Unix epoch).
    pub deadline_ms: u64,
    /// Set by the storage writer when the terminal outcome is written; zero
    /// while the transaction is pending.
    pub decided_ms: u64,
}

fn push_text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u32).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if self.bytes.len() < count {
            return Err(invalid("truncated"));
        }
        let (head, tail) = self.bytes.split_at(count);
        self.bytes = tail;
        Ok(head)
    }

    fn text(&mut self) -> Result<String, String> {
        let length =
            u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| invalid("length"))?) as usize;
        if length > MAX_FIELD_BYTES {
            return Err(invalid("field too long"));
        }
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| invalid("field is not UTF-8"))
    }

    fn number(&mut self) -> Result<u64, String> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| invalid("number"))?,
        ))
    }
}

fn invalid(why: &str) -> String {
    format!("CERTIFICATE_SEAL_INVALID: the decision certificate cannot be read ({why})")
}

impl DecisionCertificate {
    fn encode(&self) -> Vec<u8> {
        let mut out = vec![FORMAT_VERSION];
        push_text(&mut out, &self.txn_id);
        push_text(&mut out, &self.tenant);
        push_text(&mut out, &self.coordinator_scope);
        push_text(&mut out, &self.request_digest);
        out.extend_from_slice(&self.deadline_ms.to_be_bytes());
        out.extend_from_slice(&self.decided_ms.to_be_bytes());
        let digest = Sha256::digest(&out);
        out.extend_from_slice(&digest);
        out
    }

    /// The stored form of this certificate, sealed with the key that seals
    /// `xshard_prepare` values.
    pub fn seal(&self, crypto: DurableCrypto<'_>) -> Vec<u8> {
        crypto.seal(&self.encode()).into_owned()
    }

    /// Unseal and decode `sealed`; refuse one whose embedded transaction id or
    /// tenant is not the expected one.
    pub fn verify(
        sealed: &[u8],
        crypto: DurableCrypto<'_>,
        expected_txn_id: &str,
        expected_tenant: &str,
    ) -> Result<DecisionCertificate, String> {
        let certificate = Self::open(sealed, crypto)?;
        if certificate.txn_id != expected_txn_id || certificate.tenant != expected_tenant {
            return Err(invalid(
                "the certificate belongs to another transaction or tenant",
            ));
        }
        Ok(certificate)
    }

    /// Unseal and decode `sealed` without naming the transaction or tenant. Recovery
    /// uses it when it finishes a transaction knowing only its id; the integrity
    /// digest is still checked.
    pub fn open(sealed: &[u8], crypto: DurableCrypto<'_>) -> Result<DecisionCertificate, String> {
        let plain = crypto
            .unseal(sealed)
            .map_err(|cause| invalid(&format!("unseal failed: {cause}")))?;
        if plain.len() < 1 + DIGEST_BYTES {
            return Err(invalid("truncated"));
        }
        let (body, tag) = plain.split_at(plain.len() - DIGEST_BYTES);
        if Sha256::digest(body).as_slice() != tag {
            return Err(invalid("a field was altered"));
        }
        let mut cursor = Cursor { bytes: body };
        if cursor.take(1)?[0] != FORMAT_VERSION {
            return Err(invalid("unknown format version"));
        }
        let certificate = DecisionCertificate {
            txn_id: cursor.text()?,
            tenant: cursor.text()?,
            coordinator_scope: cursor.text()?,
            request_digest: cursor.text()?,
            deadline_ms: cursor.number()?,
            decided_ms: cursor.number()?,
        };
        if !cursor.bytes.is_empty() {
            return Err(invalid("trailing bytes"));
        }
        Ok(certificate)
    }
}

/// Durably store `certificate` for its transaction. An existing certificate
/// for the same `txn_id` with the same `request_digest` is an exact replay and
/// succeeds; one with a different `request_digest` refuses with
/// `CERTIFICATE_INTENT_CHANGED` and nothing is written.
pub fn store_certificate(
    shard: &Shard,
    certificate: &DecisionCertificate,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    if let Some(existing) =
        medulla_store_apply::redb_store::control::get_xshard_decision_certificate(
            shard,
            &certificate.txn_id,
        )?
    {
        let existing = DecisionCertificate::verify(
            &existing,
            crypto,
            &certificate.txn_id,
            &certificate.tenant,
        )?;
        if existing.request_digest != certificate.request_digest {
            return Err(format!(
                "CERTIFICATE_INTENT_CHANGED: transaction {} was decided for request {}, not {}",
                certificate.txn_id, existing.request_digest, certificate.request_digest
            ));
        }
        return Ok(());
    }
    medulla_store_apply::redb_store::control::put_xshard_decision_certificate(
        shard,
        &certificate.txn_id,
        &certificate.seal(crypto),
    )
}

/// The certified decision of one transaction. `Ok(None)` when nothing was
/// decided; `CERTIFICATE_LEGACY_ROW` when a terminal decision row exists with
/// no certificate row (never certified evidence); the verified certificate
/// otherwise.
pub fn certified_decision(
    shard: &Shard,
    txn_id: &str,
    tenant: &str,
    crypto: DurableCrypto<'_>,
) -> Result<Option<DecisionCertificate>, String> {
    use medulla_store_apply::redb_store::control::{
        get_xshard_decision, get_xshard_decision_certificate,
    };
    match get_xshard_decision_certificate(shard, txn_id)? {
        Some(sealed) => DecisionCertificate::verify(&sealed, crypto, txn_id, tenant).map(Some),
        None => match get_xshard_decision(shard, txn_id)? {
            None => Ok(None),
            Some(_) => Err(format!(
                "CERTIFICATE_LEGACY_ROW: transaction {txn_id} has a terminal decision row but no \
                 certificate, so it is not certified evidence"
            )),
        },
    }
}

/// Milliseconds since the Unix epoch on the storage writer's own clock.
pub fn writer_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn existing_certificate(
    shard: &Shard,
    crypto: DurableCrypto<'_>,
    txn_id: &str,
) -> Result<Option<DecisionCertificate>, String> {
    medulla_store_apply::redb_store::control::get_xshard_decision_certificate(shard, txn_id)?
        .map(|sealed| DecisionCertificate::open(&sealed, crypto))
        .transpose()
}

fn require_same_request(
    existing: &DecisionCertificate,
    wanted: &DecisionCertificate,
) -> Result<(), String> {
    if existing.tenant != wanted.tenant || existing.request_digest != wanted.request_digest {
        return Err(format!(
            "CERTIFICATE_INTENT_CHANGED: transaction {} was certified for request {}, not {}",
            wanted.txn_id, existing.request_digest, wanted.request_digest
        ));
    }
    Ok(())
}

/// Write the protocol-start marker together with the certificate request
/// (`decided_ms = 0`) in one control write. A transaction that already has a
/// certificate for the same request keeps it (and its `decided_ms`); another
/// request refuses with `CERTIFICATE_INTENT_CHANGED` and nothing is written.
pub fn write_pending(
    shard: &Shard,
    crypto: DurableCrypto<'_>,
    txn_id: &str,
    certificate: Option<&DecisionCertificate>,
) -> Result<(), String> {
    use medulla_store_apply::redb_store::control::put_xshard_recoverable_pending_certified;
    let Some(wanted) = certificate else {
        return put_xshard_recoverable_pending_certified(shard, txn_id, None);
    };
    match existing_certificate(shard, crypto, txn_id)? {
        Some(existing) => {
            require_same_request(&existing, wanted)?;
            put_xshard_recoverable_pending_certified(shard, txn_id, None)
        }
        None => {
            let pending = DecisionCertificate {
                decided_ms: 0,
                ..wanted.clone()
            };
            put_xshard_recoverable_pending_certified(shard, txn_id, Some(&pending.seal(crypto)))
        }
    }
}

/// Write the terminal outcome and the certificate in one control write, and
/// return the outcome that is durable afterwards.
///
/// The storage writer sets `decided_ms` from `now_ms`. `certificate` is the
/// request being decided; `None` adopts the certificate already stored for the
/// transaction (a recovery that knows only the id) or, with none stored, writes
/// an ordinary uncertified decision row. A decided transaction is a replay: the
/// original outcome is returned and nothing changes. A COMMIT at or after the
/// deadline refuses with `CERTIFICATE_EXPIRED` and writes nothing; an ABORT
/// never expires. A terminal row with no certificate refuses a certified write
/// with `CERTIFICATE_LEGACY_ROW`.
pub fn write_decision(
    shard: &Shard,
    crypto: DurableCrypto<'_>,
    txn_id: &str,
    commit: bool,
    retain_for_parent: bool,
    certificate: Option<&DecisionCertificate>,
    now_ms: u64,
) -> Result<bool, String> {
    use medulla_store_apply::redb_store::control::{
        get_xshard_decision, put_xshard_decision_certified,
    };
    let stored = existing_certificate(shard, crypto, txn_id)?;
    let decided = get_xshard_decision(shard, txn_id)?;
    let wanted = match (certificate, &stored) {
        (Some(wanted), Some(existing)) => {
            require_same_request(existing, wanted)?;
            Some(wanted.clone())
        }
        (Some(wanted), None) => {
            if decided.is_some() {
                return Err(format!(
                    "CERTIFICATE_LEGACY_ROW: transaction {txn_id} has a terminal decision row \
                     but no certificate, so it cannot be certified now"
                ));
            }
            Some(wanted.clone())
        }
        (None, Some(existing)) => Some(existing.clone()),
        (None, None) => None,
    };
    let Some(wanted) = wanted else {
        put_xshard_decision_certified(shard, txn_id, commit, retain_for_parent, None)?;
        return Ok(commit);
    };
    if let (Some(outcome), Some(existing)) = (decided, &stored) {
        if existing.decided_ms != 0 {
            return Ok(outcome);
        }
    }
    if commit && now_ms >= wanted.deadline_ms {
        return Err(format!(
            "CERTIFICATE_EXPIRED: transaction {txn_id} reached COMMIT at {now_ms} ms, at or \
             after its deadline {} ms",
            wanted.deadline_ms
        ));
    }
    let sealed = DecisionCertificate {
        decided_ms: now_ms,
        ..wanted
    }
    .seal(crypto);
    put_xshard_decision_certified(shard, txn_id, commit, retain_for_parent, Some(&sealed))?;
    Ok(commit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_store_apply::redb_store::control::{
        clear_xshard_decision, get_xshard_decision_certificate, put_xshard_decision,
    };

    fn certificate(request: &str) -> DecisionCertificate {
        DecisionCertificate {
            txn_id: "txn-1".into(),
            tenant: "tenant-a".into(),
            coordinator_scope: "scope-a".into(),
            request_digest: request.into(),
            deadline_ms: 9_000,
            decided_ms: 4_000,
        }
    }

    fn open(tag: &str) -> (std::path::PathBuf, Shard) {
        let path = std::env::temp_dir().join(format!(
            "medulla-xshard-certificate-{tag}-{}-{}.redb",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let shard = Shard::open(&path).unwrap();
        (path, shard)
    }

    // spec: MEDULLA-DECISION-ENGINE-R036.105.1
    #[test]
    fn certificate_seals_verifies_and_rejects_tamper() {
        let crypto = DurableCrypto::none();
        let original = certificate("request-1");
        let sealed = original.seal(crypto);
        assert_eq!(
            DecisionCertificate::verify(&sealed, crypto, "txn-1", "tenant-a").unwrap(),
            original
        );
        // Flipping any one byte (every field and the seal) is refused.
        for index in 0..sealed.len() {
            let mut altered = sealed.clone();
            altered[index] ^= 0x01;
            let error =
                DecisionCertificate::verify(&altered, crypto, "txn-1", "tenant-a").unwrap_err();
            assert!(
                error.starts_with("CERTIFICATE_SEAL_INVALID"),
                "{index}: {error}"
            );
        }
        // A certificate for another tenant or transaction refuses.
        let error = DecisionCertificate::verify(&sealed, crypto, "txn-1", "tenant-b").unwrap_err();
        assert!(error.starts_with("CERTIFICATE_SEAL_INVALID"), "{error}");
        let error = DecisionCertificate::verify(&sealed, crypto, "txn-2", "tenant-a").unwrap_err();
        assert!(error.starts_with("CERTIFICATE_SEAL_INVALID"), "{error}");

        let (path, shard) = open("store");
        store_certificate(&shard, &original, crypto).unwrap();
        // An exact replay succeeds.
        store_certificate(&shard, &original, crypto).unwrap();
        // The same transaction with another request digest refuses and writes nothing.
        let error = store_certificate(&shard, &certificate("request-2"), crypto).unwrap_err();
        assert!(error.starts_with("CERTIFICATE_INTENT_CHANGED"), "{error}");
        assert_eq!(
            certified_decision(&shard, "txn-1", "tenant-a", crypto)
                .unwrap()
                .unwrap()
                .request_digest,
            "request-1"
        );
        // A certificate stored for another tenant refuses on read.
        assert!(certified_decision(&shard, "txn-1", "tenant-b", crypto).is_err());

        // Reopen: the sealed row survives.
        drop(shard);
        let shard = Shard::open(&path).unwrap();
        assert!(get_xshard_decision_certificate(&shard, "txn-1")
            .unwrap()
            .is_some());
        assert_eq!(
            certified_decision(&shard, "txn-1", "tenant-a", crypto)
                .unwrap()
                .unwrap(),
            original
        );

        // A terminal decision row with no certificate is legacy, never certified.
        put_xshard_decision(&shard, "txn-legacy", true, false).unwrap();
        let error = certified_decision(&shard, "txn-legacy", "tenant-a", crypto).unwrap_err();
        assert!(error.starts_with("CERTIFICATE_LEGACY_ROW"), "{error}");
        assert!(certified_decision(&shard, "txn-none", "tenant-a", crypto)
            .unwrap()
            .is_none());

        // Cleanup removes the certificate with the decision.
        put_xshard_decision(&shard, "txn-1", true, false).unwrap();
        clear_xshard_decision(&shard, "txn-1").unwrap();
        assert!(get_xshard_decision_certificate(&shard, "txn-1")
            .unwrap()
            .is_none());
        drop(shard);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn every_refusal_code_is_registered() {
        for code in [
            "CERTIFICATE_SEAL_INVALID",
            "CERTIFICATE_EXPIRED",
            "CERTIFICATE_INTENT_CHANGED",
            "CERTIFICATE_LEGACY_ROW",
        ] {
            assert!(REFUSALS.contains(&code));
        }
    }
}
