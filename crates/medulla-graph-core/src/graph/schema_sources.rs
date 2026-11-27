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
pub const MAX_CORE_SCHEMA_SOURCES: usize = 96;
pub const MAX_TOTAL_DYNAMIC_SCHEMA_BYTES: usize = 8 << 20;
pub const MAX_SCHEMA_DOCUMENT_TRIPLES: usize = 100_000;
pub const CORE_SOURCE_PREFIX: &str = "core:";
pub const OPERATOR_SOURCE_ID: &str = "operator";
/// The SHACL shapes of the core virtual-graph vocabulary (`core:virtual-graph-shapes@1`).
/// An attached authority document is validated against them (MEDULLA-SRC-AUTH-R002.5).
pub const VIRTUAL_GRAPH_SHAPES: &str =
    include_str!("../../../medulla-core/ontology/virtual_graph-v1.shapes.ttl");
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
    /// `tenant` is the verified tenant whose pack head this source composes,
    /// recorded where the source is attached. It is how the catalog answers
    /// "which graphs compose tenant T's `pack:<connector>`" without opening a
    /// graph (DECISIONS PACK-CLASS-IN-USE-IS-SCOPED-AND-INDEXED). A source
    /// attached before the tenant was recorded carries `None` and counts for
    /// every tenant until its next attach records one.
    Pack {
        connector: String,
        record_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant: Option<String>,
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

use medulla_catalog_types::graph_schema::{OntologyRevisionEntry, MAX_ONTOLOGY_REVISION_HISTORY};

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
    /// The typed record contract an `approved:<name>` source was rendered from
    /// (MEDULLA-TYPED-PACKS-R714). The rendered SHACL is lossy (type sets), so
    /// the delivery drift gate reads the contract itself. The shapes document is
    /// rendered from it, so it is outside the composed digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<medulla_catalog_types::graph_schema::repair::RecordContract>,
    /// The crosswalk coverage of the core revision these crosswalks were
    /// validated against, computed by the pack activation
    /// (MEDULLA-TYPED-PACKS-R097.2.3), with the tenant, graph, pack record and schema
    /// digest it was bound to (R097.2.5). Derived from `crosswalks` and the pinned
    /// revision, so it is outside both the composed digest and document identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crosswalk_coverage: Option<medulla_catalog_types::domain_pack::BoundCrosswalkCoverage>,
    /// The coverage of each external schema this pack source carries, computed by the
    /// pack activation with `compute_external_coverage`
    /// (MEDULLA-TYPED-PACKS-R097.2.7.3). Derived from the pack's `external_schema`
    /// members and the composition's pins, so it is outside both the composed digest
    /// and document identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_coverage:
        Vec<medulla_catalog_source_types::external_schema::ExternalOntologyCoverage>,
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
            contract: None,
            crosswalk_coverage: None,
            external_coverage: Vec::new(),
            attached_at_ms,
        })
    }

    /// The same source carrying the typed contract its shapes were rendered from.
    pub fn with_contract(
        mut self,
        contract: medulla_catalog_types::graph_schema::repair::RecordContract,
    ) -> Self {
        self.contract = Some(contract);
        self
    }

    /// The same source carrying the crosswalks the importer validated.
    pub fn with_crosswalks(
        mut self,
        crosswalks: Vec<medulla_catalog_types::domain_pack::VocabularyCrosswalk>,
    ) -> Self {
        self.crosswalks = crosswalks;
        self
    }

    /// The same source carrying the coverage its activation computed.
    pub fn with_crosswalk_coverage(
        mut self,
        coverage: Option<medulla_catalog_types::domain_pack::BoundCrosswalkCoverage>,
    ) -> Self {
        self.crosswalk_coverage = coverage;
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
            && self.contract == other.contract
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

    /// Record the coverage an activation computed for one dynamic source.
    /// Returns whether the stored value changed; an unknown source is `false`.
    pub fn set_dynamic_crosswalk_coverage(
        &mut self,
        source_id: &str,
        coverage: Option<medulla_catalog_types::domain_pack::BoundCrosswalkCoverage>,
    ) -> bool {
        match self.dynamic.get_mut(source_id) {
            Some(source) if source.crosswalk_coverage != coverage => {
                source.crosswalk_coverage = coverage;
                true
            }
            _ => false,
        }
    }

    /// Record the external-schema coverage an activation computed for one dynamic
    /// source. Returns whether the stored value changed; an unknown source is `false`.
    pub fn set_dynamic_external_coverage(
        &mut self,
        source_id: &str,
        coverage: Vec<medulla_catalog_source_types::external_schema::ExternalOntologyCoverage>,
    ) -> bool {
        match self.dynamic.get_mut(source_id) {
            Some(source) if source.external_coverage != coverage => {
                source.external_coverage = coverage;
                true
            }
            _ => false,
        }
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
        reject_unknown_source_role(source_id, source)?;
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

/// The closed set of `vg:sourceRole` values (MEDULLA-SRC-AUTH-R010.3); the
/// virtual-graph crate pins it to the serde names of `SourceRole`.
pub const SOURCE_ROLE_VALUES: [&str; 3] = ["live_truth", "historical_context", "inspiration"];

/// MEDULLA-SRC-AUTH-R010.3: a `vg:sourceRole` value outside the closed set is
/// refused at the attach seam with the typed code `SOURCE_ROLE_UNKNOWN`, so a
/// misspelt role can never silently rank a source as live truth.
fn reject_unknown_source_role(source_id: &str, source: &GraphSchemaSource) -> Result<(), String> {
    for (label, document) in [
        ("shapes", source.shapes_ttl.as_deref()),
        ("ontology", source.ontology_ttl.as_deref()),
    ] {
        if let Some(value) = document.and_then(unknown_source_role) {
            return Err(format!(
                "SOURCE_ROLE_UNKNOWN: {label} document of '{source_id}' gives vg:sourceRole \
                 \"{value}\"; the roles are {SOURCE_ROLE_VALUES:?}"
            ));
        }
    }
    Ok(())
}

/// The first quoted `vg:sourceRole` value of `document` that is not a known role.
fn unknown_source_role(document: &str) -> Option<String> {
    const ROLE: &str = medulla_types::ontology_iri!("virtual-graph#sourceRole");
    let lines: Vec<&str> = document.lines().collect();
    let mut needles: Vec<String> = vec![format!("<{ROLE}>")];
    for (label, iri) in prefix_bindings(&lines) {
        if format!("{iri}sourceRole") == ROLE {
            needles.push(format!("{label}:sourceRole"));
        }
    }
    for line in &lines {
        let text = strip_line_comment(line);
        for needle in &needles {
            for (position, _) in text.match_indices(needle.as_str()) {
                let rest = text[position + needle.len()..].trim_start();
                let Some(quoted) = rest.strip_prefix('"') else {
                    continue;
                };
                let value = quoted.split('"').next().unwrap_or_default();
                if !SOURCE_ROLE_VALUES.contains(&value) {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Conservative line scan for a truthy `vg:approved` triple. This crate has no
/// RDF parser, so the scan recognises the predicate as the full IRI or as any
/// `@prefix` label that resolves to the virtual-graph namespace, and the value
/// as the first term after the predicate (falling through to the next line
/// across a property-list continuation). A comment never starts inside an IRI,
/// so only a `#` at line start or after whitespace terminates a line.
fn document_claims_virtual_graph_approval(document: &str) -> bool {
    const APPROVED: &str = medulla_types::ontology_iri!("virtual-graph#approved");
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
            tenant,
        } => {
            keyed(source_id, "pack:", connector)
                && !record_id.is_empty()
                && tenant.as_deref().is_none_or(|tenant| !tenant.is_empty())
        }
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
                tenant: current_tenant,
            },
            SchemaSourceOrigin::Pack {
                connector: replacement_connector,
                record_id: replacement_record,
                tenant: replacement_tenant,
            },
        ) => {
            // A source never moves to another tenant's pack. A source attached
            // before the tenant was recorded adopts the tenant of the same
            // record when that record is attached again.
            let same_tenant = current_tenant.is_none() || current_tenant == replacement_tenant;
            let adopts_tenant = current_record == replacement_record
                && current_tenant.is_none()
                && replacement_tenant.is_some();
            current_connector == replacement_connector
                && same_tenant
                && (adopts_tenant || pack_record_advances(current_record, replacement_record))
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
            module: "dmn",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/dmn-v1.shapes.ttl"
            )),
            ontology: Some(include_str!("../../../medulla-core/ontology/dmn-v1.ttl")),
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
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/a2a-v1.shapes.ttl"
            )),
            ontology: Some(include_str!("../../../medulla-core/ontology/a2a-v1.ttl")),
        },
        CoreSpec {
            module: "action",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/action-v1.shapes.ttl"
            )),
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
        // MEDULLA-TYPED-PACKS-R105.10.4: every packaged core module is in the parent
        // catalog, so pack import judges a pack against the class set the wheel ships
        // (downtime-work is registered below with its shapes, R101.3).
        CoreSpec {
            module: "bpmn",
            version: 1,
            // MEDULLA-ORCH-VOCAB-R014.5: the BPMN structural constraints.
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/bpmn-v1.shapes.ttl"
            )),
            ontology: Some(include_str!("../../../medulla-core/ontology/bpmn-v1.ttl")),
        },
        CoreSpec {
            module: "content",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/content-v1.ttl"
            )),
        },
        CoreSpec {
            module: "connector_seed",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/connector_seed-v1.ttl"
            )),
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
        // MEDULLA-SDLC-R014.5: the spec-driven development pack. Every class
        // extends a core class; the document imports nothing.
        CoreSpec {
            module: "sdd_spec",
            version: 1,
            shapes: None,
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/sdd-spec-v1.ttl"
            )),
        },
        // MEDULLA-SDLC-R014.11: SHACL shapes that mirror the spec-standard row checks.
        CoreSpec {
            module: "sdd-row-shapes",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/sdd-row-v1.shapes.ttl"
            )),
            ontology: None,
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
        // MEDULLA-IDENTITY-R023.8: the typed principal metadata vocabulary and its
        // bounds, one core source (ontology plus its shapes companion).
        CoreSpec {
            module: "identity_metadata",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/identity_metadata.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/identity_metadata.ttl"
            )),
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
            shapes: Some(VIRTUAL_GRAPH_SHAPES),
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
        // MEDULLA-BUSINESS-OPS-R001: the business operations pack (profile, orders and
        // money, procurement, compliance, tax, approvals), with its shapes companion in
        // the same CoreSpec shapes field (the commercial pattern).
        CoreSpec {
            module: "business_ops",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/business_ops-v1.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/business_ops-v1.ttl"
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
        // MEDULLA-DURABLE-KERNEL-R101.3: the downtime job-type vocabulary the
        // downtime tick reads from the commons graph, with its shapes companion.
        CoreSpec {
            module: "downtime-work",
            version: 1,
            shapes: Some(include_str!(
                "../../../medulla-core/ontology/downtime_work-v1.shapes.ttl"
            )),
            ontology: Some(include_str!(
                "../../../medulla-core/ontology/downtime_work-v1.ttl"
            )),
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
        // A source with no recorded tenant keeps the bytes it was digested
        // under, so a stored graph's composed digest does not move.
        SchemaSourceOrigin::Pack {
            connector,
            record_id,
            tenant: None,
        } => format!("pack\0{connector}\0{record_id}").into_bytes(),
        SchemaSourceOrigin::Pack {
            connector,
            record_id,
            tenant: Some(tenant),
        } => format!("pack\0{connector}\0{record_id}\0{tenant}").into_bytes(),
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
        // 70 (counted entries of `core_specs`): +3 for bpmn, content and connector_seed
        // (MEDULLA-TYPED-PACKS-R105.10.4), on top of 67 entries; the pin read 65 and had
        // not moved for the two entries registered before this row.
        assert_eq!(sources.core.len(), 70);
        assert!(sources.dynamic.is_empty());
        assert!(sources.core.keys().all(|id| id.starts_with("core:")));
    }

    // spec: MEDULLA-ORCH-VOCAB-R014.7
    #[test]
    fn bpmn_and_dmn_sources_compose() {
        let sources = GraphSchemaSources::default();
        sources.validate().unwrap();
        for module in ["bpmn", "dmn"] {
            let id = format!("core:{module}@1");
            let source = sources
                .core
                .get(&id)
                .unwrap_or_else(|| panic!("{id} is a core source"));
            assert!(source.shapes_ttl.is_some(), "{id} loads its shapes");
            assert!(source.ontology_ttl.is_some(), "{id} loads its ontology");
            assert!(
                sources.shapes().any(|(source_id, _)| source_id == id),
                "{id} is composed with the shapes"
            );
            assert!(
                sources.ontologies().any(|(source_id, _)| source_id == id),
                "{id} is composed with the ontology"
            );
        }
        // The classes the process vocabulary answers queries over are in the composed text.
        let composed: String = sources.ontologies().map(|(_, text)| text).collect();
        for class in [
            ":BpmnUserTask",
            ":BpmnExclusiveGateway",
            ":DmnDecisionTable",
        ] {
            assert!(composed.contains(class), "{class} is composed");
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R105.10.4
    #[test]
    fn core_sources_cover_every_packaged_module() {
        let sources = GraphSchemaSources::default();
        let loaded: std::collections::BTreeSet<&str> =
            sources.ontologies().map(|(_, document)| document).collect();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../medulla-core/ontology");
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".ttl") || name.ends_with(".shapes.ttl") {
                continue;
            }
            let text = std::fs::read_to_string(entry.path()).unwrap();
            if !loaded.contains(text.as_str()) {
                missing.push(name);
            }
        }
        missing.sort();
        assert!(
            missing.is_empty(),
            "packaged core modules absent from the parent catalog: {missing:?}"
        );
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
                tenant: None,
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
                    tenant: None,
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
                    tenant: None,
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
    fn pack_source_records_its_tenant_and_never_changes_tenant() {
        let pack = |record_id: &str, tenant: Option<&str>| {
            GraphSchemaSource::new(
                SchemaSourceOrigin::Pack {
                    connector: "github".to_string(),
                    record_id: record_id.to_string(),
                    tenant: tenant.map(str::to_string),
                },
                None,
                Some(Arc::from("@prefix owl: <http://www.w3.org/2002/07/owl#> .")),
                0,
            )
            .unwrap()
        };
        let mut legacy = GraphSchemaSources::default();
        legacy
            .attach_dynamic("pack:github".to_string(), pack("pack:github:1:aaaa", None))
            .unwrap();
        let legacy_digest = legacy.composed_digest();
        // A source with no recorded tenant keeps its digest bytes.
        assert_eq!(
            origin_bytes(&pack("pack:github:1:aaaa", None).origin),
            b"pack\0github\0pack:github:1:aaaa".to_vec()
        );
        // Attaching the same record again records the tenant.
        assert!(legacy
            .attach_dynamic(
                "pack:github".to_string(),
                pack("pack:github:1:aaaa", Some("tenant:a")),
            )
            .unwrap());
        assert_ne!(legacy.composed_digest(), legacy_digest);
        // Once recorded, neither the same nor a newer record moves the source
        // to another tenant.
        for record_id in ["pack:github:1:aaaa", "pack:github:2:bbbb"] {
            assert!(legacy
                .attach_dynamic("pack:github".to_string(), pack(record_id, Some("tenant:b")))
                .unwrap_err()
                .contains("SCHEMA_SOURCE_REGRESSION"));
        }
        assert!(legacy
            .attach_dynamic(
                "pack:github".to_string(),
                pack("pack:github:2:bbbb", Some("tenant:a")),
            )
            .unwrap());
        // An empty tenant is not a tenant.
        assert!(GraphSchemaSources::default()
            .attach_dynamic(
                "pack:github".to_string(),
                pack("pack:github:1:aaaa", Some("")),
            )
            .is_err());
    }

    #[test]
    fn empty_pack_head_is_a_monotonic_withdrawal_tombstone() {
        let pack = |record_id: &str, ontology: Option<&'static str>| {
            GraphSchemaSource::new(
                SchemaSourceOrigin::Pack {
                    connector: "graph-os".to_string(),
                    record_id: record_id.to_string(),
                    tenant: None,
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

    /// Every core artifact lives under the one ontology IRI base
    /// (`https://knuckles-team.github.io/ontology/<module>#`): none declares a
    /// legacy namespace or the retired `eg:` prefix, a document's default
    /// namespace is under the base, and every document names the shared core
    /// vocabulary. The artifacts are authored by hand and change with every
    /// vocabulary row, so no digest of their bytes is pinned.
    #[test]
    fn core_artifacts_use_only_the_one_ontology_iri_base() {
        const BASE: &str = "https://knuckles-team.github.io/ontology/";
        const LEGACY: [&str; 4] = [
            "http://knuckles.team/kg",
            "https://agent-utilities.dev/ontology",
            "http://medulla/owl#",
            "@prefix eg:",
        ];
        let catalog = current_core_catalog();
        assert!(!catalog.is_empty(), "the core catalog has sources");
        for (source_id, source) in catalog {
            for document in [source.ontology_ttl.as_deref(), source.shapes_ttl.as_deref()]
                .into_iter()
                .flatten()
            {
                for legacy in LEGACY {
                    assert!(
                        !document.contains(legacy),
                        "{source_id} still carries the legacy namespace {legacy}"
                    );
                }
                for line in document.lines().filter(|l| l.starts_with("@prefix : <")) {
                    assert!(
                        line.starts_with(&format!("@prefix : <{BASE}")),
                        "{source_id}: default namespace is outside the ontology base: {line}"
                    );
                }
            }
            let document = source
                .ontology_ttl
                .as_deref()
                .or(source.shapes_ttl.as_deref())
                .expect("a core source carries a document");
            assert!(
                document.contains(&format!("{BASE}core")),
                "{source_id} does not name the core vocabulary"
            );
        }
    }

    // spec: MEDULLA-SRC-AUTH-R003.2
    #[test]
    fn pack_or_ingestion_attach_refuses_virtual_graph_approval_claims() {
        let approved_mapping = concat!(
            "@prefix vg: <https://knuckles-team.github.io/ontology/virtual-graph#> .\n",
            "@prefix m: <http://example.org/mapping#> .\n",
            "m:incidentMapping a vg:Mapping ;\n",
            "    vg:mappedClass m:Incident ;\n",
            "    vg:approved true .\n"
        );
        let approved_authority = concat!(
            "@prefix vg: <https://knuckles-team.github.io/ontology/virtual-graph#> .\n",
            "@prefix a: <http://example.org/authority#> .\n",
            "a:incidentAuthority a vg:SourceAuthority ;\n",
            "    vg:authorityFor a:Incident ;\n",
            "    vg:approved true .\n"
        );
        let iri_claim = concat!(
            "@prefix m: <http://example.org/mapping#> .\n",
            "m:iriMapping a m:Thing ;\n",
            "    <https://knuckles-team.github.io/ontology/virtual-graph#approved> true .\n"
        );
        let pack = |record_id: &str, document: String| {
            GraphSchemaSource::new(
                SchemaSourceOrigin::Pack {
                    connector: "servicenow".to_string(),
                    record_id: record_id.to_string(),
                    tenant: None,
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
                tenant: None,
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

    // spec: MEDULLA-SRC-AUTH-R010.3
    #[test]
    fn an_unknown_source_role_is_refused_and_the_closed_set_attaches() {
        let doc = |role: &str| {
            format!(
                "@prefix vg: <https://knuckles-team.github.io/ontology/virtual-graph#> .\n\
                 <urn:x:a> a vg:SourceConnection ; vg:sourceRole \"{role}\" .\n"
            )
        };
        for role in SOURCE_ROLE_VALUES {
            assert_eq!(unknown_source_role(&doc(role)), None, "{role}");
        }
        assert_eq!(
            unknown_source_role(&doc("live-truth")).as_deref(),
            Some("live-truth")
        );
        let source = GraphSchemaSource::new(
            SchemaSourceOrigin::Admin {
                name: "roles".to_string(),
            },
            None,
            Some(Arc::from(doc("primary"))),
            0,
        )
        .unwrap();
        let error = reject_unknown_source_role("admin:roles", &source).unwrap_err();
        assert!(error.starts_with("SOURCE_ROLE_UNKNOWN:"), "{error}");
    }
}
