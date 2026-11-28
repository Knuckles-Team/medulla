//! The inventory record set and its canonical form (MEDULLA-MACHINE-R006.3).
//!
//! The records are plain data in the shape of the connector ingest contract: entities with a
//! node type and properties, and relationships between entity ids. A property value is a text,
//! an integer, a boolean or a list of texts; no float exists. [`canonical_json`] writes one
//! byte sequence for one record set, whatever the order the caller built it in: object keys
//! sort by code point, lists of texts sort and drop duplicates, entities sort by
//! `(node_type, id)` and relationships by `(source, relationship, target)`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The schema tag of a record set.
pub const INVENTORY_RECORDS_SCHEMA: &str = "inventory-records/v1";

/// One property value. No float exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum PropertyValue {
    /// A text.
    Text(String),
    /// An integer.
    Integer(i64),
    /// A boolean.
    Boolean(bool),
    /// A list of texts. The canonical form sorts it and drops duplicates.
    List(Vec<String>),
}

/// One entity of a record set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InventoryEntity {
    /// The entity id.
    pub id: String,
    /// The core class of the entity: `Machine`, `MachineKey`, a part class, `ProbeSnapshot` or
    /// `MachineLifecycleTransitionRecord`.
    pub node_type: String,
    /// The properties by name.
    pub properties: BTreeMap<String, PropertyValue>,
}

/// One relationship of a record set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InventoryRelationship {
    /// The id of the source entity.
    pub source: String,
    /// The relationship name: `identifies`, `hasComponent`, `bondMember`, `observes` or
    /// `transitionOf`.
    pub relationship: String,
    /// The id of the target entity.
    pub target: String,
}

/// The record set of one inventory build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InventoryRecordSet {
    /// Always [`INVENTORY_RECORDS_SCHEMA`].
    pub schema: String,
    /// The entities.
    pub entities: Vec<InventoryEntity>,
    /// The relationships.
    pub relationships: Vec<InventoryRelationship>,
}

impl InventoryRecordSet {
    /// An empty record set of the current schema.
    pub fn new() -> Self {
        Self {
            schema: INVENTORY_RECORDS_SCHEMA.to_string(),
            entities: Vec::new(),
            relationships: Vec::new(),
        }
    }
}

impl Default for InventoryRecordSet {
    fn default() -> Self {
        Self::new()
    }
}

/// The record set in its canonical order: sorted lists, sorted entities, sorted relationships.
fn ordered(set: &InventoryRecordSet) -> InventoryRecordSet {
    let mut out = set.clone();
    for entity in &mut out.entities {
        for value in entity.properties.values_mut() {
            if let PropertyValue::List(items) = value {
                items.sort();
                items.dedup();
            }
        }
    }
    out.entities
        .sort_by(|a, b| (&a.node_type, &a.id).cmp(&(&b.node_type, &b.id)));
    out.relationships.sort_by(|a, b| {
        (&a.source, &a.relationship, &a.target).cmp(&(&b.source, &b.relationship, &b.target))
    });
    out
}

/// Write `value` as JSON with object keys sorted by code point, no extra white space and
/// UTF-8 text. The writer is its own, because the key order of a `serde_json` map depends on a
/// feature that another crate of the build may switch on.
fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (index, (key, item)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// The canonical JSON text of any plain-data value: sorted keys, no extra white space.
pub fn canonical_text<T: Serialize>(value: &T) -> String {
    let mut out = String::new();
    let tree = serde_json::to_value(value).unwrap_or(Value::Null);
    write_canonical(&tree, &mut out);
    out
}

/// The canonical JSON text of a record set.
pub fn canonical_json(set: &InventoryRecordSet) -> String {
    canonical_text(&ordered(set))
}

/// `sha256:` followed by the 64 lower-case hex digits of the SHA-256 of `bytes`.
pub fn canonical_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// The kinds of hardware part, with the tag each uses in an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    /// A processor package.
    Processor,
    /// A memory module.
    Memory,
    /// A storage device.
    Storage,
    /// A network interface.
    Nic,
    /// A network bond.
    Bond,
    /// An accelerator.
    Accelerator,
    /// A management controller.
    Management,
}

impl ComponentKind {
    /// The tag of the kind in a part id.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Processor => "processor",
            Self::Memory => "memory",
            Self::Storage => "storage",
            Self::Nic => "nic",
            Self::Bond => "bond",
            Self::Accelerator => "accelerator",
            Self::Management => "management",
        }
    }

    /// The kind whose tag is `tag`.
    pub fn from_tag(tag: &str) -> Option<Self> {
        [
            Self::Processor,
            Self::Memory,
            Self::Storage,
            Self::Nic,
            Self::Bond,
            Self::Accelerator,
            Self::Management,
        ]
        .into_iter()
        .find(|kind| kind.tag() == tag)
    }

    /// The property names that name a part of this kind, in preference order. After them comes
    /// the ordinal.
    fn natural_key_properties(self) -> &'static [&'static str] {
        match self {
            Self::Processor => &["socketIndex"],
            Self::Memory => &["componentSlot", "serialNumber"],
            Self::Storage => &["serialNumber", "deviceName"],
            Self::Nic => &["macAddress", "interfaceName"],
            Self::Bond => &["bondName"],
            Self::Accelerator => &["pciAddress", "serialNumber"],
            Self::Management => &["controllerId"],
        }
    }

    /// Whether a part of this kind may fall back to its ordinal.
    fn has_ordinal(self) -> bool {
        !matches!(self, Self::Bond | Self::Management)
    }
}

/// First 32 lower-case hex digits of the SHA-256 of `parts` joined by LF.
fn id_digest(parts: &[&str]) -> String {
    hex::encode(Sha256::digest(parts.join("\n").as_bytes()))[..32].to_string()
}

/// The id of one part: `urn:machine-part:` and the digest of the machine IRI, the kind tag and
/// the natural key.
pub fn component_id(machine_iri: &str, kind: ComponentKind, natural_key: &str) -> String {
    format!(
        "urn:machine-part:{}",
        id_digest(&[machine_iri, kind.tag(), natural_key])
    )
}

/// The id of one probe snapshot: `urn:machine-probe:` and the digest of the machine IRI, the
/// probe kind and the probe digest.
pub fn probe_id(machine_iri: &str, probe_kind: &str, probe_digest: &str) -> String {
    format!(
        "urn:machine-probe:{}",
        id_digest(&[machine_iri, probe_kind, probe_digest])
    )
}

/// The id of one lifecycle transition record: `urn:machine-transition:` and the digest of the
/// machine IRI, the edge id and the evidence digest.
pub fn transition_id(machine_iri: &str, edge_id: &str, evidence_digest: &str) -> String {
    format!(
        "urn:machine-transition:{}",
        id_digest(&[machine_iri, edge_id, evidence_digest])
    )
}

/// The text a property contributes to a natural key, when it holds a non-empty value.
fn natural_text(part: &BTreeMap<String, PropertyValue>, name: &str) -> Option<String> {
    match part.get(name)? {
        PropertyValue::Text(text) if !text.is_empty() => Some(text.clone()),
        PropertyValue::Integer(number) => Some(number.to_string()),
        _ => None,
    }
}

/// The ids of the parts of one kind of one machine, in the order the parts came in. The natural
/// key of a part is its first non-empty key property, then `ordinal:<n>`, where `n` is the
/// zero-based position after the part records sort by their canonical JSON text. A part of a
/// kind with no ordinal and no key property has no id and yields `None`.
pub fn component_ids(
    machine_iri: &str,
    kind: ComponentKind,
    parts: &[BTreeMap<String, PropertyValue>],
) -> Vec<Option<String>> {
    let mut order: Vec<usize> = (0..parts.len()).collect();
    let texts: Vec<String> = parts.iter().map(canonical_text).collect();
    order.sort_by(|a, b| texts[*a].cmp(&texts[*b]));
    let mut ordinal = vec![0usize; parts.len()];
    for (position, index) in order.into_iter().enumerate() {
        ordinal[index] = position;
    }
    parts
        .iter()
        .enumerate()
        .map(|(index, part)| {
            let named = kind
                .natural_key_properties()
                .iter()
                .find_map(|name| natural_text(part, name));
            let key = named.or_else(|| kind.has_ordinal().then(|| format!("ordinal:{}", ordinal[index])));
            key.map(|key| component_id(machine_iri, kind, &key))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> PropertyValue {
        PropertyValue::Text(value.to_string())
    }

    fn entity(id: &str, node_type: &str, props: &[(&str, PropertyValue)]) -> InventoryEntity {
        InventoryEntity {
            id: id.to_string(),
            node_type: node_type.to_string(),
            properties: props
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect(),
        }
    }

    fn relationship(source: &str, name: &str, target: &str) -> InventoryRelationship {
        InventoryRelationship {
            source: source.to_string(),
            relationship: name.to_string(),
            target: target.to_string(),
        }
    }

    fn sample() -> InventoryRecordSet {
        let mut set = InventoryRecordSet::new();
        set.entities = vec![
            entity(
                "urn:machine:serial:aa",
                "Machine",
                &[
                    ("machineName", text("alpha")),
                    ("processorCount", PropertyValue::Integer(2)),
                    ("retireRequested", PropertyValue::Boolean(false)),
                    (
                        "machineRole",
                        PropertyValue::List(vec!["worker".into(), "build".into(), "worker".into()]),
                    ),
                ],
            ),
            entity("urn:machine-key:host:bb", "MachineKey", &[("machineKey", text("host:alpha"))]),
        ];
        set.relationships = vec![
            relationship("urn:machine-key:host:bb", "identifies", "urn:machine:serial:aa"),
            relationship("urn:machine:serial:aa", "hasComponent", "urn:machine-part:cc"),
        ];
        set
    }

    // spec: MEDULLA-MACHINE-R006.3
    #[test]
    fn canonical_json_is_byte_stable_under_input_reordering() {
        let first = sample();
        let mut second = sample();
        second.entities.reverse();
        second.relationships.reverse();
        if let Some(PropertyValue::List(items)) =
            second.entities.iter_mut().find_map(|e| e.properties.get_mut("machineRole"))
        {
            items.reverse();
        }
        let bytes = canonical_json(&first);
        assert_eq!(bytes, canonical_json(&second));
        assert_eq!(
            bytes,
            concat!(
                r#"{"entities":[{"id":"urn:machine:serial:aa","node_type":"Machine","properties":{"#,
                r#""machineName":"alpha","machineRole":["build","worker"],"#,
                r#""processorCount":2,"retireRequested":false}},"#,
                r#"{"id":"urn:machine-key:host:bb","node_type":"MachineKey","#,
                r#""properties":{"machineKey":"host:alpha"}}],"#,
                r#""relationships":[{"relationship":"identifies","source":"urn:machine-key:host:bb","#,
                r#""target":"urn:machine:serial:aa"},"#,
                r#"{"relationship":"hasComponent","source":"urn:machine:serial:aa","#,
                r#""target":"urn:machine-part:cc"}],"schema":"inventory-records/v1"}"#
            )
        );
        assert_eq!(canonical_digest(bytes.as_bytes()), canonical_digest(canonical_json(&second).as_bytes()));
        assert!(canonical_digest(b"").starts_with("sha256:e3b0c44298fc1c14"));
        assert_eq!(canonical_digest(b"abc").len(), 71);
    }

    // spec: MEDULLA-MACHINE-R006.3
    #[test]
    fn canonical_text_sorts_keys_at_every_depth_and_escapes_text() {
        let value = serde_json::json!({"b": {"z": 1, "a": [{"y": true, "x": null}]}, "a": "q\"\n\u{e9}"});
        assert_eq!(
            canonical_text(&value),
            "{\"a\":\"q\\\"\\n\u{e9}\",\"b\":{\"a\":[{\"x\":null,\"y\":true}],\"z\":1}}"
        );
    }

    // spec: MEDULLA-MACHINE-R006.3
    #[test]
    fn ids_match_the_recorded_vectors() {
        let machine = "urn:machine:serial:f3af1defd63343f5dd2ee44b56cb636d";
        let rows: [(String, &str); 3] = [
            (
                component_id(machine, ComponentKind::Nic, "aa:bb:cc:dd:ee:01"),
                "urn:machine-part:ac177e96f0f67eb6b2177b714b5c56ff",
            ),
            (
                probe_id(machine, "system", &canonical_digest(b"probe")),
                "urn:machine-probe:28f5f2cfc55fdb0d71f831aa5bc76257",
            ),
            (
                transition_id(machine, "H01", &canonical_digest(b"evidence")),
                "urn:machine-transition:69c8d2b36e2b8c081faafbb880d25b2e",
            ),
        ];
        for (got, want) in rows {
            assert_eq!(got, want);
        }
    }

    // spec: MEDULLA-MACHINE-R006.3
    #[test]
    fn part_ids_use_the_natural_key_then_the_ordinal() {
        let machine = "urn:machine:host:1";
        let nic = |mac: Option<&str>, name: Option<&str>| {
            let mut part = BTreeMap::new();
            if let Some(mac) = mac {
                part.insert("macAddress".to_string(), text(mac));
            }
            if let Some(name) = name {
                part.insert("interfaceName".to_string(), text(name));
            }
            part
        };
        let parts = [nic(Some("aa:bb:cc:dd:ee:01"), Some("eth0")), nic(None, Some("eth1")), nic(None, None)];
        let ids = component_ids(machine, ComponentKind::Nic, &parts);
        assert_eq!(ids[0], Some(component_id(machine, ComponentKind::Nic, "aa:bb:cc:dd:ee:01")));
        assert_eq!(ids[1], Some(component_id(machine, ComponentKind::Nic, "eth1")));
        // `{}` sorts after `{"..."`, so the empty record is ordinal 2.
        assert_eq!(ids[2], Some(component_id(machine, ComponentKind::Nic, "ordinal:2")));
        let bond = component_ids(machine, ComponentKind::Bond, &[BTreeMap::new()]);
        assert_eq!(bond, vec![None]);
        let cpu = component_ids(machine, ComponentKind::Processor, &[BTreeMap::new(), BTreeMap::new()]);
        assert_eq!(cpu.len(), 2);
        assert_eq!(ComponentKind::from_tag("nic"), Some(ComponentKind::Nic));
        assert_eq!(ComponentKind::from_tag("gpu"), None);
    }
}
