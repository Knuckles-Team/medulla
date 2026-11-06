//! Typed baseline-hydration model (`MEDULLA-REPO-INGEST-R650.1`) and its pure
//! planning functions (`MEDULLA-REPO-INGEST-R650.2`).
//!
//! These values describe a planned hydration pass.  They carry the tenant
//! boundary and durable source identities.  The planners here classify, split
//! and diff in memory only; they never fetch or write.  Submitting the planned
//! work belongs to the server bootstrap wiring (R650.3).

use std::collections::BTreeMap;
use std::fmt;

use medulla_types::contract::Digest256;
use medulla_types::scheduler_config::SchedulerConfig;
use serde::{Deserialize, Serialize};

/// The stable model version for hydration records.
pub const HYDRATION_MODEL_VERSION: &str = "eg-durable/hydration-v1";

const MANIFEST_DIGEST_DOMAIN: &[u8] = b"medulla/hydration/delta-manifest/v1";
const PLAN_DIGEST_DOMAIN: &[u8] = b"medulla/hydration/baseline-plan/v1";

/// Stages every baseline work item runs.
pub const BASELINE_STAGES: [StagedStage; 4] = [
    StagedStage::Fetch,
    StagedStage::Parse,
    StagedStage::Enrich,
    StagedStage::Write,
];

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
    /// The manifest of every source in scope, as observed for this plan.
    pub manifest: DeltaManifest,
    /// Keys that are new or whose content hash changed, in canonical order.
    pub changed: Vec<DeltaManifestKey>,
    /// Number of in-scope sources whose content hash is unchanged.
    pub skipped: u64,
    /// Digest over tenant, scope, manifest digest and changed keys.
    pub plan_digest: Digest256,
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
    /// Digest over the tenant and the sorted entries; independent of the
    /// order the entries were observed in.
    pub manifest_digest: Digest256,
}

impl DeltaManifest {
    /// Build a manifest in canonical order with its digest.
    pub fn new(
        tenant_id: impl Into<String>,
        mut entries: Vec<DeltaManifestEntry>,
    ) -> Result<Self, HydrationModelError> {
        let tenant_id = tenant_id.into();
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        let manifest_digest = compute_manifest_digest(&tenant_id, &entries)?;
        let manifest = Self {
            tenant_id,
            entries,
            manifest_digest,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Entries must be in strictly ascending key order (canonical form) and
    /// the digest must match them.
    pub fn validate(&self) -> Result<(), HydrationModelError> {
        require_text("tenant_id", &self.tenant_id)?;
        for entry in &self.entries {
            entry.validate()?;
        }
        for pair in self.entries.windows(2) {
            match pair[0].key.cmp(&pair[1].key) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => return Err(HydrationModelError::DuplicateManifestKey),
                std::cmp::Ordering::Greater => {
                    return Err(HydrationModelError::ManifestNotCanonical)
                }
            }
        }
        if compute_manifest_digest(&self.tenant_id, &self.entries)? != self.manifest_digest {
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
        self.manifest_digest = compute_manifest_digest(&self.tenant_id, &self.entries)?;
        Ok(())
    }
}

fn compute_manifest_digest(
    tenant_id: &str,
    sorted: &[DeltaManifestEntry],
) -> Result<Digest256, HydrationModelError> {
    let mut fields: Vec<&[u8]> = Vec::with_capacity(1 + sorted.len() * 4);
    fields.push(tenant_id.as_bytes());
    for entry in sorted {
        fields.push(entry.key.graph.as_bytes());
        fields.push(entry.key.category.as_bytes());
        fields.push(entry.key.uri.as_bytes());
        fields.push(entry.content_hash.as_bytes());
    }
    Digest256::framed(MANIFEST_DIGEST_DOMAIN, &fields)
        .map_err(|_| HydrationModelError::DigestFraming)
}

/// A deterministic directory-weight split for one repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSplitPlan {
    pub tenant_id: String,
    pub repository_uri: String,
    /// Directory prefix to estimated work weight (file count).
    pub directory_weights: BTreeMap<String, u64>,
    /// Parts in fill order; each holds whole directories up to the target.
    pub parts: Vec<RepoSplitPart>,
}

/// One split part: whole directories whose file counts fill up to the target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSplitPart {
    pub directories: Vec<String>,
    pub files: u64,
}

/// The files of one repository handed to [`split_repository`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFileSet {
    pub tenant_id: String,
    pub repository_uri: String,
    /// Repository-relative paths with `/` separators.
    pub paths: Vec<String>,
}

/// The sources of one tenant scope handed to [`plan_baseline`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineSources {
    pub tenant_id: String,
    pub scope: String,
    /// The current identity of every source in scope.
    pub entries: Vec<DeltaManifestEntry>,
    /// The manifest committed by the previous run; `None` for an empty store.
    pub previous: Option<DeltaManifest>,
}

/// Per-run hydration evidence (R650.1 model; written by R650.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydrationEvidence {
    pub tenant_id: String,
    pub plan_digest: Digest256,
    pub split_weights: BTreeMap<String, u64>,
    pub skipped: u64,
    pub changed: u64,
    pub coverage_by_class: BTreeMap<FileClass, u64>,
}

impl HydrationEvidence {
    /// Evidence of one planned run from its plan, split and coverage.
    pub fn from_plan(
        plan: &BaselinePlan,
        split: &RepoSplitPlan,
        coverage: &CoverageReport,
    ) -> Self {
        Self {
            tenant_id: plan.tenant_id.clone(),
            plan_digest: plan.plan_digest,
            split_weights: split.directory_weights.clone(),
            skipped: plan.skipped,
            changed: plan.changed.len() as u64,
            coverage_by_class: coverage.files_by_class.clone(),
        }
    }
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
    ManifestNotCanonical,
    ScopeOverCap { items: usize, cap: u32 },
    TenantMismatch,
    DigestFraming,
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
            Self::ManifestDigestMismatch => write!(f, "MANIFEST_DIGEST_MISMATCH"),
            Self::ManifestNotCanonical => write!(f, "HYDRATION_MANIFEST_NOT_CANONICAL"),
            Self::ScopeOverCap { items, cap } => {
                write!(f, "HYDRATION_SCOPE_OVER_CAP: {items} > {cap}")
            }
            Self::TenantMismatch => write!(f, "HYDRATION_TENANT_MISMATCH"),
            Self::DigestFraming => write!(f, "HYDRATION_DIGEST_FRAMING"),
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

/// Plan one tenant scope: refuse a scope over the cap, build the manifest and
/// diff it against the previous one.  Cost is O(files) after the sort.
pub fn plan_baseline(
    sources: &BaselineSources,
    config: &SchedulerConfig,
) -> Result<BaselinePlan, HydrationModelError> {
    require_text("tenant_id", &sources.tenant_id)?;
    require_text("scope", &sources.scope)?;
    if sources.entries.len() > config.hydration_scope_cap as usize {
        return Err(HydrationModelError::ScopeOverCap {
            items: sources.entries.len(),
            cap: config.hydration_scope_cap,
        });
    }
    let manifest = DeltaManifest::new(sources.tenant_id.clone(), sources.entries.clone())?;
    let mut changed = Vec::new();
    let mut skipped = 0u64;
    match &sources.previous {
        None => changed.extend(manifest.entries.iter().map(|entry| entry.key.clone())),
        Some(previous) => {
            if previous.tenant_id != sources.tenant_id {
                return Err(HydrationModelError::TenantMismatch);
            }
            previous.validate()?;
            // Both manifests are in canonical key order: one merge walk, O(files).
            let mut prior = previous.entries.iter().peekable();
            for entry in &manifest.entries {
                while prior.next_if(|old| old.key < entry.key).is_some() {}
                match prior.peek() {
                    Some(old) if old.key == entry.key && old.content_hash == entry.content_hash => {
                        skipped += 1;
                    }
                    _ => changed.push(entry.key.clone()),
                }
            }
        }
    }
    let manifest_hex = manifest.manifest_digest.to_hex();
    let mut fields: Vec<&[u8]> = Vec::with_capacity(3 + changed.len() * 3);
    fields.push(sources.tenant_id.as_bytes());
    fields.push(sources.scope.as_bytes());
    fields.push(manifest_hex.as_bytes());
    for key in &changed {
        fields.push(key.graph.as_bytes());
        fields.push(key.category.as_bytes());
        fields.push(key.uri.as_bytes());
    }
    let plan_digest = Digest256::framed(PLAN_DIGEST_DOMAIN, &fields)
        .map_err(|_| HydrationModelError::DigestFraming)?;
    let plan = BaselinePlan {
        tenant_id: sources.tenant_id.clone(),
        scope: sources.scope.clone(),
        max_work_items: config.hydration_scope_cap,
        stages: BASELINE_STAGES.to_vec(),
        manifest,
        changed,
        skipped,
        plan_digest,
    };
    plan.validate()?;
    Ok(plan)
}

/// Split a repository by top-level directory: directories sorted by file
/// count (descending, then name), filled into parts up to `target` files.  A
/// directory larger than the target forms its own part.
pub fn split_repository(files: &RepoFileSet, target: u32) -> RepoSplitPlan {
    let mut weights: BTreeMap<String, u64> = BTreeMap::new();
    for path in &files.paths {
        let trimmed = path.trim_start_matches('/');
        let directory = match trimmed.split_once('/') {
            Some((head, _)) if !head.is_empty() => head,
            _ => ".",
        };
        *weights.entry(directory.to_string()).or_default() += 1;
    }
    let mut ordered: Vec<(&String, u64)> = weights.iter().map(|(dir, n)| (dir, *n)).collect();
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    let target = u64::from(target.max(1));
    let mut parts: Vec<RepoSplitPart> = Vec::new();
    let mut current = RepoSplitPart {
        directories: Vec::new(),
        files: 0,
    };
    for (directory, count) in ordered {
        if !current.directories.is_empty() && current.files + count > target {
            parts.push(std::mem::replace(
                &mut current,
                RepoSplitPart {
                    directories: Vec::new(),
                    files: 0,
                },
            ));
        }
        current.directories.push(directory.clone());
        current.files += count;
    }
    if !current.directories.is_empty() {
        parts.push(current);
    }
    RepoSplitPlan {
        tenant_id: files.tenant_id.clone(),
        repository_uri: files.repository_uri.clone(),
        directory_weights: weights,
        parts,
    }
}

/// Classify one file: by extension first, then by the leading magic bytes.
pub fn classify_file(path: &str, head_bytes: &[u8]) -> FileClass {
    let name = path.rsplit('/').next().unwrap_or(path);
    if let Some(class) = classify_by_name(name) {
        return class;
    }
    const MAGIC: [(&[u8], FileClass); 6] = [
        (b"%PDF", FileClass::Document),
        (b"PK\x03\x04", FileClass::Binary),
        (b"\x7fELF", FileClass::Binary),
        (b"\x89PNG", FileClass::Binary),
        (b"GIF8", FileClass::Binary),
        (b"\xff\xd8\xff", FileClass::Binary),
    ];
    MAGIC
        .iter()
        .find(|(magic, _)| head_bytes.starts_with(magic))
        .map_or(FileClass::Unknown, |(_, class)| *class)
}

fn classify_by_name(name: &str) -> Option<FileClass> {
    match name {
        "Dockerfile" | "Makefile" | "Containerfile" | "Justfile" => return Some(FileClass::Config),
        _ => {}
    }
    let (_, extension) = name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    let class = match extension.as_str() {
        "rs" | "py" | "pyi" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "go" | "java"
        | "kt" | "kts" | "scala" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "cs" | "rb"
        | "php" | "swift" | "sh" | "bash" | "zsh" | "sql" | "lua" | "zig" | "hs" | "ml" | "ex"
        | "exs" | "erl" | "r" | "jl" | "dart" | "vue" | "svelte" => FileClass::Source,
        "md" | "markdown" | "rst" | "txt" | "adoc" | "org" | "tex" | "pdf" | "html" | "htm"
        | "docx" | "odt" | "rtf" | "epub" => FileClass::Document,
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "json" | "xml" | "properties"
        | "env" | "lock" => FileClass::Config,
        "csv" | "tsv" | "jsonl" | "ndjson" | "parquet" | "arrow" | "avro" | "orc" | "ttl"
        | "nt" | "nq" | "rdf" | "owl" | "db" | "sqlite" => FileClass::Data,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "zip" | "gz" | "tgz" | "bz2" | "xz"
        | "zst" | "tar" | "so" | "dll" | "dylib" | "exe" | "bin" | "wasm" | "o" | "a" | "class"
        | "jar" => FileClass::Binary,
        _ => return None,
    };
    Some(class)
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

    fn entry(uri: &str, hash: &str) -> DeltaManifestEntry {
        DeltaManifestEntry {
            key: key(uri),
            content_hash: hash.into(),
        }
    }

    fn scheduler() -> SchedulerConfig {
        serde_json::from_value(serde_json::json!({
            "total_workers": 4,
            "max_queue_depth": 100
        }))
        .unwrap()
    }

    fn sources(
        entries: Vec<DeltaManifestEntry>,
        previous: Option<DeltaManifest>,
    ) -> BaselineSources {
        BaselineSources {
            tenant_id: "tenant-a".into(),
            scope: "workspace".into(),
            entries,
            previous,
        }
    }

    // spec: MEDULLA-REPO-INGEST-R650.1
    #[test]
    fn model_types_round_trip_with_stable_enum_names() {
        let plan = plan_baseline(
            &sources(
                vec![entry("repo://b", "sha256:b"), entry("repo://a", "sha256:a")],
                None,
            ),
            &scheduler(),
        )
        .unwrap();
        plan.validate().unwrap();
        plan.manifest.validate().unwrap();
        let split = split_repository(
            &RepoFileSet {
                tenant_id: plan.tenant_id.clone(),
                repository_uri: "repo://a".into(),
                paths: vec!["src/lib.rs".into(), "README.md".into()],
            },
            2_000,
        );
        split.validate().unwrap();
        let coverage = CoverageReport {
            tenant_id: plan.tenant_id.clone(),
            files_by_class: BTreeMap::from([(FileClass::Source, 1), (FileClass::Document, 1)]),
            expected_sources: 2,
            observed_sources: 1,
            changed_sources: 1,
        };
        coverage.validate().unwrap();
        let evidence = HydrationEvidence::from_plan(&plan, &split, &coverage);
        assert_eq!(evidence.changed, 2);
        assert_eq!(evidence.skipped, 0);

        let encoded = serde_json::to_string(&(&plan, &split, &coverage, &evidence)).unwrap();
        assert!(encoded.contains("\"fetch\""));
        assert!(encoded.contains("\"source\""));
        assert!(encoded.contains("\"document\""));
        let decoded: (
            BaselinePlan,
            RepoSplitPlan,
            CoverageReport,
            HydrationEvidence,
        ) = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, (plan, split, coverage, evidence));
    }

    // spec: MEDULLA-REPO-INGEST-R650.1
    #[test]
    fn manifest_digest_is_order_independent() {
        let forward = DeltaManifest::new(
            "tenant-a",
            vec![
                entry("repo://a", "h1"),
                entry("repo://b", "h2"),
                entry("repo://c", "h3"),
            ],
        )
        .unwrap();
        let reversed = DeltaManifest::new(
            "tenant-a",
            vec![
                entry("repo://c", "h3"),
                entry("repo://a", "h1"),
                entry("repo://b", "h2"),
            ],
        )
        .unwrap();
        assert_eq!(forward.manifest_digest, reversed.manifest_digest);
        assert_eq!(forward, reversed);

        let mut recorded = DeltaManifest::new("tenant-a", Vec::new()).unwrap();
        for item in [
            entry("repo://b", "h2"),
            entry("repo://c", "h3"),
            entry("repo://a", "h1"),
        ] {
            recorded.record(item).unwrap();
        }
        assert_eq!(recorded.manifest_digest, forward.manifest_digest);

        let other_tenant = DeltaManifest::new("tenant-b", forward.entries.clone()).unwrap();
        assert_ne!(other_tenant.manifest_digest, forward.manifest_digest);
        let changed = DeltaManifest::new(
            "tenant-a",
            vec![
                entry("repo://a", "h1"),
                entry("repo://b", "hX"),
                entry("repo://c", "h3"),
            ],
        )
        .unwrap();
        assert_ne!(changed.manifest_digest, forward.manifest_digest);

        let mut tampered = forward.clone();
        tampered.entries[0].content_hash = "other".into();
        assert_eq!(
            tampered.validate().unwrap_err(),
            HydrationModelError::ManifestDigestMismatch
        );
    }

    // spec: MEDULLA-REPO-INGEST-R650.2
    #[test]
    fn plan_is_deterministic_across_thread_counts() {
        let entries: Vec<DeltaManifestEntry> = (0..400)
            .map(|n| entry(&format!("repo://f{n:04}"), &format!("h{n}")))
            .collect();
        let previous = DeltaManifest::new(
            "tenant-a",
            entries
                .iter()
                .enumerate()
                .map(|(n, item)| {
                    let mut item = item.clone();
                    if n % 20 == 0 {
                        item.content_hash = "stale".into();
                    }
                    item
                })
                .collect(),
        )
        .unwrap();
        let config = scheduler();
        let reference =
            plan_baseline(&sources(entries.clone(), Some(previous.clone())), &config).unwrap();
        assert_eq!(reference.changed.len(), 20);
        assert_eq!(reference.skipped, 380);

        for threads in [1usize, 2, 4, 8] {
            let plans: Vec<BaselinePlan> = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..threads)
                    .map(|t| {
                        let mut shuffled = entries.clone();
                        let len = shuffled.len();
                        shuffled.rotate_left((t * 37) % len);
                        if t % 2 == 1 {
                            shuffled.reverse();
                        }
                        let previous = previous.clone();
                        let config = &config;
                        scope.spawn(move || {
                            plan_baseline(&sources(shuffled, Some(previous)), config).unwrap()
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            for plan in plans {
                assert_eq!(plan, reference);
            }
        }

        let mut small = scheduler();
        small.hydration_scope_cap = 100;
        assert_eq!(
            plan_baseline(&sources(entries.clone(), None), &small).unwrap_err(),
            HydrationModelError::ScopeOverCap {
                items: 400,
                cap: 100
            }
        );

        let files = RepoFileSet {
            tenant_id: "tenant-a".into(),
            repository_uri: "repo://r".into(),
            paths: (0..300)
                .map(|n| format!("{}/f{n}.rs", ["a", "b", "c"][n % 3]))
                .chain((0..150).map(|n| format!("d/f{n}.rs")))
                .chain(std::iter::once("README.md".to_string()))
                .collect(),
        };
        let split = split_repository(&files, 200);
        assert_eq!(split, split_repository(&files, 200));
        assert_eq!(split.directory_weights["d"], 150);
        assert_eq!(split.directory_weights["."], 1);
        assert_eq!(split.parts[0].directories, vec!["d".to_string()]);
        assert!(split.parts.iter().all(|part| part.files <= 200));
        assert_eq!(split.parts.iter().map(|part| part.files).sum::<u64>(), 451);
    }

    // spec: MEDULLA-REPO-INGEST-R650.2
    #[test]
    fn classify_uses_extension_then_magic_bytes() {
        assert_eq!(classify_file("src/lib.rs", b""), FileClass::Source);
        assert_eq!(classify_file("docs/a.MD", b""), FileClass::Document);
        assert_eq!(classify_file("Cargo.toml", b""), FileClass::Config);
        assert_eq!(classify_file("data/x.parquet", b""), FileClass::Data);
        assert_eq!(classify_file("img/x.png", b""), FileClass::Binary);
        assert_eq!(classify_file("blob", b"%PDF-1.7"), FileClass::Document);
        for magic in [
            &b"PK\x03\x04rest"[..],
            b"\x7fELF\x02",
            b"\x89PNG\r\n",
            b"GIF89a",
            b"\xff\xd8\xff\xe0",
        ] {
            assert_eq!(classify_file("blob", magic), FileClass::Binary);
        }
        assert_eq!(classify_file("blob", b"plain"), FileClass::Unknown);
        assert_eq!(classify_file("x.rs", b"%PDF"), FileClass::Source);
    }

    // spec: MEDULLA-REPO-INGEST-R650.1
    #[test]
    fn invalid_identity_bounds_and_coverage_are_refused() {
        let mut bad_sources = sources(vec![entry("repo://a", "h")], None);
        bad_sources.tenant_id = " ".into();
        assert_eq!(
            plan_baseline(&bad_sources, &scheduler()).unwrap_err(),
            HydrationModelError::EmptyIdentifier("tenant_id")
        );

        assert_eq!(
            DeltaManifest::new(
                "tenant-a",
                vec![entry("repo://a", "sha256:a"), entry("repo://a", "sha256:b")]
            )
            .unwrap_err(),
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
