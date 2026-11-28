//! Stable machine identity (MEDULLA-MACHINE-R002).
//!
//! One pure function derives one IRI per machine from ordered keys: the same keys always give
//! the same IRI. Four key kinds exist. Hardware keys (system UUID, serial, management
//! controller ID) carry no scope. A hostname is scoped to the tenant that owns it. A
//! placeholder value that firmware reports for an unset field counts as absent.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The four kinds of identity key, in preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineKeyKind {
    /// The hardware system UUID.
    SystemUuid,
    /// The hardware serial number.
    SystemSerial,
    /// The hardware address of the out-of-band management controller.
    ManagementControllerId,
    /// The host name, scoped to a tenant.
    Hostname,
}

impl MachineKeyKind {
    /// Every kind in preference order.
    pub const ALL: [Self; 4] = [
        Self::SystemUuid,
        Self::SystemSerial,
        Self::ManagementControllerId,
        Self::Hostname,
    ];

    /// The tag of this kind in a key text and in an IRI: `uuid`, `serial`, `bmc` or `host`.
    pub fn tag(self) -> &'static str {
        match self {
            Self::SystemUuid => "uuid",
            Self::SystemSerial => "serial",
            Self::ManagementControllerId => "bmc",
            Self::Hostname => "host",
        }
    }

    /// The kind whose tag is `tag`.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.tag() == tag)
    }
}

/// One normalized identity key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MachineKey {
    /// The kind of the key.
    pub kind: MachineKeyKind,
    /// The normalized value.
    pub value: String,
}

/// Why a raw key was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineKeyRefusalReason {
    /// The value does not have the form of its kind.
    Malformed,
    /// The value is one that firmware reports for an unset field.
    Placeholder,
    /// The value is empty.
    Empty,
}

impl MachineKeyRefusalReason {
    /// The wire text of the reason.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Placeholder => "placeholder",
            Self::Empty => "empty",
        }
    }
}

/// A raw key that was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}:{}", kind.tag(), reason.as_str())]
pub struct MachineKeyRefused {
    /// The kind the caller named.
    pub kind: MachineKeyKind,
    /// Why the value was refused.
    pub reason: MachineKeyRefusalReason,
}

const PLACEHOLDER_UUIDS: [&str; 4] = [
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
    "03000200-0400-0500-0006-000700080009",
    "00020003-0004-0005-0006-000700080009",
];

const PLACEHOLDER_SERIALS: [&str; 8] = [
    "to be filled by o.e.m.",
    "default string",
    "none",
    "n/a",
    "not specified",
    "system serial number",
    "0",
    "123456789",
];

/// Normalize one raw key of `kind`, or refuse it.
pub fn normalize_key(kind: MachineKeyKind, raw: &str) -> Result<MachineKey, MachineKeyRefused> {
    let refuse = |reason| MachineKeyRefused { kind, reason };
    let value = match kind {
        MachineKeyKind::SystemUuid => normalize_uuid(raw),
        MachineKeyKind::SystemSerial => normalize_serial(raw),
        MachineKeyKind::ManagementControllerId => normalize_controller_id(raw),
        MachineKeyKind::Hostname => normalize_hostname(raw),
    }
    .map_err(refuse)?;
    Ok(MachineKey { kind, value })
}

fn normalize_uuid(raw: &str) -> Result<String, MachineKeyRefusalReason> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err(MachineKeyRefusalReason::Empty);
    }
    let groups: Vec<&str> = value.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    let hex = groups
        .iter()
        .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()));
    if lengths != [8, 4, 4, 4, 12] || !hex {
        return Err(MachineKeyRefusalReason::Malformed);
    }
    if PLACEHOLDER_UUIDS.contains(&value.as_str()) {
        return Err(MachineKeyRefusalReason::Placeholder);
    }
    Ok(value)
}

fn normalize_serial(raw: &str) -> Result<String, MachineKeyRefusalReason> {
    let value = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        return Err(MachineKeyRefusalReason::Empty);
    }
    if value.chars().any(char::is_control) {
        return Err(MachineKeyRefusalReason::Malformed);
    }
    let lower = value.to_ascii_lowercase();
    if PLACEHOLDER_SERIALS.contains(&lower.as_str()) {
        return Err(MachineKeyRefusalReason::Placeholder);
    }
    Ok(value.to_ascii_uppercase())
}

fn normalize_controller_id(raw: &str) -> Result<String, MachineKeyRefusalReason> {
    let text = raw.trim().to_ascii_lowercase();
    if text.is_empty() {
        return Err(MachineKeyRefusalReason::Empty);
    }
    let digits = controller_digits(&text).ok_or(MachineKeyRefusalReason::Malformed)?;
    let octets: Vec<&str> = (0..6).map(|index| &digits[index * 2..index * 2 + 2]).collect();
    if octets.iter().all(|octet| *octet == "00") || octets.iter().all(|octet| *octet == "ff") {
        return Err(MachineKeyRefusalReason::Placeholder);
    }
    Ok(octets.join(":"))
}

/// The twelve hex digits of a controller ID written with `:` or `-` or no separator.
fn controller_digits(text: &str) -> Option<String> {
    let separator = [':', '-'].into_iter().find(|sep| text.contains(*sep));
    let digits: String = match separator {
        Some(sep) => {
            let parts: Vec<&str> = text.split(sep).collect();
            if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
                return None;
            }
            parts.concat()
        }
        None => text.to_string(),
    };
    let valid = digits.len() == 12 && digits.bytes().all(|byte| byte.is_ascii_hexdigit());
    valid.then_some(digits)
}

fn normalize_hostname(raw: &str) -> Result<String, MachineKeyRefusalReason> {
    let lower = raw.trim().to_ascii_lowercase();
    let value = lower.strip_suffix('.').unwrap_or(&lower);
    if value.is_empty() {
        return Err(MachineKeyRefusalReason::Empty);
    }
    let label_ok = |label: &str| {
        !label.is_empty()
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    if value.len() > 253 || !value.split('.').all(label_ok) {
        return Err(MachineKeyRefusalReason::Malformed);
    }
    Ok(value.to_string())
}

/// The first valid key by kind preference: `SystemUuid`, `SystemSerial`,
/// `ManagementControllerId`, `Hostname`. A key is valid when it is already normalized and
/// not a placeholder; the candidate is returned in its normalized form.
pub fn select_key(candidates: &[MachineKey]) -> Option<MachineKey> {
    MachineKeyKind::ALL.into_iter().find_map(|kind| {
        candidates
            .iter()
            .filter(|candidate| candidate.kind == kind)
            .find_map(|candidate| normalize_key(kind, &candidate.value).ok())
    })
}

/// First 32 lower-case hex digits of the SHA-256 of `parts` joined by LF (`0x0A`).
fn digest_prefix(parts: &[&str]) -> String {
    let digest = Sha256::digest(parts.join("\n").as_bytes());
    hex::encode(digest)[..32].to_string()
}

/// The IRI of the canonical machine that `key` identifies:
/// `urn:machine:<tag>:<32 hex of sha256(tag LF scope LF value)>`. The scope is empty for a
/// hardware kind and the tenant id for a hostname.
pub fn machine_iri(tenant: &str, key: &MachineKey) -> String {
    let tag = key.kind.tag();
    let scope = match key.kind {
        MachineKeyKind::Hostname => tenant,
        _ => "",
    };
    format!(
        "urn:machine:{tag}:{}",
        digest_prefix(&[tag, scope, &key.value])
    )
}

/// The id of the anchor node of `key`:
/// `urn:machine-key:<tag>:<32 hex of sha256(tag LF value)>`. It has no tenant scope, because
/// the tenant graph isolates the anchor.
pub fn machine_key_iri(key: &MachineKey) -> String {
    let tag = key.kind.tag();
    format!("urn:machine-key:{tag}:{}", digest_prefix(&[tag, &key.value]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const VECTORS: &str = include_str!("../../tests/vectors/machine_iri.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).expect("the vector file parses")
    }

    fn kind_of(tag: &Value) -> MachineKeyKind {
        MachineKeyKind::from_tag(tag.as_str().unwrap()).expect("a known tag")
    }

    fn key(kind: MachineKeyKind, value: &str) -> MachineKey {
        MachineKey {
            kind,
            value: value.to_string(),
        }
    }

    // spec: MEDULLA-MACHINE-R002
    // spec: MEDULLA-MACHINE-R002.2
    #[test]
    fn every_vector_returns_its_recorded_iri() {
        let data = vectors();
        let rows = data["iri_vectors"].as_array().unwrap();
        assert_eq!(rows.len(), 7);
        for row in rows {
            let kind = kind_of(&row["kind"]);
            let normalized = normalize_key(kind, row["raw"].as_str().unwrap()).unwrap();
            assert_eq!(normalized.value, row["normalized"].as_str().unwrap());
            let iri = machine_iri(row["tenant"].as_str().unwrap(), &normalized);
            assert_eq!(iri, row["iri"].as_str().unwrap());
        }
    }

    // spec: MEDULLA-MACHINE-R002.2
    #[test]
    fn select_key_prefers_uuid_then_serial_then_bmc_then_host() {
        let data = vectors();
        let rows = data["preference_vectors"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        for row in rows {
            let keys: Vec<MachineKey> = row["keys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|k| key(kind_of(&k["kind"]), k["value"].as_str().unwrap()))
                .collect();
            let expected = row["selects"]
                .as_object()
                .map(|s| key(kind_of(&s["kind"]), s["value"].as_str().unwrap()));
            assert_eq!(select_key(&keys), expected);
        }
        let all = [
            key(MachineKeyKind::Hostname, "alpha"),
            key(MachineKeyKind::ManagementControllerId, "aa:bb:cc:dd:ee:01"),
            key(MachineKeyKind::SystemSerial, "SN-0001"),
            key(
                MachineKeyKind::SystemUuid,
                "123e4567-e89b-12d3-a456-426614174000",
            ),
        ];
        assert_eq!(select_key(&all).unwrap().kind, MachineKeyKind::SystemUuid);
    }

    // spec: MEDULLA-MACHINE-R002.2
    #[test]
    fn hostname_iri_differs_across_tenants() {
        let host = key(MachineKeyKind::Hostname, "node-1.example");
        assert_ne!(machine_iri("tenant-a", &host), machine_iri("tenant-b", &host));
        let serial = key(MachineKeyKind::SystemSerial, "SN-0001");
        assert_eq!(machine_iri("tenant-a", &serial), machine_iri("tenant-b", &serial));
    }

    // spec: MEDULLA-MACHINE-R002.4
    #[test]
    fn key_anchor_vectors_match() {
        let data = vectors();
        let rows = data["anchor_vectors"].as_array().unwrap();
        assert_eq!(rows.len(), 5);
        for row in rows {
            let anchor = key(kind_of(&row["kind"]), row["value"].as_str().unwrap());
            assert_eq!(machine_key_iri(&anchor), row["iri"].as_str().unwrap());
        }
    }

    // spec: MEDULLA-MACHINE-R002.1
    #[test]
    fn normalization_and_placeholder_refusals_match_the_table() {
        use MachineKeyKind as K;
        use MachineKeyRefusalReason as R;
        let table: [(K, &str, Result<&str, R>); 30] = [
            (K::SystemUuid, "123E4567-E89B-12D3-A456-426614174000", Ok("123e4567-e89b-12d3-a456-426614174000")),
            (K::SystemUuid, "  123e4567-e89b-12d3-a456-426614174000 ", Ok("123e4567-e89b-12d3-a456-426614174000")),
            (K::SystemUuid, "123e4567e89b12d3a456426614174000", Err(R::Malformed)),
            (K::SystemUuid, "123e4567-e89b-12d3-a456-42661417400g", Err(R::Malformed)),
            (K::SystemUuid, "", Err(R::Empty)),
            (K::SystemUuid, "   ", Err(R::Empty)),
            (K::SystemUuid, "00000000-0000-0000-0000-000000000000", Err(R::Placeholder)),
            (K::SystemUuid, "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF", Err(R::Placeholder)),
            (K::SystemUuid, "03000200-0400-0500-0006-000700080009", Err(R::Placeholder)),
            (K::SystemUuid, "00020003-0004-0005-0006-000700080009", Err(R::Placeholder)),
            (K::SystemSerial, "  abc   123 ", Ok("ABC 123")),
            (K::SystemSerial, "sn\t0001", Ok("SN 0001")),
            (K::SystemSerial, "", Err(R::Empty)),
            (K::SystemSerial, "To Be Filled By O.E.M.", Err(R::Placeholder)),
            (K::SystemSerial, "Default String", Err(R::Placeholder)),
            (K::SystemSerial, "NONE", Err(R::Placeholder)),
            (K::SystemSerial, "N/A", Err(R::Placeholder)),
            (K::SystemSerial, "Not  Specified", Err(R::Placeholder)),
            (K::SystemSerial, "System Serial Number", Err(R::Placeholder)),
            (K::SystemSerial, "0", Err(R::Placeholder)),
            (K::SystemSerial, "123456789", Err(R::Placeholder)),
            (K::SystemSerial, "AB\u{7}C", Err(R::Malformed)),
            (K::ManagementControllerId, "AA-BB-CC-DD-EE-01", Ok("aa:bb:cc:dd:ee:01")),
            (K::ManagementControllerId, "aabbccddee01", Ok("aa:bb:cc:dd:ee:01")),
            (K::ManagementControllerId, "aa:bb:cc:dd:ee", Err(R::Malformed)),
            (K::ManagementControllerId, "00:00:00:00:00:00", Err(R::Placeholder)),
            (K::ManagementControllerId, "FF-FF-FF-FF-FF-FF", Err(R::Placeholder)),
            (K::Hostname, "Node-1.Example.", Ok("node-1.example")),
            (K::Hostname, "bad_name", Err(R::Malformed)),
            (K::Hostname, "a..b", Err(R::Malformed)),
        ];
        for (kind, raw, expected) in table {
            let got = normalize_key(kind, raw).map(|k| k.value);
            let want = expected.map(str::to_string).map_err(|reason| MachineKeyRefused { kind, reason });
            assert_eq!(got, want, "{kind:?} {raw:?}");
        }
        assert_eq!(normalize_key(K::Hostname, "").unwrap_err().reason, R::Empty);
        assert_eq!(normalize_key(K::Hostname, ".").unwrap_err().reason, R::Empty);
        let long = format!("{}.example", "a".repeat(250));
        assert_eq!(normalize_key(K::Hostname, &long).unwrap_err().reason, R::Malformed);
        assert_eq!(
            normalize_key(K::SystemUuid, "").unwrap_err().to_string(),
            "uuid:empty"
        );
    }
}
