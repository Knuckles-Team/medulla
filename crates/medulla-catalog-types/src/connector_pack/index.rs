//! What a connector publishes: one index over one archive.
//!
//! A pack is content-addressed end to end. The index names every entry by URI
//! and pins each of its sections as an `(offset, length, sha256)` triple into
//! one archive blob, so the engine can validate a section without reading the
//! rest and can prove the archive it read is the one the index describes.

use serde::{Deserialize, Serialize};

use super::annotations::PackAnnotations;
use medulla_types::{contract::BoundedVec, contract::Digest256, contract::ResourceId};

/// What one pack entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PackEntryKind {
    McpServer,
    Tool,
    Skill,
    Prompt,
    // One concrete MCP resource. Its URI is the upstream URI, not an
    // engine-invented `resource://` alias.
    Resource,
    // One MCP resource template. `uri` is the upstream URI template and the
    // input/output sections pin its argument/result schemas.
    ResourceTemplate,
    Ontology,
    Shapes,
    ModelProfile,
    A2aCard,
    // One supporting file of a skill (D11): a script, reference or asset the
    // skill's `SKILL.md` names by relative path. `name` is
    // `<skill>/<relative path>`, the URI `skill-file://<name>`, and the entry
    // references its skill's `SKILL.md`. A plain comment, like its siblings':
    // a doc comment would make schemars split this variant out of the enum
    // and the generated Python type would stop being an Enum.
    SkillFile,
    // One vocabulary crosswalk (MEDULLA-TYPED-PACKS-R071.1.3): the body is a
    // `VocabularyCrosswalk` JSON record. Plain comment, like its siblings'.
    Crosswalk,
    // One proposed canonical concept path (MEDULLA-TYPED-PACKS-R071.100): the
    // body is a `ConceptPathRecord` JSON record.
    ConceptPath,
}

/// Exact identity of the served MCP catalog snapshot this pack came from.
///
/// The pack digest binds this whole value. Consequently a stored resource can
/// never be mistaken for one observed under a different configuration,
/// connection or authorization scope even when its own body is byte-identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct McpCatalogSnapshotBinding {
    pub configuration_revision: u64,
    pub catalog_generation: u64,
    pub snapshot_digest: Digest256,
    pub child_connection_generation: u64,
    pub authorization_scope_digest: Digest256,
}

/// One byte range of the archive, pinned by its own digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackSection {
    pub offset: u64,
    pub length: u64,
    pub sha256: Digest256,
}

/// A reference from one entry to another, by URI and kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackRef {
    pub uri: String,
    pub kind: PackEntryKind,
}

/// One published entry of a pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackEntry {
    pub kind: PackEntryKind,
    pub uri: String,
    pub name: String,
    pub media_type: String,
    pub body: PackSection,
    #[serde(default)]
    pub input_schema: Option<PackSection>,
    #[serde(default)]
    pub output_schema: Option<PackSection>,
    #[serde(default)]
    pub annotations: PackAnnotations,
    #[serde(default)]
    pub references: BoundedVec<PackRef, 64>,
}

/// Where the archive lives and what it must hash to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackArchiveRef {
    /// The engine blob reference the archive was uploaded under.
    pub blob_digest: String,
    pub length: u64,
    pub sha256: Digest256,
}

/// The tool that produced this pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackProducer {
    pub name: String,
    pub version: String,
}

/// One connector's complete published surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConnectorPackIndex {
    pub schema_version: u16,
    pub connector: ResourceId,
    /// The MCP server entry itself. Every other entry hangs off it.
    pub server: PackEntry,
    pub server_package_version: String,
    pub archive: PackArchiveRef,
    pub entries: BoundedVec<PackEntry, { super::MAX_PACK_ENTRIES }>,
    pub producer: PackProducer,
    /// The exact served generation/digest observed by the catalog lister.
    pub catalog: McpCatalogSnapshotBinding,
    /// The digest of everything above, recomputed by the engine before
    /// anything is committed.
    pub pack_digest: Digest256,
}

#[cfg(test)]
mod kind_wire_tests {
    use super::PackEntryKind;

    // spec: MEDULLA-TYPED-PACKS-R101.10
    #[test]
    fn the_manifest_kind_is_not_on_the_wire() {
        assert!(serde_json::from_str::<PackEntryKind>("\"manifest\"").is_err());
        assert_eq!(
            serde_json::from_str::<PackEntryKind>("\"resource\"").unwrap(),
            PackEntryKind::Resource
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectorPackIndex, PackEntry, PackEntryKind};

    fn msgpack_round_trip<T>(value: &T) -> T
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        // The named MessagePack encoding `medulla_storage::encode_bounded`
        // writes for the import index size bound.
        let bytes = rmp_serde::to_vec_named(value).expect("encode");
        rmp_serde::from_slice(&bytes).expect("decode")
    }

    // spec: MEDULLA-TYPED-PACKS-R001.1
    #[test]
    fn index_entry_round_trip_and_refusals() {
        let index = crate::catalog_fixtures::contract_pack::index();
        assert_eq!(msgpack_round_trip(&index), index);
        let json = serde_json::to_value(&index).unwrap();
        assert_eq!(
            serde_json::from_value::<ConnectorPackIndex>(json.clone()).unwrap(),
            index
        );
        for entry in std::iter::once(&index.server).chain(index.entries.iter()) {
            assert_eq!(&msgpack_round_trip(entry), entry);
            assert_eq!(msgpack_round_trip(&entry.kind), entry.kind);
        }
        let kinds: std::collections::BTreeSet<PackEntryKind> = std::iter::once(index.server.kind)
            .chain(index.entries.iter().map(|entry| entry.kind))
            .collect();
        assert_eq!(kinds.len(), 13, "the fixture carries every entry kind");

        let mut unknown_kind = json.clone();
        unknown_kind["entries"][0]["kind"] = serde_json::json!("plugin");
        assert!(serde_json::from_value::<ConnectorPackIndex>(unknown_kind).is_err());
        let mut entry_wire = serde_json::to_value(&index.server).unwrap();
        entry_wire["kind"] = serde_json::json!("manifest");
        assert!(serde_json::from_value::<PackEntry>(entry_wire).is_err());

        for (field, bad) in [
            ("schema_version", serde_json::json!("1")),
            ("server_package_version", serde_json::json!(7)),
            ("pack_digest", serde_json::json!("not-a-digest")),
            ("entries", serde_json::json!({})),
            ("archive", serde_json::json!({ "length": 1 })),
        ] {
            let mut malformed = json.clone();
            malformed[field] = bad;
            assert!(
                serde_json::from_value::<ConnectorPackIndex>(malformed).is_err(),
                "a malformed `{field}` must refuse"
            );
        }
        let mut extra = json;
        extra["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ConnectorPackIndex>(extra).is_err());
    }
}
