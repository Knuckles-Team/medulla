//! Signed `ListMemorySnapshots` cursors (MEDULLA-DURABLE-KERNEL-R085.2).
//!
//! The body is a [`CursorFamily`] token over `<issued_at_s>:<last doc_id>`.
//! An HMAC-SHA256 tag under the server auth secret follows it, keyed over the
//! body and the list binding: tenant, graph incarnation, principal, policy
//! digest and filter digest. A cursor minted for any other binding, or edited
//! in any byte, is `MEMSYNC_CURSOR_INVALID`; one older than
//! `memory_sync.cursor_ttl_s` is `MEMSYNC_CURSOR_EXPIRED`.

use hmac::{Hmac, Mac};
use medulla_types::contract::Digest256;
use medulla_types::memory_sync::MemorySyncRefusal;
use medulla_types::tenant_cursor::CursorFamily;
use sha2::Sha256;

const FAMILY: CursorFamily = CursorFamily {
    domain: b"medulla/memory-snapshot-list/v1",
    max_bytes: 960,
    noun: "memory snapshot list",
};

/// Hex characters of the HMAC tag a cursor carries.
const MAC_HEX: usize = 32;

/// Everything one cursor is bound to.
pub(crate) struct CursorBinding<'a> {
    pub tenant: &'a str,
    pub graph_incarnation: &'a str,
    pub principal: &'a str,
    pub policy_digest: Digest256,
    pub prefix: &'a str,
    pub limit: u32,
}

impl CursorBinding<'_> {
    fn filter_digest(&self) -> Digest256 {
        let limit = self.limit.to_be_bytes();
        Digest256::framed(
            b"medulla/memory-snapshot-list-filter/v1",
            &[self.prefix.as_bytes(), &limit],
        )
        .unwrap_or_else(|_| Digest256::sha256(self.prefix.as_bytes()))
    }

    fn mac(&self, secret: &str, body: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .expect("HMAC-SHA256 accepts a key of any length");
        let policy = self.policy_digest.to_hex();
        let filter = self.filter_digest().to_hex();
        for part in [
            self.tenant,
            self.graph_incarnation,
            self.principal,
            policy.as_str(),
            filter.as_str(),
            body,
        ] {
            mac.update(&(part.len() as u64).to_be_bytes());
            mac.update(part.as_bytes());
        }
        hex::encode(mac.finalize().into_bytes())[..MAC_HEX].to_string()
    }
}

/// Mint the cursor that resumes after `last_doc_id`.
pub(crate) fn encode(
    secret: &str,
    binding: &CursorBinding<'_>,
    last_doc_id: &str,
    now_s: u64,
) -> String {
    let body = FAMILY.encode(binding.tenant, &format!("{now_s}:{last_doc_id}"));
    let mac = binding.mac(secret, &body);
    format!("{body}.{mac}")
}

/// The `doc_id` a cursor resumes after, or the refusal that names it.
pub(crate) fn decode(
    secret: &str,
    binding: &CursorBinding<'_>,
    cursor: &str,
    now_s: u64,
    ttl_s: u64,
) -> Result<String, MemorySyncRefusal> {
    let (body, mac) = cursor
        .rsplit_once('.')
        .ok_or(MemorySyncRefusal::CursorInvalid)?;
    let expected = binding.mac(secret, body);
    if !constant_time_eq(mac.as_bytes(), expected.as_bytes()) {
        return Err(MemorySyncRefusal::CursorInvalid);
    }
    let key = FAMILY
        .decode(binding.tenant, body, |key| key.contains(':'))
        .map_err(|_| MemorySyncRefusal::CursorInvalid)?;
    let (issued, last) = key
        .split_once(':')
        .ok_or(MemorySyncRefusal::CursorInvalid)?;
    let issued: u64 = issued
        .parse()
        .map_err(|_| MemorySyncRefusal::CursorInvalid)?;
    if now_s.saturating_sub(issued) > ttl_s {
        return Err(MemorySyncRefusal::CursorExpired);
    }
    Ok(last.to_string())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding<'a>(principal: &'a str, prefix: &'a str) -> CursorBinding<'a> {
        CursorBinding {
            tenant: "tenant-a",
            graph_incarnation: "memsync",
            principal,
            policy_digest: Digest256::sha256(b"policy"),
            prefix,
            limit: 2,
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R085.2
    #[test]
    fn cursor_is_bound_signed_and_expires() {
        let mine = binding("alice", "notes/");
        let cursor = encode("secret", &mine, "notes/b.md", 1_000);
        assert_eq!(
            decode("secret", &mine, &cursor, 1_100, 900),
            Ok("notes/b.md".to_string())
        );
        let flip = if cursor.ends_with('a') { "b" } else { "a" };
        let tampered = format!("{}{flip}", &cursor[..cursor.len() - 1]);
        let cases: &[(&CursorBinding<'_>, &str, &str, u64, MemorySyncRefusal)] = &[
            (
                &binding("bob", "notes/"),
                "secret",
                &cursor,
                1_100,
                MemorySyncRefusal::CursorInvalid,
            ),
            (
                &binding("alice", "other/"),
                "secret",
                &cursor,
                1_100,
                MemorySyncRefusal::CursorInvalid,
            ),
            (
                &mine,
                "rotated",
                &cursor,
                1_100,
                MemorySyncRefusal::CursorInvalid,
            ),
            (
                &mine,
                "secret",
                &tampered,
                1_100,
                MemorySyncRefusal::CursorInvalid,
            ),
            (
                &mine,
                "secret",
                "no-mac",
                1_100,
                MemorySyncRefusal::CursorInvalid,
            ),
            (
                &mine,
                "secret",
                &cursor,
                1_901,
                MemorySyncRefusal::CursorExpired,
            ),
        ];
        for (bound, secret, token, now, expected) in cases {
            assert_eq!(decode(secret, bound, token, *now, 900), Err(*expected));
        }
    }
}
