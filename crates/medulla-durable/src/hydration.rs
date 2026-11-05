//! Typed baseline-hydration model (`MEDULLA-REPO-INGEST-R650.1`).
//!
//! These values describe a planned hydration pass.  They carry the tenant
//! boundary and durable source identities.  The pure planning functions
//! (`plan_baseline`, `split_repository`, `classify_file`) are
//! `MEDULLA-REPO-INGEST-R650.2`; fetching and writing belong to the later R650
//! children, which consume these values through their own bounded admission
//! paths.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use medulla_types::contract::Digest256;
use medulla_types::scheduler_config::SchedulerConfig;
use serde::{Deserialize, Serialize};

/// The stable model version for hydration records.
pub const HYDRATION_MODEL_VERSION: &str = "eg-durable/hydration-v1";

/// A bounded plan for one tenant's baseline source scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselinePlan {
    /// The tenant that owns every source in this plan.
    pub tenant_id: String,
    /// The configured source scope, such as a workspace or repository group.
    pub scope: String,
    /// Maximum number of work items admitted for this scope.
    pub max_work_items: u32,
    /// Stages that a worker may execute for each admitted item.
    pub stages: Vec<StagedStage>,
}

impl BaselinePlan {
    /// Validate the identity and the positive work bound before admission.
    pub fn validate(&self) -> Result<(), HydrationModelError> {
        require_text("tenant_id", &self.tenant_id)?;
        require_text("scope", &self.scope)?;
        if self.max_work_items == 0 {
            return Err(HydrationModelError::NonPositiveBound("max_work_items"));
        }
        if self.stages.is_empty() {
            return Err(HydrationModelError::EmptyStages);
        }
        Ok(())
    }
}

/// One typed key in a delta manifest.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeltaManifestKey {
    pub graph: String,
    pub category: String,
    pub uri: String,
}

impl DeltaManifestKey {
    pub fn validate(&self) -> Result<(), HydrationModelError> {
        require_text("graph", &self.graph)?;
        require_text("category", &self.category)?;
        require_text("uri", &self.uri)
    }
}

/// One `(graph, category, uri) -> content_hash` manifest entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaManifestEntry {
    pub key: DeltaManifestKey,
    pub content_hash: String,
}

impl DeltaManifestEntry {
    pub fn validate(&self) -> Result<(), HydrationModelError> {
        self.key.validate()?;
        require_text("content_hash", &self.content_hash)
    }
}

/// The durable content identities observed for one tenant's source set.
/// Entries are kept in canonical key order so serialization is deterministic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaManifest {
    pub tenant_id: String,
    pub entries: Vec<DeltaManifestEntry>,
    /// Digest over the tenant and the entries in canonical key order, so it is
    /// independent of insertion order.
    pub manifest_digest: Digest256,
}

impl DeltaManifest {
    /// An empty manifest for one tenant, with its digest already computed.
    pub fn new(tenant_id: impl Into<String>) -> Self {
        let mut manifest = Self {
            tenant_id: tenant_id.into(),
            entries: Vec::new(),
            manifest_digest: Digest256::from_bytes([0; 32]),
        };
        manifest.manifest_digest = manifest.compute_digest();
        manifest
    }

    /// Digest over the tenant and the entries sorted by key.
    pub fn compute_digest(&self) -> Digest256 {
        let mut sorted: Vec<&DeltaManifestEntry> = self.entries.iter().collect();
        sorted.sort_by(|left, right| left.key.cmp(&right.key));
        let mut fields: Vec<&[u8]> = vec![self.tenant_id.as_bytes()];
        for entry in sorted {
            fields.push(entry.key.graph.as_bytes());
            fields.push(entry.key.category.as_bytes());
            fields.push(entry.key.uri.as_bytes());
            fields.push(entry.content_hash.as_bytes());
        }
        Digest256::framed(b"eg-durable/delta-manifest-v1", &fields)
            .expect("fixed non-empty domain and a bounded field count")
    }

    pub fn validate(&self) -> Result<(), HydrationModelError> {
        require_text("tenant_id", &self.tenant_id)?;
        let mut keys = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !keys.insert(&entry.key) {
                return Err(HydrationModelError::DuplicateManifestKey);
            }
        }
        if self.manifest_digest != self.compute_digest() {
            return Err(HydrationModelError::ManifestDigestMismatch);
        }
        Ok(())
    }

    /// Insert or replace one source identity and retain canonical ordering.
    pub fn record(&mut self, entry: DeltaManifestEntry) -> Result<(), HydrationModelError> {
        entry.validate()?;
        if let Some(existing) = self.entries.iter_mut().find(|item| item.key == entry.key) {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
        self.entries.sort_by(|left, right| left.key.cmp(&right.key));
        self.manifest_digest = self.compute_digest();
        Ok(())
    }
}

/// A deterministic directory-weight split for one repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSplitPlan {
    pub tenant_id: String,
    pub repository_uri: String,
    /// Directory prefix to estimated work weight.  A flat map is sufficient
    /// for planning; the worker owns the later partition admission.
    pub directory_weights: BTreeMap<String, u64>,
    /// Parts in fill order; each lists whole directories whose summed weight
    /// is at most the split target, except a single directory that alone
    /// exceeds it.
    #[serde(default)]
    pub parts: Vec<Vec<String>>,
}

impl RepoSplitPlan {
    pub fn validate(&self) -> Result<(), HydrationModelError> {
        require_text("tenant_id", &self.tenant_id)?;
        require_text("repository_uri", &self.repository_uri)?;
        if self
            .directory_weights
            .keys()
            .any(|path| path.trim().is_empty())
        {
            return Err(HydrationModelError::EmptyIdentifier("directory"));
        }
        Ok(())
    }
}

/// Classification used by coverage accounting for source and non-source files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileClass {
    Source,
    Document,
    Config,
    Binary,
    Data,
    Unknown,
}

/// What one hydration pass observed, scoped to its tenant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydrationEvidence {
    pub tenant_id: String,
    pub plan_digest: Digest256,
    /// Directory to file count, from the split plan.
    pub split_weights: BTreeMap<String, u64>,
    /// Files skipped as already current.
    pub skipped: u64,
    /// Files that were new or changed.
    pub changed: u64,
    pub coverage_by_class: BTreeMap<FileClass, u64>,
}

impl HydrationEvidence {
    pub fn validate(&self) -> Result<(), HydrationModelError> {
        require_text("tenant_id", &self.tenant_id)?;
        if self.split_weights.keys().any(|path| path.trim().is_empty()) {
            return Err(HydrationModelError::EmptyIdentifier("directory"));
        }
        Ok(())
    }
}

/// One source scope a baseline pass may cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydrationSource {
    pub tenant_id: String,
    pub scope: String,
    pub items: u64,
}

/// The file paths of one repository to split.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFiles {
    pub tenant_id: String,
    pub repository_uri: String,
    pub paths: Vec<String>,
}

impl BaselinePlan {
    /// Digest of the plan's identity, bound, and stages.
    pub fn digest(&self) -> Digest256 {
        let max = self.max_work_items.to_be_bytes();
        let mut fields: Vec<&[u8]> = vec![self.tenant_id.as_bytes(), self.scope.as_bytes(), &max];
        let stages: Vec<[u8; 1]> = self.stages.iter().map(|stage| [*stage as u8]).collect();
        fields.extend(stages.iter().map(|stage| stage.as_slice()));
        Digest256::framed(b"eg-durable/baseline-plan-v1", &fields)
            .expect("fixed non-empty domain and a bounded field count")
    }
}

/// Plan one tenant's baseline pass. The scope is the sorted, de-duplicated
/// source scopes joined by `,`; the work bound is the summed item count
/// capped at `hydration_scope_cap`. Cost is O(sources).
pub fn plan_baseline(
    sources: &[HydrationSource],
    config: &SchedulerConfig,
) -> Result<BaselinePlan, HydrationModelError> {
    let first = sources.first().ok_or(HydrationModelError::NoSources)?;
    let mut scopes = BTreeSet::new();
    let mut items: u64 = 0;
    for source in sources {
        require_text("tenant_id", &source.tenant_id)?;
        require_text("scope", &source.scope)?;
        if source.tenant_id != first.tenant_id {
            return Err(HydrationModelError::MixedTenants);
        }
        scopes.insert(source.scope.as_str());
        items = items.saturating_add(source.items);
    }
    let cap = u64::from(config.hydration_scope_cap);
    let plan = BaselinePlan {
        tenant_id: first.tenant_id.clone(),
        scope: scopes.into_iter().collect::<Vec<_>>().join(","),
        max_work_items: u32::try_from(items.min(cap)).unwrap_or(u32::MAX),
        stages: vec![
            StagedStage::Fetch,
            StagedStage::Parse,
            StagedStage::Enrich,
            StagedStage::Write,
            StagedStage::Reason,
        ],
    };
    plan.validate()?;
    Ok(plan)
}

/// Group a repository's files by directory and fill parts up to `target`
/// files. Directories are taken in descending file count (ties by name); a
/// directory that alone exceeds the target forms its own part. Cost is
/// O(files) plus a sort of the directories.
pub fn split_repository(
    files: &RepoFiles,
    target: u32,
) -> Result<RepoSplitPlan, HydrationModelError> {
    require_text("tenant_id", &files.tenant_id)?;
    require_text("repository_uri", &files.repository_uri)?;
    if target == 0 {
        return Err(HydrationModelError::NonPositiveBound("target"));
    }
    let mut directory_weights: BTreeMap<String, u64> = BTreeMap::new();
    for path in &files.paths {
        let directory = match path.rfind('/') {
            Some(0) | None => ".",
            Some(index) => &path[..index],
        };
        *directory_weights.entry(directory.to_owned()).or_default() += 1;
    }
    let mut ordered: Vec<(&String, u64)> = directory_weights
        .iter()
        .map(|(directory, count)| (directory, *count))
        .collect();
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    let target = u64::from(target);
    let mut parts: Vec<Vec<String>> = Vec::new();
    let mut filled: u64 = 0;
    for (directory, count) in ordered {
        match parts.last_mut() {
            Some(part) if filled + count <= target => {
                part.push(directory.clone());
                filled += count;
            }
            _ => {
                parts.push(vec![directory.clone()]);
                filled = count;
            }
        }
    }
    let plan = RepoSplitPlan {
        tenant_id: files.tenant_id.clone(),
        repository_uri: files.repository_uri.clone(),
        directory_weights,
        parts,
    };
    plan.validate()?;
    Ok(plan)
}

const MAGIC_BINARY: &[&[u8]] = &[
    b"%PDF",
    b"PK\x03\x04",
    b"\x7fELF",
    b"\x89PNG",
    b"GIF8",
    b"\xff\xd8\xff",
];

/// Classify one file: the extension decides first; only a file whose
/// extension is unknown is checked against the binary magic bytes.
pub fn classify_file(path: &str, head_bytes: &[u8]) -> FileClass {
    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, extension)| extension.to_ascii_lowercase());
    if let Some(extension) = extension.as_deref() {
        let by_extension = match extension {
            "rs" | "py" | "js" | "jsx" | "ts" | "tsx" | "go" | "java" | "kt" | "c" | "h" | "cc"
            | "cpp" | "hpp" | "cs" | "rb" | "php" | "swift" | "sh" | "sql" | "scala" => {
                Some(FileClass::Source)
            }
            "md" | "rst" | "txt" | "adoc" | "pdf" | "docx" | "html" => Some(FileClass::Document),
            "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "env" | "lock" => {
                Some(FileClass::Config)
            }
            "json" | "jsonl" | "csv" | "tsv" | "parquet" | "avro" | "xml" | "ttl" | "nt" => {
                Some(FileClass::Data)
            }
            "exe" | "dll" | "so" | "dylib" | "bin" | "o" | "a" | "png" | "jpg" | "jpeg" | "gif"
            | "zip" | "gz" | "tar" | "wasm" => Some(FileClass::Binary),
            _ => None,
        };
        if let Some(class) = by_extension {
            return class;
        }
    }
    if MAGIC_BINARY
        .iter()
        .any(|magic| head_bytes.starts_with(magic))
    {
        FileClass::Binary
    } else {
        FileClass::Unknown
    }
}

/// Counts observed during one tenant's baseline scan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageReport {
    pub tenant_id: String,
    pub files_by_class: BTreeMap<FileClass, u64>,
    pub expected_sources: u64,
    pub observed_sources: u64,
    pub changed_sources: u64,
}

impl CoverageReport {
    pub fn validate(&self) -> Result<(), HydrationModelError> {
        require_text("tenant_id", &self.tenant_id)?;
        if self.observed_sources > self.expected_sources {
            return Err(HydrationModelError::ObservedExceedsExpected);
        }
        if self.changed_sources > self.observed_sources {
            return Err(HydrationModelError::ChangedExceedsObserved);
        }
        Ok(())
    }
}

/// The bounded stages that a hydration work item can report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagedStage {
    Fetch,
    Parse,
    Enrich,
    Write,
    Reason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HydrationModelError {
    EmptyIdentifier(&'static str),
    NonPositiveBound(&'static str),
    EmptyStages,
    DuplicateManifestKey,
    ObservedExceedsExpected,
    ChangedExceedsObserved,
    ManifestDigestMismatch,
    NoSources,
    MixedTenants,
}

impl fmt::Display for HydrationModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyIdentifier(field) => write!(f, "HYDRATION_ID_EMPTY: {field}"),
            Self::NonPositiveBound(field) => write!(f, "HYDRATION_BOUND_INVALID: {field}"),
            Self::EmptyStages => write!(f, "HYDRATION_STAGES_EMPTY"),
            Self::DuplicateManifestKey => write!(f, "HYDRATION_MANIFEST_DUPLICATE_KEY"),
            Self::ObservedExceedsExpected => {
                write!(f, "HYDRATION_COVERAGE_OBSERVED_EXCEEDS_EXPECTED")
            }
            Self::ChangedExceedsObserved => {
                write!(f, "HYDRATION_COVERAGE_CHANGED_EXCEEDS_OBSERVED")
            }
            Self::ManifestDigestMismatch => write!(f, "HYDRATION_MANIFEST_DIGEST_MISMATCH"),
            Self::NoSources => write!(f, "HYDRATION_SOURCES_EMPTY"),
            Self::MixedTenants => write!(f, "HYDRATION_SOURCES_MIXED_TENANTS"),
        }
    }
}

impl std::error::Error for HydrationModelError {}

fn require_text(field: &'static str, value: &str) -> Result<(), HydrationModelError> {
    if value.trim().is_empty() {
        Err(HydrationModelError::EmptyIdentifier(field))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(uri: &str) -> DeltaManifestKey {
        DeltaManifestKey {
            graph: "workspace".into(),
            category: "source".into(),
            uri: uri.into(),
        }
    }

    // spec: MEDULLA-REPO-INGEST-R650.1
    #[test]
    fn model_types_round_trip_with_stable_enum_names() {
        let plan = BaselinePlan {
            tenant_id: "tenant-a".into(),
            scope: "workspace".into(),
            max_work_items: 8,
            stages: vec![StagedStage::Fetch, StagedStage::Parse, StagedStage::Write],
        };
        plan.validate().unwrap();
        let mut manifest = DeltaManifest::new(plan.tenant_id.clone());
        manifest
            .record(DeltaManifestEntry {
                key: key("repo://b"),
                content_hash: "sha256:b".into(),
            })
            .unwrap();
        manifest
            .record(DeltaManifestEntry {
                key: key("repo://a"),
                content_hash: "sha256:a".into(),
            })
            .unwrap();
        manifest.validate().unwrap();
        let split = RepoSplitPlan {
            tenant_id: plan.tenant_id.clone(),
            repository_uri: "repo://a".into(),
            directory_weights: BTreeMap::from([(String::from("src"), 5)]),
            parts: vec![vec![String::from("src")]],
        };
        split.validate().unwrap();
        let coverage = CoverageReport {
            tenant_id: plan.tenant_id.clone(),
            files_by_class: BTreeMap::from([(FileClass::Source, 1), (FileClass::Document, 1)]),
            expected_sources: 2,
            observed_sources: 1,
            changed_sources: 1,
        };
        coverage.validate().unwrap();

        let encoded = serde_json::to_string(&(plan, manifest, split, coverage)).unwrap();
        assert!(encoded.contains("\"fetch\""));
        assert!(encoded.contains("\"source\""));
        let _: (BaselinePlan, DeltaManifest, RepoSplitPlan, CoverageReport) =
            serde_json::from_str(&encoded).unwrap();
    }

    // spec: MEDULLA-REPO-INGEST-R650.1
    #[test]
    fn invalid_identity_bounds_and_coverage_are_refused() {
        let bad_plan = BaselinePlan {
            tenant_id: " ".into(),
            scope: "workspace".into(),
            max_work_items: 1,
            stages: vec![StagedStage::Fetch],
        };
        assert_eq!(
            bad_plan.validate().unwrap_err(),
            HydrationModelError::EmptyIdentifier("tenant_id")
        );

        let mut duplicate = DeltaManifest::new("tenant-a");
        duplicate.entries.push(DeltaManifestEntry {
            key: key("repo://a"),
            content_hash: "sha256:a".into(),
        });
        duplicate.manifest_digest = duplicate.compute_digest();
        duplicate.entries.push(DeltaManifestEntry {
            key: key("repo://a"),
            content_hash: "sha256:b".into(),
        });
        assert_eq!(
            duplicate.validate().unwrap_err(),
            HydrationModelError::DuplicateManifestKey
        );

        let coverage = CoverageReport {
            tenant_id: "tenant-a".into(),
            files_by_class: BTreeMap::new(),
            expected_sources: 1,
            observed_sources: 2,
            changed_sources: 0,
        };
        assert_eq!(
            coverage.validate().unwrap_err(),
            HydrationModelError::ObservedExceedsExpected
        );
    }
}
