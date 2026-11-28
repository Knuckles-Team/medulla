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

use super::identity::{machine_iri, normalize_key, select_key, MachineKey, MachineKeyKind};
use super::lifecycle::{MachineLifecycleState, MACHINE_LIFECYCLE_STATE_PROPERTY};

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
    /// The relationship name: `identifies`, one of the seven owner properties of
    /// [`ComponentKind::owner_relationship`], `bondMember`, `observes` or `transitionOf`.
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
    /// The core class of a part of this kind.
    pub fn class_name(self) -> &'static str {
        match self {
            Self::Processor => "CentralProcessor",
            Self::Memory => "MemoryModule",
            Self::Storage => "StorageDevice",
            Self::Nic => "NetworkInterface",
            Self::Bond => "NetworkBond",
            Self::Accelerator => "Accelerator",
            Self::Management => "ManagementController",
        }
    }

    /// The relationship a machine uses to own a part of this kind. Each is a sub-property of
    /// `hasComponent` with a single range.
    pub fn owner_relationship(self) -> &'static str {
        match self {
            Self::Processor => "hasProcessor",
            Self::Memory => "hasMemoryModule",
            Self::Storage => "hasStorageDevice",
            Self::Nic => "hasNetworkInterface",
            Self::Bond => "hasNetworkBond",
            Self::Accelerator => "hasAccelerator",
            Self::Management => "hasManagementController",
        }
    }

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

/// The five stored lifecycle states, as the machine shape lists them.
const STORED_STATES: [&str; 5] = [
    "commissioned",
    "active",
    "draining",
    "maintenance",
    "retired",
];

/// A record that would fail its shape in the engine. The messages carry the wording of the
/// `:MachineShape` constraint that the record would break.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{entity}: {message}")]
pub struct MachineShapeRefused {
    /// The id of the entity that fails, or the relationship it belongs to.
    pub entity: String,
    /// The constraint the entity breaks.
    pub message: String,
}

fn refused(entity: &str, message: &str) -> MachineShapeRefused {
    MachineShapeRefused {
        entity: entity.to_string(),
        message: message.to_string(),
    }
}

/// Every value of a property as a list of texts, or `None` when a value is not a text.
fn texts(properties: &BTreeMap<String, PropertyValue>, name: &str) -> Option<Vec<String>> {
    match properties.get(name) {
        None => Some(Vec::new()),
        Some(PropertyValue::Text(text)) => Some(vec![text.clone()]),
        Some(PropertyValue::List(items)) => Some(items.clone()),
        Some(_) => None,
    }
}

fn is_machine_iri(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("urn:machine:") else {
        return false;
    };
    let Some((tier, digest)) = rest.split_once(':') else {
        return false;
    };
    MachineKeyKind::from_tag(tier).is_some()
        && digest.len() == 32
        && digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Check one `Machine` entity against the constraints of `:MachineShape`; every other entity
/// passes. This is the same check the engine makes on the stored node.
pub fn check_machine(entity: &InventoryEntity) -> Result<(), MachineShapeRefused> {
    if entity.node_type != "Machine" {
        return Ok(());
    }
    let id = entity.id.as_str();
    let props = &entity.properties;
    if !is_machine_iri(id) {
        return Err(refused(
            id,
            "A canonical machine is an IRI of the form urn:machine:<tier>:<32 hex digits>.",
        ));
    }
    let names = texts(props, "machineName").unwrap_or_default();
    if names.len() != 1 || names[0].is_empty() {
        return Err(refused(
            id,
            "A machine has exactly one non-empty :machineName.",
        ));
    }
    let tiers = texts(props, "identityTier").unwrap_or_default();
    if tiers.len() != 1 || MachineKeyKind::from_tag(&tiers[0]).is_none() {
        return Err(refused(
            id,
            "A machine has exactly one :identityTier: uuid, serial, bmc or host.",
        ));
    }
    let keys = texts(props, "machineKey").unwrap_or_default();
    let well_formed = |key: &String| {
        key.split_once(':').is_some_and(|(kind, value)| {
            MachineKeyKind::from_tag(kind).is_some() && !value.is_empty()
        })
    };
    if keys.is_empty() || !keys.iter().all(well_formed) {
        return Err(refused(
            id,
            "A machine has at least one :machineKey of the form <kind>:<value>.",
        ));
    }
    if texts(props, "systemUuid").map_or(true, |uuids| uuids.len() > 1) {
        return Err(refused(id, "A machine has at most one :systemUuid."));
    }
    let states = texts(props, MACHINE_LIFECYCLE_STATE_PROPERTY).unwrap_or_default();
    if states.len() != 1 || !STORED_STATES.contains(&states[0].as_str()) {
        return Err(refused(
            id,
            "A machine has exactly one :machineLifecycleState of the five stored states.",
        ));
    }
    Ok(())
}

/// Build the `Machine` entity of one host: the preferred valid key derives the IRI and the tier,
/// and every valid candidate becomes a `machineKey`. A machine that would fail `:MachineShape`
/// is refused instead of produced.
pub fn machine_entity(
    tenant: &str,
    name: &str,
    candidates: &[MachineKey],
    state: MachineLifecycleState,
) -> Result<InventoryEntity, MachineShapeRefused> {
    let valid: Vec<MachineKey> = candidates
        .iter()
        .filter_map(|candidate| normalize_key(candidate.kind, &candidate.value).ok())
        .collect();
    let Some(chosen) = select_key(&valid) else {
        return Err(refused(
            name,
            "A machine has at least one :machineKey of the form <kind>:<value>.",
        ));
    };
    let mut keys: Vec<String> = valid
        .iter()
        .map(|key| format!("{}:{}", key.kind.tag(), key.value))
        .collect();
    keys.sort();
    keys.dedup();
    let mut properties = BTreeMap::new();
    properties.insert(
        "machineName".to_string(),
        PropertyValue::Text(name.to_string()),
    );
    properties.insert(
        "identityTier".to_string(),
        PropertyValue::Text(chosen.kind.tag().to_string()),
    );
    properties.insert("machineKey".to_string(), PropertyValue::List(keys));
    if let Some(uuid) = valid
        .iter()
        .find(|key| key.kind == MachineKeyKind::SystemUuid)
    {
        properties.insert(
            "systemUuid".to_string(),
            PropertyValue::Text(uuid.value.clone()),
        );
    }
    properties.insert(
        MACHINE_LIFECYCLE_STATE_PROPERTY.to_string(),
        PropertyValue::Text(state_text(state).to_string()),
    );
    let entity = InventoryEntity {
        id: machine_iri(tenant, &chosen),
        node_type: "Machine".to_string(),
        properties,
    };
    check_machine(&entity)?;
    Ok(entity)
}

fn state_text(state: MachineLifecycleState) -> &'static str {
    match state {
        MachineLifecycleState::Commissioned => "commissioned",
        MachineLifecycleState::Active => "active",
        MachineLifecycleState::Draining => "draining",
        MachineLifecycleState::Maintenance => "maintenance",
        MachineLifecycleState::Retired => "retired",
    }
}

impl InventoryRecordSet {
    /// Refuse a record set that the engine would refuse: a `Machine` that fails `:MachineShape`,
    /// a part without exactly one owner property of its own kind, or a relationship name the
    /// vocabulary does not declare.
    pub fn check_shapes(&self) -> Result<(), MachineShapeRefused> {
        let owners: Vec<(&str, &str)> = [
            ComponentKind::Processor,
            ComponentKind::Memory,
            ComponentKind::Storage,
            ComponentKind::Nic,
            ComponentKind::Bond,
            ComponentKind::Accelerator,
            ComponentKind::Management,
        ]
        .into_iter()
        .map(|kind| (kind.class_name(), kind.owner_relationship()))
        .collect();
        for entity in &self.entities {
            check_machine(entity)?;
            if let Some((_, owner)) = owners.iter().find(|(class, _)| *class == entity.node_type) {
                let count = self
                    .relationships
                    .iter()
                    .filter(|r| r.target == entity.id && r.relationship == *owner)
                    .count();
                if count != 1 {
                    return Err(refused(
                        &entity.id,
                        &format!(
                            "A part is the object of exactly one :{owner}: this part has {count}."
                        ),
                    ));
                }
            }
        }
        const OTHERS: [&str; 4] = ["identifies", "bondMember", "observes", "transitionOf"];
        for relationship in &self.relationships {
            let name = relationship.relationship.as_str();
            if !OTHERS.contains(&name) && !owners.iter().any(|(_, owner)| *owner == name) {
                return Err(refused(
                    &relationship.source,
                    &format!("The relationship {name} is not declared."),
                ));
            }
        }
        Ok(())
    }
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
            let key = named.or_else(|| {
                kind.has_ordinal()
                    .then(|| format!("ordinal:{}", ordinal[index]))
            });
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
            entity(
                "urn:machine-key:host:bb",
                "MachineKey",
                &[("machineKey", text("host:alpha"))],
            ),
        ];
        set.relationships = vec![
            relationship(
                "urn:machine-key:host:bb",
                "identifies",
                "urn:machine:serial:aa",
            ),
            relationship(
                "urn:machine:serial:aa",
                "hasProcessor",
                "urn:machine-part:cc",
            ),
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
        if let Some(PropertyValue::List(items)) = second
            .entities
            .iter_mut()
            .find_map(|e| e.properties.get_mut("machineRole"))
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
                r#"{"relationship":"hasProcessor","source":"urn:machine:serial:aa","#,
                r#""target":"urn:machine-part:cc"}],"schema":"inventory-records/v1"}"#
            )
        );
        assert_eq!(
            canonical_digest(bytes.as_bytes()),
            canonical_digest(canonical_json(&second).as_bytes())
        );
        assert!(canonical_digest(b"").starts_with("sha256:e3b0c44298fc1c14"));
        assert_eq!(canonical_digest(b"abc").len(), 71);
    }

    // spec: MEDULLA-MACHINE-R006.3
    #[test]
    fn canonical_text_sorts_keys_at_every_depth_and_escapes_text() {
        let value =
            serde_json::json!({"b": {"z": 1, "a": [{"y": true, "x": null}]}, "a": "q\"\n\u{e9}"});
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
        let parts = [
            nic(Some("aa:bb:cc:dd:ee:01"), Some("eth0")),
            nic(None, Some("eth1")),
            nic(None, None),
        ];
        let ids = component_ids(machine, ComponentKind::Nic, &parts);
        assert_eq!(
            ids[0],
            Some(component_id(
                machine,
                ComponentKind::Nic,
                "aa:bb:cc:dd:ee:01"
            ))
        );
        assert_eq!(
            ids[1],
            Some(component_id(machine, ComponentKind::Nic, "eth1"))
        );
        // `{}` sorts after `{"..."`, so the empty record is ordinal 2.
        assert_eq!(
            ids[2],
            Some(component_id(machine, ComponentKind::Nic, "ordinal:2"))
        );
        let bond = component_ids(machine, ComponentKind::Bond, &[BTreeMap::new()]);
        assert_eq!(bond, vec![None]);
        let cpu = component_ids(
            machine,
            ComponentKind::Processor,
            &[BTreeMap::new(), BTreeMap::new()],
        );
        assert_eq!(cpu.len(), 2);
        assert_eq!(ComponentKind::from_tag("nic"), Some(ComponentKind::Nic));
        assert_eq!(ComponentKind::from_tag("gpu"), None);
    }
    fn uuid_key() -> MachineKey {
        MachineKey {
            kind: MachineKeyKind::SystemUuid,
            value: "4c4c4544-0042-3510-8051-b8c04f4d4e32".into(),
        }
    }

    fn host_key(name: &str) -> MachineKey {
        MachineKey {
            kind: MachineKeyKind::Hostname,
            value: name.into(),
        }
    }

    // spec: MEDULLA-MACHINE-R006.3
    #[test]
    fn a_built_machine_passes_the_machine_shape_and_an_unshaped_one_is_refused() {
        let built = machine_entity(
            "t1",
            "alpha",
            &[host_key("alpha"), uuid_key()],
            MachineLifecycleState::Commissioned,
        )
        .expect("a machine with a valid key is produced");
        assert!(built.id.starts_with("urn:machine:uuid:"));
        assert_eq!(built.properties["identityTier"], text("uuid"));
        assert_eq!(
            built.properties["machineLifecycleState"],
            text("commissioned")
        );
        assert!(check_machine(&built).is_ok());
        // No valid key: the placeholder uuid is not a key.
        let placeholder = MachineKey {
            kind: MachineKeyKind::SystemUuid,
            value: "00000000-0000-0000-0000-000000000000".into(),
        };
        let refused = machine_entity("t1", "alpha", &[placeholder], MachineLifecycleState::Active)
            .unwrap_err();
        assert!(refused.message.contains("at least one :machineKey"));
        // Each constraint of the shape refuses its own violation.
        let broken: [(&str, PropertyValue, &str); 5] = [
            ("machineName", text(""), "non-empty :machineName"),
            ("identityTier", text("rack"), ":identityTier"),
            (
                "machineKey",
                PropertyValue::List(vec!["serial".into()]),
                "<kind>:<value>",
            ),
            (
                "systemUuid",
                PropertyValue::List(vec!["a".into(), "b".into()]),
                "at most one :systemUuid",
            ),
            (
                "machineLifecycleState",
                text("asleep"),
                ":machineLifecycleState",
            ),
        ];
        for (name, value, fragment) in broken {
            let mut entity = built.clone();
            entity.properties.insert(name.to_string(), value);
            let refusal = check_machine(&entity).unwrap_err();
            assert!(refusal.message.contains(fragment), "{name}: {refusal}");
        }
        let mut renamed = built.clone();
        renamed.id = "urn:machine:serial:not-hex".into();
        assert!(check_machine(&renamed)
            .unwrap_err()
            .message
            .contains("urn:machine:<tier>"));
    }

    // spec: MEDULLA-MACHINE-R006.3
    #[test]
    fn a_record_set_needs_one_owner_property_per_part_and_declared_relationships() {
        let machine = machine_entity(
            "t1",
            "alpha",
            &[host_key("alpha")],
            MachineLifecycleState::Active,
        )
        .unwrap();
        let part_id = component_id(&machine.id, ComponentKind::Accelerator, "0000:01:00.0");
        let mut set = InventoryRecordSet::new();
        set.entities = vec![
            machine.clone(),
            entity(&part_id, "Accelerator", &[("acceleratorKind", text("gpu"))]),
        ];
        assert!(set
            .check_shapes()
            .unwrap_err()
            .message
            .contains("this part has 0"));
        set.relationships = vec![relationship(
            &machine.id,
            ComponentKind::Accelerator.owner_relationship(),
            &part_id,
        )];
        assert!(set.check_shapes().is_ok());
        set.relationships[0].relationship = "hasComponent".into();
        assert!(set
            .check_shapes()
            .unwrap_err()
            .message
            .contains("this part has 0"));
        set.relationships[0].relationship = "hasProcessor".into();
        assert!(set.check_shapes().is_err());
        set.relationships[0].relationship = "ownsEverything".into();
        assert!(set.check_shapes().is_err());
        for kind in [
            ComponentKind::Processor,
            ComponentKind::Memory,
            ComponentKind::Storage,
            ComponentKind::Nic,
            ComponentKind::Bond,
            ComponentKind::Accelerator,
            ComponentKind::Management,
        ] {
            assert!(kind.owner_relationship().starts_with("has"));
            assert_ne!(kind.owner_relationship(), "hasComponent");
        }
    }
}
