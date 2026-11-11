//! Durable, keyed schema authority for one graph.
//!
//! Core sources are compiled into the engine and replaced only as one catalog
//! during an engine upgrade. Dynamic sources are graph-local operator, pack, or
//! ingestion attachments. Keeping the maps separate makes the 32-source quota
//! apply only to tenant/operator material and makes it impossible for a generic
//! detach request to remove an engine-owned source.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use medulla_types::contract::Digest256;

use super::LegacyIntegrityPolicy;

/// Closed upper bound for one binary's immutable catalog.  The current catalog
/// contains the aggregate document, foundation, 31 domain TBoxes (the world model
/// as its life, environment and nutrition modules), the core governance-shape
/// slice, the world-model shapes (35 artifacts, EH-364) and the swarm-topology
/// vocabulary and shapes (37, ST-1), the finance module and its shapes (39,
/// EH-411), the temporal bi-temporal-fact shapes (40, MERISTEM-SEMANTIC-R004), and
/// the virtual-graph vocabulary and shapes (42, MEDULLA-UNIFIED-DATA-PLANE-R037), and
/// the service-operations module (43, MEDULLA-TYPED-PACKS-R105.4), and the
/// metadata-catalog vocabulary and shapes (46, MEDULLA-TYPED-PACKS-R106.1), and the
/// data-platform vocabulary plus the data-platform, IT4IT-delivery, media and
/// retrieval domain modules (51, MEDULLA-DATAPLAT-VOCAB-R001.1 and
/// MEDULLA-TYPED-PACKS-R105.6 to R105.9), the commercial module with its shapes
/// companion (52, MEDULLA-SUPPLY-VOCAB-R003.13), the supply-chain vocabulary
/// (53, MEDULLA-SUPPLY-VOCAB-R001.1.2), and the connector-pack vocabulary with its
/// shapes companion (54, MEDULLA-TYPED-PACKS-R101.1 to R102.1), the government
/// and foundation shapes companions (55, 56, MEDULLA-CQ-CONN-R073.1, R074.1), and
/// the generated decision-contract ontology and shapes (57, MEDULLA-DECISION-ENGINE-R054.1).
/// It is deliberately independent of the dynamic
/// 32-source tenant quota, and equal to the wire bound
/// `medulla_catalog_types::graph_schema::MAX_CORE_GRAPH_SCHEMA_SOURCES`.
pub const MAX_CORE_SCHEMA_SOURCES: usize = 64;
pub const MAX_TOTAL_DYNAMIC_SCHEMA_BYTES: usize = 8 << 20;
pub const MAX_SCHEMA_DOCUMENT_TRIPLES: usize = 100_000;
pub const CORE_SOURCE_PREFIX: &str = "core:";
pub const OPERATOR_SOURCE_ID: &str = "operator";
const COMPOSED_DIGEST_DOMAIN: &[u8] = b"eg/graph-schema-sources/v1";
const CORE_SET_DIGEST_DOMAIN: &[u8] = b"eg/core-schema-catalog/v1";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum SchemaSourceOrigin {
    Core {
        module: String,
        version: u32,
        set_digest: Digest256,
    },
    Operator,
    Admin {
        name: String,
    },
    Pack {
        connector: String,
        record_id: String,
    },
    Ingestion {
        mapping: String,
        revision: u64,
    },
    /// EH-403: a governed candidate attached under `approved:<name>` by
    /// `GraphSchema.AttachApproved`, which verified `approval_lease_id` first.
    Approved {
        name: String,
        approval_lease_id: String,
        /// Digest of the shadow-ingest report the attach was admitted under.
        #[serde(default)]
        shadow_report_digest: Option<String>,
    },
    /// MEDULLA-TYPED-PACKS-R103.2: a connector-owned runtime ontology under
    /// `ontology:<connector>/<ontology_iri>`. `revision` is the active (and only
    /// composed) revision; `importer` is the principal bound by the first
    /// accepted revision; `history` is metadata only (no earlier bodies), the
    /// newest [`MAX_ONTOLOGY_REVISION_HISTORY`] entries.
    Ontology {
        connector: String,
        ontology_iri: String,
        revision: u64,
        importer: String,
        history: Vec<OntologyRevisionEntry>,
    },
}

pub use medulla_catalog_types::graph_schema::{
    OntologyRevisionEntry, MAX_ONTOLOGY_REVISION_HISTORY,
};

/// What a schema source's vocabulary may describe in a connector seed graph
/// (MEDULLA-TYPED-PACKS-R104.4). Only `Domain` vocabulary is bulk data and is
/// refused in a seed triple; the importer assigns it, an archive never carries it.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SchemaSourceRole {
    Identity,
    Catalog,
    Reference,
    #[default]
    Domain,
}

impl SchemaSourceRole {
    /// Whether a seed triple may use a predicate declared by a source of this role.
    pub fn allows_seed_predicates(self) -> bool {
        !matches!(self, Self::Domain)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphSchemaSource {
    pub origin: SchemaSourceOrigin,
    /// Seed-graph role; engine-owned core vocabulary is `Reference`, every
    /// other source starts as `Domain` until the importer says otherwise.
    #[serde(default)]
    pub role: SchemaSourceRole,
    pub shapes_ttl: Option<Arc<str>>,
    pub ontology_ttl: Option<Arc<str>>,
    pub shapes_sha256: Option<Digest256>,
    pub ontology_sha256: Option<Digest256>,
    /// Validated vocabulary crosswalks committed with this source
    /// (MEDULLA-TYPED-PACKS-R071.1.3). They are references between governed
    /// vocabularies, never schema axioms, so they are outside the composed digest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crosswalks: Vec<medulla_catalog_types::domain_pack::VocabularyCrosswalk>,
    /// Deterministic provenance time. Replicated handlers must never read the
    /// local wall clock; until the commit envelope exposes a consensus time,
    /// live attachments use the documented sentinel `0`.
    pub attached_at_ms: u64,
}

impl GraphSchemaSource {
    pub fn new(
        origin: SchemaSourceOrigin,
        shapes_ttl: Option<Arc<str>>,
        ontology_ttl: Option<Arc<str>>,
        attached_at_ms: u64,
    ) -> Result<Self, String> {
        // A pack head that intentionally withdraws its last semantic member is
        // retained as an empty Pack-origin tombstone.  Its record id is the
        // high-water mark that prevents a stale reproject from resurrecting
        // withdrawn schema.  Generic/admin/operator sources still need a real
        // document; an empty request is never their detach alias.
        if shapes_ttl.is_none()
            && ontology_ttl.is_none()
            && !matches!(&origin, SchemaSourceOrigin::Pack { .. })
        {
            return Err("a schema source needs shapes, ontology, or both".to_string());
        }
        let shapes_sha256 = shapes_ttl
            .as_deref()
            .map(|document| Digest256::sha256(document.as_bytes()));
        let ontology_sha256 = ontology_ttl
            .as_deref()
            .map(|document| Digest256::sha256(document.as_bytes()));
        let role = if matches!(&origin, SchemaSourceOrigin::Core { .. }) {
            SchemaSourceRole::Reference
        } else {
            SchemaSourceRole::Domain
        };
        Ok(Self {
            origin,
            role,
            shapes_ttl,
            ontology_ttl,
            shapes_sha256,
            ontology_sha256,
            crosswalks: Vec::new(),
            attached_at_ms,
        })
    }

    /// The same source carrying the crosswalks the importer validated.
    pub fn with_crosswalks(
        mut self,
        crosswalks: Vec<medulla_catalog_types::domain_pack::VocabularyCrosswalk>,
    ) -> Self {
        self.crosswalks = crosswalks;
        self
    }

    /// The same source with the seed-graph `role` the importer assigns.
    pub fn with_role(mut self, role: SchemaSourceRole) -> Self {
        self.role = role;
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_ontology_origin(&self.origin)?;
        if self.shapes_ttl.is_none()
            && self.ontology_ttl.is_none()
            && !matches!(&self.origin, SchemaSourceOrigin::Pack { .. })
        {
            return Err("schema source has no document".to_string());
        }
        for (label, document) in [
            ("shapes", self.shapes_ttl.as_deref()),
            ("ontology", self.ontology_ttl.as_deref()),
        ] {
            if document.is_some_and(|value| {
                value.len() > medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES
            }) {
                return Err(format!(
                    "schema source {label} exceeds {} bytes",
                    medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES
                ));
            }
        }
        validate_document_digest("shapes", self.shapes_ttl.as_deref(), self.shapes_sha256)?;
        validate_document_digest(
            "ontology",
            self.ontology_ttl.as_deref(),
            self.ontology_sha256,
        )
    }

    pub fn documents_equal(&self, other: &Self) -> bool {
        // Attachment time is provenance, not document identity. Compare each
        // document together with its recorded digest so this remains distinct
        // from full-record equality.
        self.origin == other.origin
            && self.crosswalks == other.crosswalks
            && same_schema_document(
                self.shapes_ttl.as_deref(),
                self.shapes_sha256,
                other.shapes_ttl.as_deref(),
                other.shapes_sha256,
            )
            && same_schema_document(
                self.ontology_ttl.as_deref(),
                self.ontology_sha256,
                other.ontology_ttl.as_deref(),
                other.ontology_sha256,
            )
    }

    pub fn byte_len(&self) -> usize {
        self.shapes_ttl.as_deref().map_or(0, str::len)
            + self.ontology_ttl.as_deref().map_or(0, str::len)
    }
}

fn same_schema_document(
    left_document: Option<&str>,
    left_digest: Option<Digest256>,
    right_document: Option<&str>,
    right_digest: Option<Digest256>,
) -> bool {
    left_document == right_document && left_digest == right_digest
}

/// A runtime-ontology record names its active revision as the newest history
/// entry, within the fixed history bound, with strictly increasing revisions.
fn validate_ontology_origin(origin: &SchemaSourceOrigin) -> Result<(), String> {
    let SchemaSourceOrigin::Ontology {
        revision,
        importer,
        history,
        ..
    } = origin
    else {
        return Ok(());
    };
    if importer.is_empty() || *revision == 0 {
        return Err("runtime ontology record needs an importer and a revision".to_string());
    }
    if history.is_empty() || history.len() > MAX_ONTOLOGY_REVISION_HISTORY {
        return Err(format!(
            "runtime ontology history must hold 1..={MAX_ONTOLOGY_REVISION_HISTORY} entries"
        ));
    }
    if history
        .windows(2)
        .any(|pair| pair[1].revision <= pair[0].revision)
        || history.last().map(|entry| entry.revision) != Some(*revision)
    {
        return Err("runtime ontology history does not end at the active revision".to_string());
    }
    Ok(())
}

fn validate_document_digest(
    label: &str,
    document: Option<&str>,
    digest: Option<Digest256>,
) -> Result<(), String> {
    match (document, digest) {
        (None, None) => Ok(()),
        (Some(document), Some(digest)) if Digest256::sha256(document.as_bytes()) == digest => {
            Ok(())
        }
        (Some(_), Some(_)) => Err(format!("schema source {label} digest mismatch")),
        _ => Err(format!(
            "schema source {label} document/digest presence differs"
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphSchemaSources {
    /// Immutable engine-owned catalog. Not counted against `dynamic`'s quota.
    pub core: BTreeMap<String, GraphSchemaSource>,
    /// Operator/admin/pack/ingestion sources, bounded to 32 entries.
    pub dynamic: BTreeMap<String, GraphSchemaSource>,
}

impl Default for GraphSchemaSources {
    fn default() -> Self {
        Self {
            core: current_core_catalog().clone(),
            dynamic: BTreeMap::new(),
        }
    }
}

impl GraphSchemaSources {
    pub fn with_dynamic(dynamic: BTreeMap<String, GraphSchemaSource>) -> Result<Self, String> {
        let value = Self {
            core: current_core_catalog().clone(),
            dynamic,
        };
        value.validate()?;
        Ok(value)
    }

    /// Replace every persisted core module with this binary's one coherent set.
    /// Dynamic bytes remain untouched. This is the atomic engine-upgrade rule:
    /// no graph can observe a mixture of core catalog versions.
    pub fn reconcile_current_core(&mut self) {
        self.core = current_core_catalog().clone();
    }

    pub fn reconciled_current_core(mut self) -> Result<Self, String> {
        self.reconcile_current_core();
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_core_sources(&self.core)?;
        validate_dynamic_sources(&self.dynamic)
    }

    pub fn composed_digest(&self) -> Digest256 {
        let mut fields = Vec::<Vec<u8>>::new();
        for (scope, sources) in [
            (b"core".as_slice(), &self.core),
            (b"dynamic", &self.dynamic),
        ] {
            for (source_id, source) in sources {
                fields.push(scope.to_vec());
                fields.push(source_id.as_bytes().to_vec());
                fields.push(origin_bytes(&source.origin));
                fields.push(optional_digest_bytes(source.shapes_sha256));
                fields.push(optional_digest_bytes(source.ontology_sha256));
            }
        }
        let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
        Digest256::framed(COMPOSED_DIGEST_DOMAIN, &refs)
            .expect("bounded schema fields always fit digest framing")
    }

    pub fn all(&self) -> impl Iterator<Item = (&str, &GraphSchemaSource)> {
        self.core
            .iter()
            .chain(self.dynamic.iter())
            .map(|(id, source)| (id.as_str(), source))
    }

    pub fn shapes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.all()
            .filter_map(|(id, source)| source.shapes_ttl.as_deref().map(|document| (id, document)))
    }

    pub fn ontologies(&self) -> impl Iterator<Item = (&str, &str)> {
        self.all().filter_map(|(id, source)| {
            source
                .ontology_ttl
                .as_deref()
                .map(|document| (id, document))
        })
    }

    pub fn has_shapes(&self) -> bool {
        self.shapes().next().is_some()
    }

    pub fn attach_dynamic(
        &mut self,
        source_id: String,
        source: GraphSchemaSource,
    ) -> Result<bool, String> {
        validate_dynamic_identity(&source_id, &source.origin)?;
        source.validate()?;
        if self
            .dynamic
            .get(&source_id)
            .is_some_and(|current| current.documents_equal(&source))
        {
            return Ok(false);
        }
        if let Some(current) = self.dynamic.get(&source_id) {
            validate_replacement_progression(current, &source)?;
        }
        let previous = self.dynamic.insert(source_id.clone(), source);
        if let Err(error) = self.validate() {
            match previous {
                Some(previous) => {
                    self.dynamic.insert(source_id, previous);
                }
                None => {
                    self.dynamic.remove(&source_id);
                }
            }
            return Err(error);
        }
        Ok(true)
    }

    pub fn detach_dynamic(&mut self, source_id: &str) -> bool {
        self.dynamic.remove(source_id).is_some()
    }
}

fn validate_core_sources(core: &BTreeMap<String, GraphSchemaSource>) -> Result<(), String> {
    if core.len() > MAX_CORE_SCHEMA_SOURCES {
        return Err("core schema catalog exceeds its closed bound".to_string());
    }
    if core != current_core_catalog() {
        return Err("core schema catalog differs from this binary's immutable catalog".to_string());
    }

    let expected_set = current_core_set_digest();
    let mut modules = BTreeSet::new();
    for (source_id, source) in core {
        let (module, _) = validate_core_source(source_id, source, expected_set)?;
        if !modules.insert(module.to_string()) {
            return Err(format!(
                "multiple active core versions for module '{module}'"
            ));
        }
    }
    Ok(())
}

fn validate_core_source<'a>(
    source_id: &'a str,
    source: &GraphSchemaSource,
    expected_set: Digest256,
) -> Result<(&'a str, u32), String> {
    let Some((module, version)) = parse_core_source_id(source_id) else {
        return Err(format!("invalid core schema source id '{source_id}'"));
    };
    match &source.origin {
        SchemaSourceOrigin::Core {
            module: origin_module,
            version: origin_version,
            set_digest,
        } if origin_module == module
            && *origin_version == version
            && *set_digest == expected_set => {}
        _ => {
            return Err(format!(
                "core schema source '{source_id}' origin is invalid"
            ));
        }
    }
    source.validate()?;
    Ok((module, version))
}

fn validate_dynamic_sources(dynamic: &BTreeMap<String, GraphSchemaSource>) -> Result<(), String> {
    if dynamic.len() > medulla_catalog_types::graph_schema::MAX_GRAPH_SCHEMA_SOURCES {
        return Err("dynamic schema source count exceeds 32".to_string());
    }
    let mut total = 0usize;
    for (source_id, source) in dynamic {
        validate_dynamic_identity(source_id, &source.origin)?;
        source.validate()?;
        reject_pack_or_ingestion_approval_claim(source_id, source)?;
        total = total
            .checked_add(source.byte_len())
            .ok_or_else(|| "dynamic schema byte count overflowed".to_string())?;
    }
    if total > MAX_TOTAL_DYNAMIC_SCHEMA_BYTES {
        return Err(format!(
            "dynamic schema documents exceed {MAX_TOTAL_DYNAMIC_SCHEMA_BYTES} bytes"
        ));
    }
    Ok(())
}

/// MEDULLA-SRC-AUTH-R003.2: the virtual-graph `approved` flag is an operator
/// assertion. A `Pack` or `Ingestion` origin attaching a document that carries
/// a truthy `vg:approved` triple is refused at the attach seam with the typed
/// code `APPROVAL_CLAIM_REFUSED`; the same document under an `Operator`,
/// `Admin` or `Approved` origin is not a claim and may attach.
fn reject_pack_or_ingestion_approval_claim(
    source_id: &str,
    source: &GraphSchemaSource,
) -> Result<(), String> {
    let claim_origin = matches!(
        source.origin,
        SchemaSourceOrigin::Pack { .. } | SchemaSourceOrigin::Ingestion { .. }
    );
    if !claim_origin {
        return Ok(());
    }
    for (label, document) in [
        ("shapes", source.shapes_ttl.as_deref()),
        ("ontology", source.ontology_ttl.as_deref()),
    ] {
        if document.is_some_and(document_claims_virtual_graph_approval) {
            return Err(format!(
                "APPROVAL_CLAIM_REFUSED: {label} document of '{source_id}' asserts \
                 vg:approved true; approval is an operator assertion and may not ride a \
                 pack or ingestion origin"
            ));
        }
    }
    Ok(())
}

/// Conservative line scan for a truthy `vg:approved` triple. This crate has no
/// RDF parser, so the scan recognises the predicate as the full IRI or as any
/// `@prefix` label that resolves to the virtual-graph namespace, and the value
/// as the first term after the predicate (falling through to the next line
/// across a property-list continuation). A comment never starts inside an IRI,
/// so only a `#` at line start or after whitespace terminates a line.
fn document_claims_virtual_graph_approval(document: &str) -> bool {
    const APPROVED: &str = "http://knuckles.team/kg/virtual-graph#approved";
    let lines: Vec<&str> = document.lines().collect();
    let mut needles: Vec<String> = vec![format!("<{APPROVED}>")];
    for (label, iri) in prefix_bindings(&lines) {
        if format!("{iri}approved") == APPROVED {
            needles.push(format!("{label}:approved"));
        }
    }
    for (index, line) in lines.iter().enumerate() {
        let text = strip_line_comment(line);
        for needle in &needles {
            for position in text
                .match_indices(needle.as_str())
                .map(|(position, _)| position)
            {
                let before = &text[..position];
                let before_ok = match before.chars().next_back() {
                    None => true,
                    Some(c) => c.is_whitespace() || c == '(',
                };
                if !before_ok {
                    continue;
                }
                let after = &text[position + needle.len()..];
                let after_ok = match after.chars().next() {
                    None => true,
                    Some(c) => c.is_whitespace() || c == ';' || c == '.',
                };
                if !after_ok {
                    continue;
                }
                let rest = after.trim_start();
                let value = if rest.is_empty() || rest.starts_with(';') || rest.starts_with('.') {
                    // The predicate ends the line: across a property-list
                    // continuation the value, if any, starts the next line.
                    next_line_first_term(&lines[index + 1..])
                } else {
                    Some(rest.split([';', '.']).next().unwrap_or("").trim())
                };
                if value.is_some_and(is_truthy_approval_literal) {
                    return true;
                }
            }
        }
    }
    false
}

/// `@prefix label: <iri> .` bindings declared in the document.
fn prefix_bindings(lines: &[&str]) -> BTreeMap<String, String> {
    let mut prefixes: BTreeMap<String, String> = BTreeMap::new();
    for line in lines {
        let Some(declaration) = strip_line_comment(line)
            .trim_start()
            .strip_prefix("@prefix")
        else {
            continue;
        };
        let Some((label, rest)) = declaration.trim_start().split_once(':') else {
            continue;
        };
        let label = label.trim();
        let iri = rest
            .trim_start()
            .strip_prefix('<')
            .and_then(|value| value.split_once('>').map(|(iri, _)| iri))
            .unwrap_or("");
        if !label.is_empty() && !iri.is_empty() {
            prefixes.insert(label.to_string(), iri.to_string());
        }
    }
    prefixes
}

/// The first term of the first non-blank, non-comment line after a
/// property-list continuation.
fn next_line_first_term<'a>(lines: &'a [&'a str]) -> Option<&'a str> {
    for line in lines {
        let text = strip_line_comment(line).trim();
        if text.is_empty() {
            continue;
        }
        return Some(text.split([';', '.']).next().unwrap_or("").trim());
    }
    None
}

/// Drop a trailing Turtle comment. A `#` inside an IRI (such as the
/// virtual-graph namespace) is not a comment start; only a `#` at line start
/// or after whitespace is.
fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'#' && (index == 0 || bytes[index - 1].is_ascii_whitespace()) {
            return &line[..index];
        }
    }
    line
}

/// `true`, `1` and their quoted forms, with or without a datatype suffix.
fn is_truthy_approval_literal(term: &str) -> bool {
    let base = term.split("^^").next().unwrap_or(term);
    matches!(base, "true" | "1" | "\"true\"" | "\"1\"")
}

fn validate_dynamic_identity(source_id: &str, origin: &SchemaSourceOrigin) -> Result<(), String> {
    if source_id.is_empty()
        || source_id.len() > medulla_catalog_types::graph_schema::MAX_SCHEMA_SOURCE_ID_BYTES
        || source_id.chars().any(char::is_control)
    {
        return Err(format!(
            "schema source id must be printable and 1..={} bytes",
            medulla_catalog_types::graph_schema::MAX_SCHEMA_SOURCE_ID_BYTES
        ));
    }
    if origin_owns_key(source_id, origin) {
        Ok(())
    } else {
        Err(format!(
            "schema source '{source_id}' does not match its authority origin"
        ))
    }
}

/// Whether `origin` is the authority its key prefix asserts (`admin:<name>`,
/// `pack:<connector>`, `ingest:<mapping>`, `approved:<name>`,
/// `ontology:<connector>/<iri>`, `operator`).
fn origin_owns_key(source_id: &str, origin: &SchemaSourceOrigin) -> bool {
    match origin {
        SchemaSourceOrigin::Core { .. } => false,
        SchemaSourceOrigin::Operator => source_id == OPERATOR_SOURCE_ID,
        SchemaSourceOrigin::Admin { name } => keyed(source_id, "admin:", name),
        SchemaSourceOrigin::Pack {
            connector,
            record_id,
        } => keyed(source_id, "pack:", connector) && !record_id.is_empty(),
        SchemaSourceOrigin::Ingestion { mapping, .. } => keyed(source_id, "ingest:", mapping),
        SchemaSourceOrigin::Approved {
            name,
            approval_lease_id,
            ..
        } => keyed(source_id, "approved:", name) && !approval_lease_id.is_empty(),
        SchemaSourceOrigin::Ontology {
            connector,
            ontology_iri,
            importer,
            ..
        } => {
            !connector.is_empty()
                && !ontology_iri.is_empty()
                && !importer.is_empty()
                && source_id == format!("ontology:{connector}/{ontology_iri}")
        }
    }
}

/// `source_id` is exactly `<prefix><name>` with a non-empty name.
fn keyed(source_id: &str, prefix: &str, name: &str) -> bool {
    source_id
        .strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty() && suffix == name)
}

fn validate_replacement_progression(
    current: &GraphSchemaSource,
    replacement: &GraphSchemaSource,
) -> Result<(), String> {
    let advances = match (&current.origin, &replacement.origin) {
        (
            SchemaSourceOrigin::Pack {
                connector: current_connector,
                record_id: current_record,
            },
            SchemaSourceOrigin::Pack {
                connector: replacement_connector,
                record_id: replacement_record,
            },
        ) => {
            current_connector == replacement_connector
                && pack_record_advances(current_record, replacement_record)
        }
        (
            SchemaSourceOrigin::Ingestion {
                mapping: current_mapping,
                revision: current_revision,
            },
            SchemaSourceOrigin::Ingestion {
                mapping: replacement_mapping,
                revision: replacement_revision,
            },
        ) => current_mapping == replacement_mapping && replacement_revision > current_revision,
        (
            SchemaSourceOrigin::Ontology {
                revision: current_revision,
                importer: current_importer,
                ..
            },
            SchemaSourceOrigin::Ontology {
                revision: replacement_revision,
                importer: replacement_importer,
                ..
            },
        ) => current_importer == replacement_importer && replacement_revision > current_revision,
        (SchemaSourceOrigin::Pack { .. }, _)
        | (SchemaSourceOrigin::Ingestion { .. }, _)
        | (SchemaSourceOrigin::Ontology { .. }, _) => false,
        _ => true,
    };
    if advances {
        Ok(())
    } else {
        Err("SCHEMA_SOURCE_REGRESSION: imported schema source did not advance".to_string())
    }
}

fn pack_record_advances(current: &str, replacement: &str) -> bool {
    match (
        pack_record_sequence(current),
        pack_record_sequence(replacement),
    ) {
        (
            Some((current_prefix, current_sequence)),
            Some((replacement_prefix, replacement_sequence)),
        ) if current_prefix == replacement_prefix => replacement_sequence > current_sequence,
        _ => replacement > current,
    }
}

fn pack_record_sequence(record_id: &str) -> Option<(&str, u64)> {
    let (prefix_and_sequence, _digest) = record_id.rsplit_once(':')?;
    let (prefix, sequence) = prefix_and_sequence.rsplit_once(':')?;
    Some((prefix, sequence.parse().ok()?))
}

pub fn lift_v2_integrity_policy(
    policy: Option<LegacyIntegrityPolicy>,
) -> Result<Arc<GraphSchemaSources>, String> {
    let mut dynamic = BTreeMap::new();
    if let Some(policy) = policy {
        let source = GraphSchemaSource::new(
            SchemaSourceOrigin::Operator,
            Some(Arc::from(policy.shapes_ttl)),
            None,
            0,
        )
        .expect("a v2 policy always carries a shapes document");
        dynamic.insert(OPERATOR_SOURCE_ID.to_string(), source);
    }
    GraphSchemaSources::with_dynamic(dynamic).map(Arc::new)
}

fn current_core_catalog() -> &'static BTreeMap<String, GraphSchemaSource> {
    static CATALOG: OnceLock<BTreeMap<String, GraphSchemaSource>> = OnceLock::new();
    CATALOG.get_or_init(build_core_catalog)
}

pub fn current_core_set_digest() -> Digest256 {
    static DIGEST: OnceLock<Digest256> = OnceLock::new();
    *DIGEST.get_or_init(|| {
        let specs = core_specs();
        let mut fields = Vec::<Vec<u8>>::new();
        for spec in specs {
            fields.push(spec.module.as_bytes().to_vec());
            fields.push(spec.version.to_be_bytes().to_vec());
            fields.push(optional_digest_bytes(
                spec.shapes.map(|v| Digest256::sha256(v.as_bytes())),
            ));
            fields.push(optional_digest_bytes(
                spec.ontology.map(|v| Digest256::sha256(v.as_bytes())),
            ));
        }
        let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
        Digest256::framed(CORE_SET_DIGEST_DOMAIN, &refs)
            .expect("closed core catalog always fits digest framing")
    })
}

fn build_core_catalog() -> BTreeMap<String, GraphSchemaSource> {
    let set_digest = current_core_set_digest();
    core_specs()
        .iter()
        .map(|spec| {
            let source_id = format!("core:{}@{}", spec.module, spec.version);
            let source = GraphSchemaSource::new(
                SchemaSourceOrigin::Core {
                    module: spec.module.to_string(),
                    version: spec.version,
                    set_digest,
                },
                spec.shapes.map(Arc::from),
                spec.ontology.map(Arc::from),
                0,
            )
            .expect("closed core catalog entries always carry a document");
            (source_id, source)
        })
        .collect()
}

struct CoreSpec {
    module: &'static str,
    version: u32,
    shapes: Option<&'static str>,
    ontology: Option<&'static str>,
}

fn core_specs() -> &'static [CoreSpec] {
    const SPECS: &[CoreSpec] = &[
        CoreSpec {
            module: "catalog",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/catalog-v1.ttl"
            )),
        },
        CoreSpec {
            module: "foundation",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/core-foundation-v1.ttl"
            )),
        },
        // The core-foundation SHACL shapes are their own core source, the
        // same pattern as orchestration-shapes and finance-shapes.
        CoreSpec {
            module: "foundation-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/core-foundation-v1.shapes.ttl"
            )),
            ontology: None,
        },
        CoreSpec {
            module: "capability",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/capability-v1.ttl"
            )),
        },
        CoreSpec {
            module: "archimate",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/archimate-v1.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/archimate-v1.ttl"
            )),
        },
        CoreSpec {
            module: "enterprise",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/enterprise-v1.ttl"
            )),
        },
        // The enterprise SHACL shapes are their own core source, the same
        // pattern as orchestration-shapes and finance-shapes.
        CoreSpec {
            module: "enterprise-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/enterprise-v1.shapes.ttl"
            )),
            ontology: None,
        },
        CoreSpec {
            module: "governance-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/governance-core-v1.shapes.ttl"
            )),
            ontology: None,
        },
        CoreSpec {
            module: "a2a",
            version: 1,
            shapes: None,
            ontology: Some(include_str!("../../../medulla-core/ontology/a2a-v1.ttl")),
        },
        CoreSpec {
            module: "action",
            version: 1,
            shapes: None,
            ontology: Some(include_str!("../../../medulla-core/ontology/action-v1.ttl")),
        },
        CoreSpec {
            module: "argumentation",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/argumentation-v1.ttl"
            )),
        },
        CoreSpec {
            module: "calendar",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/calendar-v1.ttl"
            )),
        },
        CoreSpec {
            module: "company",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/company-v1.ttl"
            )),
        },
        CoreSpec {
            module: "company_infra",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/company_infra-v1.ttl"
            )),
        },
        CoreSpec {
            module: "concepts",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/concepts-v1.ttl"
            )),
        },
        CoreSpec {
            module: "dataplatform",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/dataplatform-v1.ttl"
            )),
        },
        CoreSpec {
            module: "documentation",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/documentation-v1.ttl"
            )),
        },
        // MEDULLA-CQ-CONN-R071.1: the documentation shapes companion, the same
        // pattern as orchestration-shapes and finance-shapes.
        CoreSpec {
            module: "documentation-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/documentation-v1.shapes.ttl"
            )),
            ontology: None,
        },
        CoreSpec {
            module: "energy_geopolitics",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/energy_geopolitics-v1.ttl"
            )),
        },
        CoreSpec {
            module: "government",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/government-v1.ttl"
            )),
        },
        // The government SHACL shapes are their own core source, the same
        // pattern as orchestration-shapes and finance-shapes.
        CoreSpec {
            module: "government-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/government-v1.shapes.ttl"
            )),
            ontology: None,
        },
        CoreSpec {
            module: "harness",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/harness-v1.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/harness-v1.ttl"
            )),
        },
        CoreSpec {
            module: "hr",
            version: 1,
            shapes: None,
            ontology: Some(include_str!("../../../medulla-core/ontology/hr-v1.ttl")),
        },
        CoreSpec {
            module: "identity",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/identity-v1.ttl"
            )),
        },
        // The identity SHACL shapes are their own core source, the same
        // pattern as orchestration-shapes and finance-shapes.
        CoreSpec {
            module: "identity-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/identity-v1.shapes.ttl"
            )),
            ontology: None,
        },
        CoreSpec {
            module: "infrastructure",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/infrastructure-v1.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/infrastructure-v1.ttl"
            )),
        },
        CoreSpec {
            module: "it4it_delivery",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/it4it_delivery-v1.ttl"
            )),
        },
        CoreSpec {
            module: "medical",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/medical-v1.ttl"
            )),
        },
        CoreSpec {
            module: "media",
            version: 1,
            shapes: None,
            ontology: Some(include_str!("../../../medulla-core/ontology/media-v1.ttl")),
        },
        CoreSpec {
            module: "native_source_connector",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/native_source_connector-v1.ttl"
            )),
        },
        CoreSpec {
            module: "orchestration",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/orchestration-v1.ttl"
            )),
        },
        // The orchestration SHACL shapes are their own core source, the same
        // pattern as swarm-topology-shapes and finance-shapes.
        CoreSpec {
            module: "orchestration-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/orchestration-v1.shapes.ttl"
            )),
            ontology: None,
        },
        // MEDULLA-ORCH-VOCAB-R811.5: the core state-machine vocabulary and its
        // shapes, which every pack machine extends.
        CoreSpec {
            module: "state_machines",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/state-machines-v1.ttl"
            )),
        },
        CoreSpec {
            module: "state_machines-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/state-machines-v1.shapes.ttl"
            )),
            ontology: None,
        },
        CoreSpec {
            module: "personal",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/personal-v1.ttl"
            )),
        },
        CoreSpec {
            module: "process_intelligence",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/process_intelligence-v1.ttl"
            )),
        },
        CoreSpec {
            module: "retrieval",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/retrieval-v1.ttl"
            )),
        },
        // MEDULLA-SDLC-R001.1: the lifecycle edge shapes companion, the same
        // pattern as sdlc_lifecycle and documentation-shapes.
        CoreSpec {
            module: "sdd",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/sdd-v1.shapes.ttl"
            )),
            ontology: Some(include_str!("../../../medulla-core/ontology/sdd-v1.ttl")),
        },
        CoreSpec {
            module: "sdlc_lifecycle",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/sdlc_lifecycle-v1.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/sdlc_lifecycle-v1.ttl"
            )),
        },
        CoreSpec {
            module: "service_operations",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/service_operations-v1.ttl"
            )),
        },
        CoreSpec {
            module: "software",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/software-v1.ttl"
            )),
        },
        CoreSpec {
            module: "system",
            version: 1,
            shapes: None,
            ontology: Some(include_str!("../../../medulla-core/ontology/system-v1.ttl")),
        },
        CoreSpec {
            module: "trm",
            version: 1,
            shapes: None,
            ontology: Some(include_str!("../../../medulla-core/ontology/trm-v1.ttl")),
        },
        CoreSpec {
            module: "life",
            version: 1,
            shapes: None,
            ontology: Some(include_str!("../../../medulla-core/ontology/life-v1.ttl")),
        },
        CoreSpec {
            module: "environment",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/environment-v1.ttl"
            )),
        },
        CoreSpec {
            module: "nutrition",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/nutrition-v1.ttl"
            )),
        },
        CoreSpec {
            module: "world-model-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/world_model-v1.shapes.ttl"
            )),
            ontology: None,
        },
        // SWARM-TOPOLOGY-DECIDE-DESIGN §4 (ST-1): topology classes, slot roles, stop
        // rules and task-shape admissibility, with the template-projection shapes.
        // Authored by agent-utilities, owned here (no AU .ttl, RF-ADR-009).
        CoreSpec {
            module: "swarm-topology",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/swarm_topology-v1.ttl"
            )),
        },
        CoreSpec {
            module: "swarm-topology-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/swarm_topology-v1.shapes.ttl"
            )),
            ontology: None,
        },
        // MEDULLA-PLACEMENT-R001.5: the placement target vocabulary and its
        // four-fact shapes, one core source beside the swarm-topology pair.
        CoreSpec {
            module: "placement",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/placement.shapes.ttl"
            )),
            ontology: Some(include_str!("../../../medulla-core/ontology/placement.ttl")),
        },
        CoreSpec {
            module: "finance",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/finance-v1.ttl"
            )),
        },
        CoreSpec {
            module: "finance-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/finance-v1.shapes.ttl"
            )),
            ontology: None,
        },
        CoreSpec {
            module: "worldview",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/worldview-v1.ttl"
            )),
        },
        // MERISTEM-SEMANTIC-R004: bi-temporal fact invariants (validity window,
        // contradiction/superseded-closure) over the engine's own
        // :TemporalFact vocabulary (core-foundation@1). Shapes-only, same
        // pattern as governance-shapes/world-model-shapes.
        CoreSpec {
            module: "temporal-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/temporal-v1.shapes.ttl"
            )),
            ontology: None,
        },
        // The metadata catalog: assets, owners and stewards, lineage, glossary and
        // the Agent Library (MEDULLA-TYPED-PACKS-R106.1, MEDULLA-DECISION-ENGINE-R814.1.1).
        CoreSpec {
            module: "metadata_catalog",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/metadata_catalog-v1.ttl"
            )),
        },
        CoreSpec {
            module: "metadata_catalog-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/metadata_catalog-v1.shapes.ttl"
            )),
            ontology: None,
        },
        CoreSpec {
            module: "virtual-graph",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/virtual_graph-v1.ttl"
            )),
        },
        // MEDULLA-DATAPLAT-VOCAB-R001.4: shared data-platform terms are one immutable
        // core source and import the virtual-graph attachment vocabulary.
        // The SHACL shapes companion (MEDULLA-DATAPLAT-VOCAB-R002.4+, including the
        // closed secret-metadata shapes of MEDULLA-DATAPLAT-VOCAB-R005.2) is wired
        // into the SAME source's shapes field, as the commercial module does
        // (MEDULLA-SUPPLY-VOCAB-R003.14) — one registered artifact per module.
        CoreSpec {
            module: "data-platform",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/data_platform-v1.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/data_platform-v1.ttl"
            )),
        },
        CoreSpec {
            module: "virtual-graph-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/virtual_graph-v1.shapes.ttl"
            )),
            ontology: None,
        },
        // MEDULLA-SUPPLY-VOCAB-R003.13: the commercial module, registered once, with
        // its shapes companion wired into the same CoreSpec shapes field
        // (R003.14) instead of a separate core source.
        CoreSpec {
            module: "commercial",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/commercial-v1.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/commercial-v1.ttl"
            )),
        },
        CoreSpec {
            module: "supply_chain",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/supply_chain-v1.ttl"
            )),
        },
        // MEDULLA-TYPED-PACKS-R101.1/R102.1: the connector-pack ingest-contract and
        // surface vocabulary, registered once, with its shapes companion wired
        // into the same CoreSpec shapes field (the commercial pattern,
        // MEDULLA-SUPPLY-VOCAB-R003.13/R003.14).
        CoreSpec {
            module: "connector-pack",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/connector_pack-v1.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/connector_pack-v1.ttl"
            )),
        },
        // MEDULLA-DECISION-ENGINE-R054.1: the decision contract's ontology and
        // shapes, generated from its JSON schemas by
        // `medulla_decision_types::ontology`, rendered by `build.rs` into
        // `OUT_DIR` (MEDULLA-SDLC-R020.1: never committed).
        CoreSpec {
            module: "decision",
            version: 1,
            shapes: Some(include_str!(concat!(
                env!("OUT_DIR"),
                "/decision.shapes.ttl"
            ))),
            ontology: Some(include_str!(concat!(env!("OUT_DIR"), "/decision.ttl"))),
        },
    ];
    SPECS
}

fn parse_core_source_id(source_id: &str) -> Option<(&str, u32)> {
    let body = source_id.strip_prefix(CORE_SOURCE_PREFIX)?;
    let (module, version) = body.rsplit_once('@')?;
    if module.is_empty() || version.is_empty() {
        return None;
    }
    Some((module, version.parse().ok()?))
}

fn optional_digest_bytes(digest: Option<Digest256>) -> Vec<u8> {
    match digest {
        Some(value) => {
            let mut bytes = Vec::with_capacity(33);
            bytes.push(1);
            bytes.extend_from_slice(value.as_bytes());
            bytes
        }
        None => vec![0],
    }
}

fn origin_bytes(origin: &SchemaSourceOrigin) -> Vec<u8> {
    match origin {
        SchemaSourceOrigin::Core {
            module,
            version,
            set_digest,
        } => {
            let mut value = format!("core\0{module}\0{version}\0").into_bytes();
            value.extend_from_slice(set_digest.as_bytes());
            value
        }
        SchemaSourceOrigin::Operator => b"operator".to_vec(),
        SchemaSourceOrigin::Admin { name } => format!("admin\0{name}").into_bytes(),
        SchemaSourceOrigin::Pack {
            connector,
            record_id,
        } => format!("pack\0{connector}\0{record_id}").into_bytes(),
        SchemaSourceOrigin::Ingestion { mapping, revision } => {
            format!("ingestion\0{mapping}\0{revision}").into_bytes()
        }
        SchemaSourceOrigin::Approved {
            name,
            approval_lease_id,
            ..
        } => format!("approved\0{name}\0{approval_lease_id}").into_bytes(),
        SchemaSourceOrigin::Ontology {
            connector,
            ontology_iri,
            revision,
            importer,
            ..
        } => format!("ontology\0{connector}\0{ontology_iri}\0{revision}\0{importer}").into_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin_source(name: &str, attached_at_ms: u64) -> GraphSchemaSource {
        GraphSchemaSource::new(
            SchemaSourceOrigin::Admin {
                name: name.to_string(),
            },
            Some(Arc::<str>::from(format!(
                "@prefix sh: <http://www.w3.org/ns/shacl#> . # {name}"
            ))),
            None,
            attached_at_ms,
        )
        .unwrap()
    }

    #[test]
    fn core_catalog_has_one_version_per_module_and_is_outside_dynamic_quota() {
        let sources = GraphSchemaSources::default();
        sources.validate().unwrap();
        assert_eq!(sources.core.len(), 63);
        assert!(sources.dynamic.is_empty());
        assert!(sources.core.keys().all(|id| id.starts_with("core:")));
    }

    #[test]
    fn core_reconciliation_is_atomic_and_preserves_dynamic_bytes() {
        let mut sources = GraphSchemaSources::default();
        let operator = GraphSchemaSource::new(
            SchemaSourceOrigin::Operator,
            Some(Arc::from("@prefix sh: <http://www.w3.org/ns/shacl#> .")),
            None,
            0,
        )
        .unwrap();
        sources.dynamic.insert("operator".into(), operator.clone());
        sources.core.clear();
        sources.reconcile_current_core();
        assert_eq!(sources.dynamic.get("operator"), Some(&operator));
        assert_eq!(sources.core, current_core_catalog().clone());
    }

    #[test]
    fn document_digest_corruption_is_refused() {
        let mut source =
            GraphSchemaSource::new(SchemaSourceOrigin::Operator, Some(Arc::from("a")), None, 0)
                .unwrap();
        source.shapes_sha256 = Some(Digest256::sha256(b"b"));
        assert!(source.validate().unwrap_err().contains("digest mismatch"));
    }

    // spec: MEDULLA-TYPED-PACKS-R061
    #[test]
    fn low_level_source_seam_enforces_the_document_bound() {
        let oversized =
            "x".repeat(medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES + 1);
        let source = GraphSchemaSource::new(
            SchemaSourceOrigin::Admin {
                name: "oversized".to_string(),
            },
            Some(Arc::from(oversized)),
            None,
            0,
        )
        .unwrap();
        let mut sources = GraphSchemaSources::default();
        assert!(sources
            .attach_dynamic("admin:oversized".to_string(), source)
            .unwrap_err()
            .contains("exceeds"));
        assert!(sources.dynamic.is_empty());
    }

    // spec: MEDULLA-TYPED-PACKS-R061
    #[test]
    fn dynamic_quota_excludes_the_immutable_core_catalog() {
        let mut sources = GraphSchemaSources::default();
        for index in 0..medulla_catalog_types::graph_schema::MAX_GRAPH_SCHEMA_SOURCES {
            sources
                .attach_dynamic(
                    format!("admin:admin-{index:02}"),
                    admin_source(&format!("admin-{index:02}"), 0),
                )
                .unwrap();
        }
        assert_eq!(
            sources.all().count(),
            sources.core.len() + medulla_catalog_types::graph_schema::MAX_GRAPH_SCHEMA_SOURCES
        );
        let error = sources
            .attach_dynamic("admin:overflow".to_string(), admin_source("overflow", 0))
            .unwrap_err();
        assert!(error.contains("exceeds 32"));
        assert!(!sources.dynamic.contains_key("admin:overflow"));
    }

    // spec: MEDULLA-TYPED-PACKS-R051
    #[test]
    fn composed_digest_is_ordered_and_excludes_provenance_time() {
        let mut left = GraphSchemaSources::default();
        left.attach_dynamic("admin:a".to_string(), admin_source("a", 1))
            .unwrap();
        left.attach_dynamic("admin:b".to_string(), admin_source("b", 2))
            .unwrap();

        let mut right = GraphSchemaSources::default();
        right
            .attach_dynamic("admin:b".to_string(), admin_source("b", 200))
            .unwrap();
        right
            .attach_dynamic("admin:a".to_string(), admin_source("a", 100))
            .unwrap();

        assert_eq!(left.composed_digest(), right.composed_digest());
    }

    #[test]
    fn dynamic_seam_rejects_forged_or_mismatched_authority() {
        let mut sources = GraphSchemaSources::default();
        let core = GraphSchemaSource::new(
            SchemaSourceOrigin::Core {
                module: "forged".to_string(),
                version: 1,
                set_digest: current_core_set_digest(),
            },
            None,
            Some(Arc::from("@prefix owl: <http://www.w3.org/2002/07/owl#> .")),
            0,
        )
        .unwrap();
        assert!(sources
            .attach_dynamic("core:forged@1".to_string(), core)
            .is_err());
        assert!(sources
            .attach_dynamic("pack:wrong".to_string(), admin_source("wrong", 0))
            .is_err());
        let mismatched_pack = GraphSchemaSource::new(
            SchemaSourceOrigin::Pack {
                connector: "actual".to_string(),
                record_id: "r1".to_string(),
            },
            None,
            Some(Arc::from("@prefix owl: <http://www.w3.org/2002/07/owl#> .")),
            0,
        )
        .unwrap();
        assert!(sources
            .attach_dynamic("pack:claimed".to_string(), mismatched_pack)
            .is_err());
        assert!(sources
            .attach_dynamic("admin:claimed".to_string(), admin_source("actual", 0))
            .is_err());
        assert!(sources.dynamic.is_empty());
    }

    #[test]
    fn imported_replacements_are_monotonic_or_byte_identical() {
        let pack = |record_id: &str, document: &'static str| {
            GraphSchemaSource::new(
                SchemaSourceOrigin::Pack {
                    connector: "github".to_string(),
                    record_id: record_id.to_string(),
                },
                None,
                Some(Arc::from(document)),
                0,
            )
            .unwrap()
        };
        let ingest = |revision: u64, document: &'static str| {
            GraphSchemaSource::new(
                SchemaSourceOrigin::Ingestion {
                    mapping: "cmdb".to_string(),
                    revision,
                },
                None,
                Some(Arc::from(document)),
                0,
            )
            .unwrap()
        };
        let mut sources = GraphSchemaSources::default();
        let ontology_a = "@prefix owl: <http://www.w3.org/2002/07/owl#> . # a";
        let ontology_b = "@prefix owl: <http://www.w3.org/2002/07/owl#> . # b";
        assert!(sources
            .attach_dynamic("pack:github".to_string(), pack("r2", ontology_a))
            .unwrap());
        assert!(!sources
            .attach_dynamic("pack:github".to_string(), pack("r2", ontology_a))
            .unwrap());
        assert!(sources
            .attach_dynamic("pack:github".to_string(), pack("r1", ontology_b))
            .unwrap_err()
            .contains("SCHEMA_SOURCE_REGRESSION"));
        let numeric_pack = |record_id: &str, document: &'static str| {
            GraphSchemaSource::new(
                SchemaSourceOrigin::Pack {
                    connector: "numeric".to_string(),
                    record_id: record_id.to_string(),
                },
                None,
                Some(Arc::from(document)),
                0,
            )
            .unwrap()
        };
        assert!(sources
            .attach_dynamic(
                "pack:numeric".to_string(),
                numeric_pack("pack:numeric:2:aaaa", ontology_a),
            )
            .unwrap());
        assert!(sources
            .attach_dynamic(
                "pack:numeric".to_string(),
                numeric_pack("pack:numeric:10:bbbb", ontology_b),
            )
            .unwrap());
        assert!(sources
            .attach_dynamic("ingest:cmdb".to_string(), ingest(2, ontology_a))
            .unwrap());
        assert!(sources
            .attach_dynamic("ingest:cmdb".to_string(), ingest(1, ontology_b))
            .unwrap_err()
            .contains("SCHEMA_SOURCE_REGRESSION"));
        assert!(sources
            .attach_dynamic("ingest:cmdb".to_string(), ingest(3, ontology_b))
            .unwrap());
    }

    #[test]
    fn empty_pack_head_is_a_monotonic_withdrawal_tombstone() {
        let pack = |record_id: &str, ontology: Option<&'static str>| {
            GraphSchemaSource::new(
                SchemaSourceOrigin::Pack {
                    connector: "graph-os".to_string(),
                    record_id: record_id.to_string(),
                },
                None,
                ontology.map(Arc::from),
                0,
            )
            .unwrap()
        };
        let mut sources = GraphSchemaSources::default();
        let ontology = "@prefix owl: <http://www.w3.org/2002/07/owl#> .";
        assert!(sources
            .attach_dynamic(
                "pack:graph-os".to_string(),
                pack("pack:graph-os:1:aaaa", Some(ontology)),
            )
            .unwrap());
        assert!(sources
            .attach_dynamic(
                "pack:graph-os".to_string(),
                pack("pack:graph-os:2:bbbb", None),
            )
            .unwrap());
        let tombstone = sources.dynamic.get("pack:graph-os").unwrap();
        assert!(tombstone.shapes_ttl.is_none() && tombstone.ontology_ttl.is_none());
        assert!(sources
            .attach_dynamic(
                "pack:graph-os".to_string(),
                pack("pack:graph-os:1:cccc", Some(ontology)),
            )
            .unwrap_err()
            .contains("SCHEMA_SOURCE_REGRESSION"));
    }

    #[test]
    fn core_catalog_cannot_be_partially_installed() {
        let mut sources = GraphSchemaSources::default();
        sources.core.pop_first();
        assert!(sources
            .validate()
            .unwrap_err()
            .contains("immutable catalog"));
    }

    #[test]
    fn core_artifact_bytes_and_legacy_iri_namespace_are_pinned() {
        // Pin the exact bytes compiled by `include_str!`, rather than a
        // pre-write renderer buffer (which may retain an extra terminal blank
        // line). Runtime source identities are derived from these bytes.
        let expected = [
            (
                "a2a",
                "3206052c6dbff90471ba7d8bdac1541b29c2580be77802880b95f8c73deb8f72",
            ),
            (
                "action",
                "eb539c1652be9fa0bc9c60881221e081a17c6afb9c3cdb9d21cd73521819c98a",
            ),
            (
                "archimate",
                "a0e75bdb8086f2e0d35b3f3e432ce8839fa02d6251522ecd0319500d57e3d697",
            ),
            (
                "capability",
                "0bef554581e3c54e61a374e0428bd8923be382601e66312b881df0d307aa37b8",
            ),
            (
                "argumentation",
                "3802cfc9bff937f93a638bf2ca1b0fcd9a6b4432b3a02be12d404e22d1582cd0",
            ),
            (
                "calendar",
                "67047ac0aa2a3bd0b41de948c729e5b0da579fb093c09e4f5492925da5d5b008",
            ),
            (
                "catalog",
                "25bb360d41fea58b0ec3b3fc277aaaf42fa8ff5b17a66787ba81b7236e361912",
            ),
            (
                "company",
                "e68b101468aa5e90b959cadf63482576db212aae64cbe51c84d28a3c17e85ca7",
            ),
            (
                "company_infra",
                "f5c16236b1d96b4ad01a61b9176250fe9f58045d3a9d4d9ed25284f137b5ec34",
            ),
            (
                "connector-pack",
                "a13628fbb9b18d4f6f92c33282515ff598c1ccd417d1cdd9dbd9c9adf3067a63",
            ),
            (
                "concepts",
                "6713cdc3435cd0f4f76991b37667583f522d4610ba0aa6191aab052c81feb8b3",
            ),
            (
                "foundation",
                "a79cc120e3d5ef46bb4b80d9912851b457486382cf886cc763c73494bc7ad92a",
            ),
            (
                "foundation-shapes",
                "405805a8b246426bffcaacc5e3caf1b54d6e1340a281379ca7a5678c87f69cfe",
            ),
            (
                "documentation",
                "f61c25413211d5cc92523c548d2d50a8d26a8a5473cb59de5bf7ab16e20155b1",
            ),
            (
                "energy_geopolitics",
                "bc3d0a535e8c5d21e50e82126c7d83244d445fb178d1dab0a995599f95c258a9",
            ),
            (
                "enterprise",
                "d70aabcd24c0d8b3149933a2a8c686d29323ed72bce79e571ecc8787bd919c17",
            ),
            (
                "enterprise-shapes",
                "df333980776e43addf5473dc9e875fd465b491e5a1e424bddf42f4eb22d9910d",
            ),
            (
                "governance-shapes",
                "8195ee0454e851d182b14c9d30e1bcfa2638b44719964cd51e08d75cdc13b12d",
            ),
            (
                "government",
                "6fc9f53374146e50161e5fe2aa83a5902b5c9e86fe03be69f089ddd6d39de310",
            ),
            (
                "government-shapes",
                "cf6514787314e925a4afb5e9019e9a45d40af8f1393ca5005375ffa4559fed0a",
            ),
            (
                "harness",
                "e11026fc84868a7f8a18b344bb8af0600022c9c6c75f9ef43fb7f02553d34c6a",
            ),
            (
                "hr",
                "bce983d484bd2d1681cacc16d6c13c9ffa229c474e0e85340f0398baf848bf6f",
            ),
            (
                "identity",
                "475e321e4af6c441942a7b02e79ad78abd1a76da619e3806a77044f388fd01e3",
            ),
            (
                "identity-shapes",
                "753ea0407a1f691351684761e2f500e3d3df3e611e5a8c249befc740350efdf8",
            ),
            (
                "infrastructure",
                "43aa6d86e29488f412eb804b8e353eca23f67e4bf5cfdc749aeeb68fc98efc4b",
            ),
            (
                "medical",
                "512fe88b3d8a190be089a674869a193e25ca09c591cb7cd307139e7039e75554",
            ),
            (
                "native_source_connector",
                "48a1e90706248bde126d4c3d2d2f34c136cca5797a00612c6a13c994358efda2",
            ),
            (
                "orchestration",
                "2e40ac70f6483732ac1caedbd2bc41aaf961fdf2951222a46f2a11b960e62fc6",
            ),
            (
                "orchestration-shapes",
                "c7462c9fe1037ef33283e59766645931956cb90f66aad23e2bb4c894015a119f",
            ),
            // MEDULLA-ORCH-VOCAB-R811.5: the core state-machine vocabulary and shapes.
            (
                "state_machines",
                "d1e853728f9f2603ecc3e65cc5d0e61a211bf5c6e60f7f87ef73616450cae3df",
            ),
            (
                "state_machines-shapes",
                "33d359241e989cab677206d9292b1cda3031637844d5f53d57aaad4b09b2da63",
            ),
            (
                "personal",
                "465c65532104da388ed28bf1d3d4d6b399f57b581962ec4c72ea782b730dec25",
            ),
            (
                "process_intelligence",
                "52a426363a7d0a7d50ed8e84237be6196c96c9eb383cf7128bf0d637b33c8694",
            ),
            (
                "sdd",
                "59a092b0199b1924b45ac8ebc29e1ed3a0622db9c6a590f6fc8e7cb95a35c65c",
            ),
            (
                "service_operations",
                "3bf4054b73e1a47ff17d28f9f2b8f662cefafd05e5feac911ec5fcf28312147d",
            ),
            (
                "dataplatform",
                "bf130bc753323c5430a6b604f93fd03a87ae6d98b7563ca0c4b9f54b442bac26",
            ),
            (
                "data-platform",
                "0cbf52870a519d293be4267b8a9d9b26fc97a378fa6e4f20f30980db6c9cdae9",
            ),
            (
                "it4it_delivery",
                "7f919c5765dc4e80aac74455b3ce1dd029b89a4d47ad08b038e024b89cb0c395",
            ),
            (
                "media",
                "3c7a75b5b97534fb7d13f060c5f049dd3bd9f7398740a99699682fb2365889dc",
            ),
            (
                "retrieval",
                "56662368e017e720d1808fbfd1478a2203155c5fe034c423868aebbcc0da82f4",
            ),
            (
                "sdlc_lifecycle",
                "17ee3ad6857ae44628eb8699c0ea93c7195d40e1b48351ee60d0d340d969644f",
            ),
            (
                "software",
                "be96d5d3c1dc265ec3e9647d6d5aa5948cd226196d08cd6e5918ca904ba0fe4d",
            ),
            (
                "system",
                "77fd37067abaff0c7cc52cfcdd0de5fcec73c6de1c9fad453e311fdf313e2720",
            ),
            (
                "trm",
                "4e4e00f56b86e279e150880f163ceac222c30663390c9fbc8bcb1545eb44e169",
            ),
            (
                "life",
                "4da55226e90f23254343fd91dc59433e4361baed5ce84ffb29c5b544e48a44dd",
            ),
            (
                "environment",
                "0617000bebceac047fc2dd836d1a2efc681d7632f83479cd46098c3d84f72a53",
            ),
            (
                "nutrition",
                "21692c19900abfaf614d62f1c848dc84cfab6b792a4258cfbed4f7064170a87b",
            ),
            (
                "world-model-shapes",
                "f2be16c304237069bc97b91afc01ac90c846a3f466b7d7ace89ed3e210acdb8d",
            ),
            (
                "swarm-topology",
                "6a537da068d767d250805becbd376d4ea82b8f80f2cd6e259e900e6a50051adb",
            ),
            (
                "swarm-topology-shapes",
                "1072f280c5761d749574f913967ab1fbdef04681795e43db961c8b5e9c3a4413",
            ),
            // MEDULLA-PLACEMENT-R001.5: the placement module (ontology plus its
            // shapes companion) joins the catalog; the pin follows the ontology.
            (
                "placement",
                "1e94bfd8f3c33e6e050ec55e1f090c94b86372d15b25d6df807156563ac772ec",
            ),
            // R004.1 and R004.2.1/.2 added the asset-class migration and portfolio
            // records; semantic acceptance/refusal fixtures live in
            // src/server/graph_schema/finance_tests.rs. Pin both updated artifacts.
            // R004.2.3-R004.2.6 added the portfolio-group, strategy and leverage records.
            (
                "finance",
                "0de289f4dfc0ae34d33ba95c905fad6b0d9e56f88fe7bbafcb3bc799c65c9781",
            ),
            // MEDULLA-CQ-CONN-R071.1: the documentation shapes companion.
            (
                "documentation-shapes",
                "ff2a265a10d6331770bcf2155a5f8e3c74e15522de4bf570e867ee95b6310cb2",
            ),
            (
                "finance-shapes",
                "47575c365cadc8e77936450e7e851803158526363366b0e63f306d4ef4c18eaa",
            ),
            (
                "worldview",
                "ff0bc0c9407d2e400ae56b3aa185a6bcc6e41980496c664ebd037c247bf1206a",
            ),
            (
                "metadata_catalog",
                "63847a25c0a66b121048f881be89b64bb487af02d306ed84a32b26e0e9a31b99",
            ),
            (
                "metadata_catalog-shapes",
                "6d31d420165aac4ebc401348fda95a9e43e94bdc6b26018c8b607f2ae0e18bb8",
            ),
            (
                "temporal-shapes",
                "9bb13dd655059b6757b33621446f960bd6992bcb5aea31d5a85f4483c86bba59",
            ),
            (
                "virtual-graph",
                "a6b6917d16902e3e3f5d7a58ad2bdb50e342ea10837bbb49ee9161eef976e3bc",
            ),
            (
                "virtual-graph-shapes",
                "57bf2d274d3be29000b236f466bf65960455652fc0d022c7f2ece3dc180b8340",
            ),
            // MEDULLA-SUPPLY-VOCAB-R003.13: the commercial module (ontology plus its
            // shapes companion) joins the catalog; the pin follows the ontology.
            (
                "commercial",
                "9ff80088ba38249ee5421ae50ad263ac9fb06f8f441c1152652b5f56a79ec321",
            ),
            (
                "supply_chain",
                "d7b82d00c2d5ba05b188af74169b1ebd9b6283a580d8cb49427fb9c51b3ea1a8",
            ),
            // MEDULLA-TYPED-PACKS-R101.1/R102.1: the connector-pack module (ontology
            // plus its surface shapes companion) joins the catalog; the pin
            // follows the ontology.
            // The generated decision module (ontology plus shapes); the pin follows the ontology.
            (
                "decision",
                "15c7aacce36cbf473f086497eb3e06caec75c8333538388a0f2f728af9101760",
            ),
        ];
        let catalog = current_core_catalog();
        let pinned_modules: BTreeSet<_> = expected.iter().map(|(module, _)| *module).collect();
        assert_eq!(
            pinned_modules.len(),
            expected.len(),
            "duplicate core artifact pin"
        );
        assert_eq!(
            expected.len(),
            catalog.len(),
            "every core source needs a byte pin"
        );
        for (module, digest) in expected {
            let source = &catalog[&format!("core:{module}@1")];
            // A module can carry both documents (commercial: ontology plus its
            // shapes companion); the pin follows the ontology, the module's
            // primary artifact, with the shapes-only modules unchanged.
            let actual = source.ontology_sha256.or(source.shapes_sha256).unwrap();
            assert_eq!(actual.to_hex(), digest, "artifact drift for {module}");
            let document = source
                .ontology_ttl
                .as_deref()
                .or(source.shapes_ttl.as_deref())
                .unwrap();
            assert!(document.contains("http://knuckles.team/kg"));
            assert!(!document.contains("@prefix eg:"));
        }
    }

    // spec: MEDULLA-SRC-AUTH-R003.2
    #[test]
    fn pack_or_ingestion_attach_refuses_virtual_graph_approval_claims() {
        let approved_mapping = concat!(
            "@prefix vg: <http://knuckles.team/kg/virtual-graph#> .\n",
            "@prefix m: <http://example.org/mapping#> .\n",
            "m:incidentMapping a vg:Mapping ;\n",
            "    vg:mappedClass m:Incident ;\n",
            "    vg:approved true .\n"
        );
        let approved_authority = concat!(
            "@prefix vg: <http://knuckles.team/kg/virtual-graph#> .\n",
            "@prefix a: <http://example.org/authority#> .\n",
            "a:incidentAuthority a vg:SourceAuthority ;\n",
            "    vg:authorityFor a:Incident ;\n",
            "    vg:approved true .\n"
        );
        let iri_claim = concat!(
            "@prefix m: <http://example.org/mapping#> .\n",
            "m:iriMapping a m:Thing ;\n",
            "    <http://knuckles.team/kg/virtual-graph#approved> true .\n"
        );
        let pack = |record_id: &str, document: String| {
            GraphSchemaSource::new(
                SchemaSourceOrigin::Pack {
                    connector: "servicenow".to_string(),
                    record_id: record_id.to_string(),
                },
                Some(Arc::from(document)),
                None,
                0,
            )
            .unwrap()
        };
        let mut sources = GraphSchemaSources::default();

        // The approved mapping row, carried in a pack shapes document, is
        // refused with the typed code.
        let error = sources
            .attach_dynamic(
                "pack:servicenow".to_string(),
                pack("rec:1", approved_mapping.to_string()),
            )
            .unwrap_err();
        assert!(error.starts_with("APPROVAL_CLAIM_REFUSED:"), "{error}");

        // The approved authority row, carried in a pack ontology document, is
        // refused the same way.
        let authority_pack = GraphSchemaSource::new(
            SchemaSourceOrigin::Pack {
                connector: "pagerduty".to_string(),
                record_id: "rec:1".to_string(),
            },
            None,
            Some(Arc::from(approved_authority)),
            0,
        )
        .unwrap();
        let error = sources
            .attach_dynamic("pack:pagerduty".to_string(), authority_pack)
            .unwrap_err();
        assert!(error.starts_with("APPROVAL_CLAIM_REFUSED:"), "{error}");

        // The same refusal holds for the ingestion origin.
        let ingestion = GraphSchemaSource::new(
            SchemaSourceOrigin::Ingestion {
                mapping: "incident".to_string(),
                revision: 1,
            },
            Some(Arc::from(approved_mapping)),
            None,
            0,
        )
        .unwrap();
        let error = sources
            .attach_dynamic("ingest:incident".to_string(), ingestion)
            .unwrap_err();
        assert!(error.starts_with("APPROVAL_CLAIM_REFUSED:"), "{error}");
        assert!(sources.dynamic.is_empty());

        // The full-IRI predicate form is refused as well.
        let error = sources
            .attach_dynamic(
                "pack:servicenow".to_string(),
                pack("rec:2", iri_claim.to_string()),
            )
            .unwrap_err();
        assert!(error.starts_with("APPROVAL_CLAIM_REFUSED:"), "{error}");
        assert!(sources.dynamic.is_empty());

        // The refusal is origin-scoped: a document without the claim attaches,
        // and the same claim text under the operator origin attaches.
        let not_approved = approved_mapping.replace("vg:approved true .", "vg:approved false .");
        sources
            .attach_dynamic("pack:servicenow".to_string(), pack("rec:3", not_approved))
            .unwrap();
        sources
            .attach_dynamic(
                OPERATOR_SOURCE_ID.to_string(),
                GraphSchemaSource::new(
                    SchemaSourceOrigin::Operator,
                    Some(Arc::from(approved_mapping)),
                    None,
                    0,
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(sources.dynamic.len(), 2);
    }
}
