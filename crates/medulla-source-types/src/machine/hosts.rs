//! The hosts file and its normalized form (MEDULLA-MACHINE-R006.1, R006.2).
//!
//! A hosts file (`schema: hosts/v1`) declares which machines exist and which references reach
//! them. It holds no secret: a key named like one refuses the whole file. [`parse_hosts_file`]
//! validates the YAML and returns the typed file; a refusal names a JSON pointer to the first
//! offending value and never echoes the value. [`normalize_hosts_file`] turns a file into one
//! byte-stable form: hosts sorted by name, lists sorted without duplicates, addresses in lower
//! case, every absent value explicit.

use std::collections::BTreeSet;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

use super::identity::MachineKey;
use super::identity::{normalize_key, MachineKeyKind};
use super::lifecycle::MachineLifecycleState;
use super::records::{
    canonical_digest, canonical_text, machine_entity, InventoryEntity, MachineShapeRefused,
};

/// The schema tag of a hosts file.
pub const HOSTS_FILE_SCHEMA: &str = "hosts/v1";
/// The schema tag of a normalized hosts file.
pub const HOSTS_NORMALIZED_SCHEMA: &str = "hosts-normalized/v1";
/// The wire code of every refusal of a hosts file.
pub const HOSTS_FILE_INVALID: &str = "HOSTS_FILE_INVALID";

/// A hosts file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HostsFile {
    /// Exactly [`HOSTS_FILE_SCHEMA`].
    pub schema: String,
    /// The declared machines; at least one.
    pub hosts: Vec<HostEntry>,
}

/// One declared machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HostEntry {
    /// The name, unique in the file: `^[a-z0-9][a-z0-9-]{0,62}$`.
    pub name: String,
    /// At least one DNS name or IP literal.
    pub addresses: Vec<String>,
    /// The address of the out-of-band management controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_address: Option<String>,
    /// Roles, each `^[a-z][a-z0-9-]{0,31}$`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    /// Groups, with the pattern of a role.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// References to the credentials that reach the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<HostCredentials>,
    /// Hardware keys the operator pins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<HostIdentityPins>,
    /// Whether the machine is marked for retirement; absent means `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired: Option<bool>,
}

/// References to credentials: `<scheme>://<path>`, never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HostCredentials {
    /// The secure shell credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<String>,
    /// The management controller credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management: Option<String>,
}

/// Hardware keys the operator pins, so a later tier change cannot move the machine IRI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HostIdentityPins {
    /// The system UUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_uuid: Option<String>,
    /// The system serial number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_serial: Option<String>,
    /// The hardware address of the management controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_mac: Option<String>,
}

/// Why a hosts file was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostsFileRefusalReason {
    /// `schema` is not exactly `hosts/v1`.
    SchemaVersion,
    /// A key the schema does not name.
    UnknownField,
    /// Two hosts share a name.
    DuplicateName,
    /// A host name does not match its pattern.
    BadName,
    /// A host has no address.
    NoAddress,
    /// An address is neither a DNS name nor an IP literal.
    BadAddress,
    /// A credential reference or an identity pin is malformed.
    BadReference,
    /// A role or a group does not match its pattern.
    BadRole,
    /// A key is named like a secret.
    SecretField,
    /// The file declares no host.
    EmptyHosts,
    /// The text is not YAML of the expected shape.
    MalformedYaml,
}

impl HostsFileRefusalReason {
    /// The wire text of the reason.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SchemaVersion => "schema_version",
            Self::UnknownField => "unknown_field",
            Self::DuplicateName => "duplicate_name",
            Self::BadName => "bad_name",
            Self::NoAddress => "no_address",
            Self::BadAddress => "bad_address",
            Self::BadReference => "bad_reference",
            Self::BadRole => "bad_role",
            Self::SecretField => "secret_field",
            Self::EmptyHosts => "empty_hosts",
            Self::MalformedYaml => "malformed_yaml",
        }
    }
}

/// The refusal of a hosts file: `{code, reason, pointer}`. The pointer is a JSON pointer to the
/// first offending value; the record never holds the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {} at '{pointer}'", reason.as_str())]
pub struct HostsFileRefused {
    /// Always [`HOSTS_FILE_INVALID`].
    pub code: String,
    /// Why the file was refused.
    pub reason: HostsFileRefusalReason,
    /// A JSON pointer to the first offending value.
    pub pointer: String,
}

impl HostsFileRefused {
    fn at(reason: HostsFileRefusalReason, pointer: &str) -> Self {
        Self {
            code: HOSTS_FILE_INVALID.to_string(),
            reason,
            pointer: pointer.to_string(),
        }
    }
}

type Refusal = HostsFileRefused;
type Reason = HostsFileRefusalReason;

const SECRET_KEYS: [&str; 6] = [
    "password",
    "passphrase",
    "secret",
    "token",
    "private_key",
    "api_key",
];
const HOSTS_KEYS: [&str; 2] = ["schema", "hosts"];
const HOST_KEYS: [&str; 8] = [
    "name",
    "addresses",
    "management_address",
    "roles",
    "groups",
    "credentials",
    "identity",
    "retired",
];
const CREDENTIAL_KEYS: [&str; 2] = ["ssh", "management"];
const IDENTITY_KEYS: [&str; 3] = ["system_uuid", "system_serial", "management_mac"];

fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

fn child(pointer: &str, token: &str) -> String {
    format!("{pointer}/{}", escape(token))
}

/// Parse and validate a hosts file.
pub fn parse_hosts_file(yaml: &str) -> Result<HostsFile, HostsFileRefused> {
    let root: Value =
        serde_yaml_ng::from_str(yaml).map_err(|_| Refusal::at(Reason::MalformedYaml, ""))?;
    reject_secret_keys(&root, "")?;
    validate_root(&root)?;
    serde_yaml_ng::from_value(root).map_err(|_| Refusal::at(Reason::MalformedYaml, ""))
}

fn reject_secret_keys(value: &Value, pointer: &str) -> Result<(), Refusal> {
    match value {
        Value::Mapping(map) => {
            for (key, item) in map {
                let name = key.as_str().unwrap_or_default();
                let here = child(pointer, name);
                if SECRET_KEYS.contains(&name.to_ascii_lowercase().as_str()) {
                    return Err(Refusal::at(Reason::SecretField, &here));
                }
                reject_secret_keys(item, &here)?;
            }
            Ok(())
        }
        Value::Sequence(items) => {
            for (index, item) in items.iter().enumerate() {
                reject_secret_keys(item, &child(pointer, &index.to_string()))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn mapping<'a>(value: &'a Value, pointer: &str) -> Result<&'a serde_yaml_ng::Mapping, Refusal> {
    value
        .as_mapping()
        .ok_or_else(|| Refusal::at(Reason::MalformedYaml, pointer))
}

fn check_keys(
    map: &serde_yaml_ng::Mapping,
    allowed: &[&str],
    pointer: &str,
) -> Result<(), Refusal> {
    for key in map.keys() {
        let name = key.as_str().unwrap_or_default();
        if !allowed.contains(&name) {
            return Err(Refusal::at(Reason::UnknownField, &child(pointer, name)));
        }
    }
    Ok(())
}

fn validate_root(root: &Value) -> Result<(), Refusal> {
    let map = mapping(root, "")?;
    check_keys(map, &HOSTS_KEYS, "")?;
    if map.get("schema").and_then(Value::as_str) != Some(HOSTS_FILE_SCHEMA) {
        return Err(Refusal::at(Reason::SchemaVersion, "/schema"));
    }
    let hosts = match map.get("hosts") {
        None => return Err(Refusal::at(Reason::EmptyHosts, "/hosts")),
        Some(Value::Sequence(hosts)) if hosts.is_empty() => {
            return Err(Refusal::at(Reason::EmptyHosts, "/hosts"))
        }
        Some(Value::Sequence(hosts)) => hosts,
        Some(_) => return Err(Refusal::at(Reason::MalformedYaml, "/hosts")),
    };
    let mut seen = BTreeSet::new();
    for (index, host) in hosts.iter().enumerate() {
        validate_host(host, &format!("/hosts/{index}"), &mut seen)?;
    }
    Ok(())
}

fn validate_host(host: &Value, pointer: &str, seen: &mut BTreeSet<String>) -> Result<(), Refusal> {
    let map = mapping(host, pointer)?;
    check_keys(map, &HOST_KEYS, pointer)?;
    let name_pointer = child(pointer, "name");
    let name = map.get("name").and_then(Value::as_str).unwrap_or_default();
    if !valid_host_name(name) {
        return Err(Refusal::at(Reason::BadName, &name_pointer));
    }
    if !seen.insert(name.to_string()) {
        return Err(Refusal::at(Reason::DuplicateName, &name_pointer));
    }
    validate_addresses(map, pointer)?;
    for key in ["roles", "groups"] {
        validate_labels(map.get(key), &child(pointer, key))?;
    }
    validate_refs(map.get("credentials"), &child(pointer, "credentials"))?;
    validate_pins(map.get("identity"), &child(pointer, "identity"))?;
    match map.get("retired") {
        None | Some(Value::Bool(_)) => Ok(()),
        Some(_) => Err(Refusal::at(
            Reason::MalformedYaml,
            &child(pointer, "retired"),
        )),
    }
}

fn valid_host_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let first_ok = bytes
        .first()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    first_ok
        && bytes.len() <= 63
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn valid_address(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.parse::<IpAddr>().is_ok() {
        return true;
    }
    let label_ok = |label: &str| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    lower.len() <= 253 && lower.split('.').all(label_ok)
}

fn validate_addresses(map: &serde_yaml_ng::Mapping, pointer: &str) -> Result<(), Refusal> {
    let list_pointer = child(pointer, "addresses");
    match map.get("addresses") {
        None => return Err(Refusal::at(Reason::NoAddress, &list_pointer)),
        Some(Value::Sequence(items)) if items.is_empty() => {
            return Err(Refusal::at(Reason::NoAddress, &list_pointer))
        }
        Some(Value::Sequence(items)) => {
            for (index, item) in items.iter().enumerate() {
                let good = item.as_str().is_some_and(valid_address);
                if !good {
                    return Err(Refusal::at(
                        Reason::BadAddress,
                        &child(&list_pointer, &index.to_string()),
                    ));
                }
            }
        }
        Some(_) => return Err(Refusal::at(Reason::BadAddress, &list_pointer)),
    }
    match map.get("management_address") {
        None => Ok(()),
        Some(value) if value.as_str().is_some_and(valid_address) => Ok(()),
        Some(_) => Err(Refusal::at(
            Reason::BadAddress,
            &child(pointer, "management_address"),
        )),
    }
}

fn valid_label(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes.len() <= 32
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn validate_labels(value: Option<&Value>, pointer: &str) -> Result<(), Refusal> {
    match value {
        None => Ok(()),
        Some(Value::Sequence(items)) => {
            for (index, item) in items.iter().enumerate() {
                if !item.as_str().is_some_and(valid_label) {
                    return Err(Refusal::at(
                        Reason::BadRole,
                        &child(pointer, &index.to_string()),
                    ));
                }
            }
            Ok(())
        }
        Some(_) => Err(Refusal::at(Reason::BadRole, pointer)),
    }
}

/// `^[a-z][a-z0-9+.-]*://[^\s]+$`
fn valid_reference(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once("://") else {
        return false;
    };
    let scheme_ok = scheme
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase)
        && scheme.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'+' | b'.' | b'-')
        });
    scheme_ok && !rest.is_empty() && !rest.chars().any(char::is_whitespace)
}

fn validate_refs(value: Option<&Value>, pointer: &str) -> Result<(), Refusal> {
    let Some(value) = value else { return Ok(()) };
    let map = mapping(value, pointer)?;
    check_keys(map, &CREDENTIAL_KEYS, pointer)?;
    for (key, item) in map {
        let here = child(pointer, key.as_str().unwrap_or_default());
        if !item.as_str().is_some_and(valid_reference) {
            return Err(Refusal::at(Reason::BadReference, &here));
        }
    }
    Ok(())
}

fn pin_kind(name: &str) -> Option<MachineKeyKind> {
    match name {
        "system_uuid" => Some(MachineKeyKind::SystemUuid),
        "system_serial" => Some(MachineKeyKind::SystemSerial),
        "management_mac" => Some(MachineKeyKind::ManagementControllerId),
        _ => None,
    }
}

fn validate_pins(value: Option<&Value>, pointer: &str) -> Result<(), Refusal> {
    let Some(value) = value else { return Ok(()) };
    let map = mapping(value, pointer)?;
    check_keys(map, &IDENTITY_KEYS, pointer)?;
    for (key, item) in map {
        let name = key.as_str().unwrap_or_default();
        let valid = pin_kind(name)
            .zip(item.as_str())
            .is_some_and(|(kind, raw)| normalize_key(kind, raw).is_ok());
        if !valid {
            return Err(Refusal::at(Reason::BadReference, &child(pointer, name)));
        }
    }
    Ok(())
}

/// The normalized credentials of one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NormalizedCredentials {
    /// The secure shell credential reference, or `null`.
    pub ssh: Option<String>,
    /// The management credential reference, or `null`.
    pub management: Option<String>,
}

/// The normalized identity pins of one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NormalizedPins {
    /// The normalized system UUID, or `null`.
    pub system_uuid: Option<String>,
    /// The normalized system serial, or `null`.
    pub system_serial: Option<String>,
    /// The normalized management controller address, or `null`.
    pub management_mac: Option<String>,
}

/// One host in normalized form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NormalizedHost {
    /// The name.
    pub name: String,
    /// The addresses in lower case, sorted, without duplicates.
    pub addresses: Vec<String>,
    /// The management address in lower case, or `null`.
    pub management_address: Option<String>,
    /// The roles, sorted, without duplicates.
    pub roles: Vec<String>,
    /// The groups, sorted, without duplicates.
    pub groups: Vec<String>,
    /// The credential references.
    pub credentials: NormalizedCredentials,
    /// The identity pins.
    pub identity: NormalizedPins,
    /// Whether the host is marked for retirement.
    pub retired: bool,
}

/// A hosts file in normalized form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NormalizedHosts {
    /// Always [`HOSTS_NORMALIZED_SCHEMA`].
    pub schema: String,
    /// The hosts sorted by name.
    pub hosts: Vec<NormalizedHost>,
}

fn sorted_unique<I: IntoIterator<Item = String>>(items: I) -> Vec<String> {
    items
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn pin(kind: MachineKeyKind, raw: &Option<String>) -> Option<String> {
    let raw = raw.as_deref()?;
    normalize_key(kind, raw).ok().map(|key| key.value)
}

fn normalize_host(host: HostEntry) -> NormalizedHost {
    let credentials = host.credentials.unwrap_or(HostCredentials {
        ssh: None,
        management: None,
    });
    let pins = host.identity.unwrap_or(HostIdentityPins {
        system_uuid: None,
        system_serial: None,
        management_mac: None,
    });
    NormalizedHost {
        name: host.name,
        addresses: sorted_unique(host.addresses.iter().map(|a| a.to_ascii_lowercase())),
        management_address: host.management_address.map(|a| a.to_ascii_lowercase()),
        roles: sorted_unique(host.roles),
        groups: sorted_unique(host.groups),
        credentials: NormalizedCredentials {
            ssh: credentials.ssh,
            management: credentials.management,
        },
        identity: NormalizedPins {
            system_uuid: pin(MachineKeyKind::SystemUuid, &pins.system_uuid),
            system_serial: pin(MachineKeyKind::SystemSerial, &pins.system_serial),
            management_mac: pin(MachineKeyKind::ManagementControllerId, &pins.management_mac),
        },
        retired: host.retired.unwrap_or(false),
    }
}

/// Parse a hosts file and return its normalized form.
pub fn normalize_hosts_file(yaml: &str) -> Result<NormalizedHosts, HostsFileRefused> {
    let file = parse_hosts_file(yaml)?;
    let mut hosts: Vec<NormalizedHost> = file.hosts.into_iter().map(normalize_host).collect();
    hosts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(NormalizedHosts {
        schema: HOSTS_NORMALIZED_SCHEMA.to_string(),
        hosts,
    })
}

impl NormalizedHost {
    /// The identity keys of the host: its pins and its name.
    pub fn keys(&self) -> Vec<MachineKey> {
        let mut keys = vec![MachineKey {
            kind: MachineKeyKind::Hostname,
            value: self.name.clone(),
        }];
        let pins = [
            (MachineKeyKind::SystemUuid, &self.identity.system_uuid),
            (MachineKeyKind::SystemSerial, &self.identity.system_serial),
            (
                MachineKeyKind::ManagementControllerId,
                &self.identity.management_mac,
            ),
        ];
        keys.extend(pins.into_iter().filter_map(|(kind, value)| {
            value.as_ref().map(|value| MachineKey {
                kind,
                value: value.clone(),
            })
        }));
        keys
    }
}

/// The `Machine` entity of one normalized host in `tenant`. A host that would yield a machine
/// failing `:MachineShape` is refused here instead of reaching the engine.
pub fn host_machine_entity(
    tenant: &str,
    host: &NormalizedHost,
    state: MachineLifecycleState,
) -> Result<InventoryEntity, MachineShapeRefused> {
    machine_entity(tenant, &host.name, &host.keys(), state)
}

/// The canonical JSON text of a normalized hosts file.
pub fn normalized_hosts_json(hosts: &NormalizedHosts) -> String {
    canonical_text(hosts)
}

/// The digest of the canonical JSON of a normalized hosts file.
pub fn hosts_digest(hosts: &NormalizedHosts) -> String {
    canonical_digest(normalized_hosts_json(hosts).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "\
schema: hosts/v1
hosts:
  - name: beta
    addresses: [BETA.Example, 10.0.0.2, 10.0.0.2]
    management_address: BMC-BETA.example
    roles: [worker, build, worker]
    groups: [rack-b]
    credentials:
      ssh: vault://ssh/beta
      management: vault://ipmi/beta
    identity:
      system_uuid: 123E4567-E89B-12D3-A456-426614174000
      management_mac: AA-BB-CC-DD-EE-01
    retired: true
  - name: alpha
    addresses: [alpha.example]
";

    const VALID_REORDERED: &str = "\
hosts:
  - addresses: [alpha.example]
    name: alpha
  - retired: true
    identity:
      management_mac: aa:bb:cc:dd:ee:01
      system_uuid: 123e4567-e89b-12d3-a456-426614174000
    credentials:
      management: vault://ipmi/beta
      ssh: vault://ssh/beta
    groups: [rack-b]
    roles: [build, worker]
    management_address: bmc-beta.example
    addresses: [10.0.0.2, beta.example]
    name: beta
schema: hosts/v1
";

    fn refusal(yaml: &str) -> HostsFileRefused {
        parse_hosts_file(yaml).expect_err("the file refuses")
    }

    fn one_host(extra: &str) -> String {
        format!(
            "schema: hosts/v1\nhosts:\n  - name: alpha\n    addresses: [alpha.example]\n{extra}"
        )
    }

    // spec: MEDULLA-MACHINE-R006.1
    #[test]
    fn each_refusal_reason_has_a_fixture_and_never_echoes_the_value() {
        use HostsFileRefusalReason as R;
        let cases: [(R, String, &str); 11] = [
            (
                R::SchemaVersion,
                "schema: hosts/v2\nhosts: []\n".into(),
                "/schema",
            ),
            (
                R::UnknownField,
                one_host("    flavour: cherry\n"),
                "/hosts/0/flavour",
            ),
            (
                R::DuplicateName,
                format!(
                    "{}  - name: alpha\n    addresses: [b.example]\n",
                    one_host("")
                ),
                "/hosts/1/name",
            ),
            (
                R::BadName,
                "schema: hosts/v1\nhosts:\n  - name: Al_pha\n    addresses: [a.example]\n".into(),
                "/hosts/0/name",
            ),
            (
                R::NoAddress,
                "schema: hosts/v1\nhosts:\n  - name: alpha\n    addresses: []\n".into(),
                "/hosts/0/addresses",
            ),
            (
                R::BadAddress,
                "schema: hosts/v1\nhosts:\n  - name: alpha\n    addresses: [not_a_host!]\n".into(),
                "/hosts/0/addresses/0",
            ),
            (
                R::BadReference,
                one_host("    credentials:\n      ssh: no-scheme-here\n"),
                "/hosts/0/credentials/ssh",
            ),
            (
                R::BadRole,
                one_host("    roles: [Bad_Role]\n"),
                "/hosts/0/roles/0",
            ),
            (
                R::SecretField,
                one_host("    credentials:\n      password: hunter2-secret-value\n"),
                "/hosts/0/credentials/password",
            ),
            (
                R::EmptyHosts,
                "schema: hosts/v1\nhosts: []\n".into(),
                "/hosts",
            ),
            (R::MalformedYaml, "schema: [unclosed\n".into(), ""),
        ];
        for (reason, yaml, pointer) in cases {
            let error = refusal(&yaml);
            assert_eq!(error.reason, reason, "{yaml}");
            assert_eq!(error.pointer, pointer, "{yaml}");
            assert_eq!(error.code, "HOSTS_FILE_INVALID");
            let record = serde_json::to_string(&error).unwrap();
            for echoed in [
                "hunter2-secret-value",
                "not_a_host!",
                "Al_pha",
                "no-scheme-here",
                "Bad_Role",
                "cherry",
            ] {
                let mentioned = record.contains(echoed) && !pointer.contains(echoed);
                assert!(!mentioned, "{reason:?} echoes {echoed}");
            }
        }
        let secret_deep =
            one_host("    identity:\n      system_serial: x\n    labels:\n      - api_key: abcd\n");
        assert_eq!(refusal(&secret_deep).reason, R::SecretField);
        assert!(refusal("- just\n- a list\n").reason == R::MalformedYaml);
    }

    // spec: MEDULLA-MACHINE-R006.1
    #[test]
    fn hosts_file_schema_lists_exactly_the_documented_fields() {
        let host = HostEntry {
            name: "a".into(),
            addresses: vec!["a.example".into()],
            management_address: Some("m.example".into()),
            roles: vec!["r".into()],
            groups: vec!["g".into()],
            credentials: Some(HostCredentials {
                ssh: Some("v://x".into()),
                management: Some("v://y".into()),
            }),
            identity: Some(HostIdentityPins {
                system_uuid: Some("u".into()),
                system_serial: Some("s".into()),
                management_mac: Some("m".into()),
            }),
            retired: Some(false),
        };
        let file = HostsFile {
            schema: HOSTS_FILE_SCHEMA.into(),
            hosts: vec![host],
        };
        let json = serde_json::to_value(&file).unwrap();
        let keys = |v: &serde_json::Value| -> Vec<String> {
            let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        let mut documented = HOSTS_KEYS.map(String::from).to_vec();
        documented.sort();
        assert_eq!(keys(&json), documented);
        let mut host_keys = HOST_KEYS.map(String::from).to_vec();
        host_keys.sort();
        assert_eq!(keys(&json["hosts"][0]), host_keys);
        let mut credential_keys = CREDENTIAL_KEYS.map(String::from).to_vec();
        credential_keys.sort();
        assert_eq!(keys(&json["hosts"][0]["credentials"]), credential_keys);
        let mut identity_keys = IDENTITY_KEYS.map(String::from).to_vec();
        identity_keys.sort();
        assert_eq!(keys(&json["hosts"][0]["identity"]), identity_keys);
        assert!(
            serde_json::from_str::<HostsFile>(r#"{"schema":"hosts/v1","hosts":[],"x":1}"#).is_err()
        );
        #[cfg(feature = "contract-schema")]
        {
            let schema = serde_json::to_value(schemars::schema_for!(HostsFile)).unwrap();
            let defs = schema["$defs"]["HostEntry"]["properties"]
                .as_object()
                .unwrap();
            let mut fields: Vec<String> = defs.keys().cloned().collect();
            fields.sort();
            assert_eq!(fields, host_keys);
        }
    }

    // spec: MEDULLA-MACHINE-R006.2
    #[test]
    fn two_orderings_of_one_file_normalize_to_identical_bytes() {
        let first = normalize_hosts_file(VALID).unwrap();
        let second = normalize_hosts_file(VALID_REORDERED).unwrap();
        let (a, b) = (
            normalized_hosts_json(&first),
            normalized_hosts_json(&second),
        );
        assert_eq!(a, b);
        assert_eq!(hosts_digest(&first), hosts_digest(&second));
        assert!(hosts_digest(&first).starts_with("sha256:"));
        assert_eq!(first.schema, "hosts-normalized/v1");
        let names: Vec<&str> = first.hosts.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta"]);
        let alpha = &first.hosts[0];
        assert!(!alpha.retired && alpha.roles.is_empty() && alpha.management_address.is_none());
        assert_eq!(alpha.credentials.ssh, None);
        let beta = &first.hosts[1];
        assert_eq!(beta.addresses, ["10.0.0.2", "beta.example"]);
        assert_eq!(
            beta.identity.system_uuid.as_deref(),
            Some("123e4567-e89b-12d3-a456-426614174000")
        );
        assert_eq!(
            beta.identity.management_mac.as_deref(),
            Some("aa:bb:cc:dd:ee:01")
        );
        assert!(a.contains("\"system_serial\":null"));
    }
    // spec: MEDULLA-MACHINE-R006.2
    #[test]
    fn every_normalized_host_yields_a_machine_that_passes_the_machine_shape() {
        let hosts = normalize_hosts_file(VALID).expect("the fixture normalizes");
        assert!(!hosts.hosts.is_empty());
        for host in &hosts.hosts {
            let entity = host_machine_entity("tenant-1", host, MachineLifecycleState::Commissioned)
                .unwrap_or_else(|e| panic!("{} is refused: {e}", host.name));
            assert_eq!(
                entity.properties["machineName"],
                crate::machine::records::PropertyValue::Text(host.name.clone())
            );
            // The same host in another tenant is another machine only through the host name key.
            assert!(entity.id.starts_with("urn:machine:"));
        }
    }
}
