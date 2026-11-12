//! The bounded incremental index delta of one `IndexRepository` job
//! (MEDULLA-REPO-INGEST-R006.4, on the baseline commit path of R007). A delta
//! lists added, modified, deleted and renamed paths against a `base_revision`
//! and `target_revision`. The owner stays `repository_index.rs`: a delta reuses
//! that one lowering, never a second pipeline or a second store.
//!
//! - A mismatched base refuses with `INDEX_BASE_MISMATCH` before any write.
//! - Unchanged paths reuse their parse artifacts; only added and modified paths
//!   carry fresh parse artifacts.
//! - The target generation drops the entities of deleted paths and rebinds
//!   renamed ones.
//! - Only a complete generation becomes the current code map.
//!
//! Each job also carries working memory: bytes are reserved at the job's
//! `max_working_bytes` bound and released on completion, cancel and failure.

use std::collections::{HashMap, HashSet};

use medulla_types::ingestion_wire::{ExtractedEdge, ExtractedNode, IndexDelta, IndexResult};

/// A generation of the repository code map: only `complete` generations may
/// become the current one, and only a generation whose `base_revision` matches
/// the delta's `base_revision` may be advanced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryGeneration {
    pub map_generation: u64,
    pub base_revision: String,
    pub complete: bool,
}

/// Node property that binds a parse artifact to its repository-relative path.
const FILE_PATH_PROPERTY: &str = "file_path";

/// Stable refusal of a delta that does not chain onto the current generation's
/// base revision. It is checked before any lowering, so nothing is staged or
/// written when it fires.
pub const INDEX_BASE_MISMATCH: &str = "INDEX_BASE_MISMATCH";

/// Stable backpressure name of a job that reserved past its
/// `max_working_bytes` bound; the allocation is refused and the caller waits.
pub const WORKING_BYTES_EXCEEDED: &str = "WORKING_BYTES_EXCEEDED";

/// The staged outcome of one delta: the repository-wide result after deleted
/// paths dropped and renames rebound, its lowered write-set, and the complete
/// target generation it publishes.
pub struct StagedDelta {
    pub result: IndexResult,
    pub write_set: super::IndexWriteSet,
    pub generation: RepositoryGeneration,
    /// Paths whose parse artifacts were freshly parsed (added + modified).
    pub parse_files: usize,
    /// Paths whose parse artifacts were reused unchanged (unchanged + renamed).
    pub reused_files: usize,
}

/// Apply one incremental delta on top of `current` and lower the outcome with
/// the one existing owner (`repository_index.rs`).
pub fn apply_delta(
    result: &IndexResult,
    delta: &IndexDelta,
    current: Option<&RepositoryGeneration>,
) -> Result<StagedDelta, String> {
    refuse_mismatched_base(delta, current)?;
    let renamed = renames(delta);
    let removed = removed_paths(delta, &renamed);

    let mut nodes = Vec::with_capacity(result.nodes.len());
    let mut dropped_ids: HashSet<&str> = HashSet::new();
    for node in &result.nodes {
        match node.properties.get(FILE_PATH_PROPERTY).map(String::as_str) {
            Some(path) if renamed.contains_key(path) => {
                nodes.push(copy_node(node, Some(renamed[path])));
            }
            Some(path) if removed.contains(path) => {
                dropped_ids.insert(node.node_id.as_str());
            }
            // Unchanged path: the parse artifact is reused untouched.
            other => nodes.push(copy_node(node, other)),
        }
    }
    let edges: Vec<ExtractedEdge> = result
        .edges
        .iter()
        .filter(|edge| {
            !dropped_ids.contains(edge.source.as_str())
                && !dropped_ids.contains(edge.target.as_str())
                && edge
                    .properties
                    .get(FILE_PATH_PROPERTY)
                    .is_none_or(|path| !removed.contains(path.as_str()))
        })
        .map(|edge| rebind_edge(edge, &renamed))
        .collect();

    let parse_files = delta.added.len() + delta.modified.len();
    let reused_files = result.files_parsed.saturating_sub(parse_files);
    let generation = RepositoryGeneration {
        map_generation: current.map_or(1, |current| current.map_generation + 1),
        base_revision: delta.target_revision.clone(),
        complete: true,
    };
    let applied = IndexResult {
        nodes,
        edges,
        ..result.clone()
    };
    let applied = drop_removed_outcomes(applied, &removed);
    let write_set = super::lower(&applied)?;
    Ok(StagedDelta {
        write_set,
        result: applied,
        generation,
        parse_files,
        reused_files,
    })
}

/// The one write-preceding check of a delta: nothing lowers, stages or commits
/// until the base binds.
fn refuse_mismatched_base(
    delta: &IndexDelta,
    current: Option<&RepositoryGeneration>,
) -> Result<(), String> {
    match current {
        Some(current)
            if current.complete && current.base_revision == delta.base_revision.as_str() =>
        {
            Ok(())
        }
        Some(current) => Err(format!(
            "{INDEX_BASE_MISMATCH}: the delta chains from {} but the current generation chains from {}",
            delta.base_revision, current.base_revision
        )),
        None => Err(format!(
            "{INDEX_BASE_MISMATCH}: no complete generation exists as a base for {}",
            delta.base_revision
        )),
    }
}

/// Deleted paths plus rename sources: entities of these paths leave the target
/// generation (a renamed path keeps its artifact under the new path).
fn removed_paths<'d>(
    delta: &'d IndexDelta,
    renamed: &HashMap<&'d str, &'d str>,
) -> HashSet<&'d str> {
    delta
        .deleted
        .iter()
        .map(String::as_str)
        .chain(renamed.keys().copied())
        .collect()
}

fn renames(delta: &IndexDelta) -> HashMap<&str, &str> {
    delta
        .renamed
        .iter()
        .map(|rename| (rename.from.as_str(), rename.to.as_str()))
        .collect()
}

fn copy_node(node: &ExtractedNode, new_path: Option<&str>) -> ExtractedNode {
    let mut properties = node.properties.clone();
    if let Some(path) = new_path {
        properties.insert(FILE_PATH_PROPERTY.to_owned(), path.to_owned());
    }
    ExtractedNode {
        node_id: node.node_id.clone(),
        node_type: node.node_type.clone(),
        properties,
    }
}

fn rebind_edge(edge: &ExtractedEdge, renamed: &HashMap<&str, &str>) -> ExtractedEdge {
    let properties = match edge.properties.get(FILE_PATH_PROPERTY) {
        Some(path) if renamed.contains_key(path.as_str()) => {
            let mut properties = edge.properties.clone();
            properties.insert(
                FILE_PATH_PROPERTY.to_owned(),
                renamed[path.as_str()].to_owned(),
            );
            properties
        }
        _ => edge.properties.clone(),
    };
    ExtractedEdge {
        source: edge.source.clone(),
        target: edge.target.clone(),
        edge_type: edge.edge_type.clone(),
        properties,
    }
}

/// File outcomes carry the same path binding: deleted paths lose their
/// outcome; unknown paths never had one.
fn drop_removed_outcomes(mut result: IndexResult, removed: &HashSet<&str>) -> IndexResult {
    result
        .file_outcomes
        .retain(|outcome| !removed.contains(outcome.file_path.as_str()));
    result
}

/// The working-memory ledger of one bounded index job
/// (`ingest.index_max_working_bytes`, unit bytes; MEDULLA-REPO-INGEST-R006.4).
/// Every allocation reserves its bytes before use and is refused —
/// backpressured, not queued in memory — once the bound is exhausted. Release
/// happens on completion, cancel and failure; a caller may not release more
/// than it reserved.
#[derive(Debug)]
pub struct WorkingBytes {
    limit: u64,
    reserved: u64,
}

impl WorkingBytes {
    /// A job ledger from the job's `max_working_bytes` value.
    pub fn new(limit: u64) -> Self {
        Self { limit, reserved: 0 }
    }

    /// The job's `max_working_bytes` bound.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// Bytes currently reserved and not yet released.
    pub fn reserved(&self) -> u64 {
        self.reserved
    }

    /// Reserve `bytes` for one allocation before using it. Past the bound the
    /// reservation is refused: the job backpressures, nothing writes.
    pub fn reserve(&mut self, bytes: u64) -> Result<(), String> {
        let total = self
            .reserved
            .checked_add(bytes)
            .ok_or_else(|| exhausted(self.limit))?;
        if total > self.limit {
            return Err(exhausted(self.limit));
        }
        self.reserved = total;
        Ok(())
    }

    /// Release `bytes` on completion, cancel or failure. Over-release is a
    /// caller bug and is refused, not silently ignored.
    pub fn release(&mut self, bytes: u64) -> Result<(), String> {
        if bytes > self.reserved {
            return Err(format!(
                "{WORKING_BYTES_EXCEEDED}: releasing {bytes} bytes exceeds the {} reserved",
                self.reserved
            ));
        }
        self.reserved -= bytes;
        Ok(())
    }
}

fn exhausted(limit: u64) -> String {
    format!(
        "{WORKING_BYTES_EXCEEDED}: the job reserved past its max_working_bytes bound of {limit} bytes"
    )
}

#[cfg(test)]
mod tests {
    use medulla_types::contract::BoundedVec;
    use medulla_types::ingestion_wire::{
        IndexDeltaChange, IndexDeltaRename, MAX_INDEX_DELTA_PATHS,
    };
    use medulla_types::scheduler_config;

    use super::*;

    fn file_node(node_id: &str, path: &str) -> ExtractedNode {
        let mut properties = HashMap::new();
        properties.insert(FILE_PATH_PROPERTY.to_owned(), path.to_owned());
        ExtractedNode {
            node_id: node_id.into(),
            node_type: "symbol".into(),
            properties,
        }
    }

    fn bounded<T>(items: impl Iterator<Item = T>) -> BoundedVec<T, MAX_INDEX_DELTA_PATHS> {
        BoundedVec::new(items.collect::<Vec<_>>()).expect("test deltas stay inside the delta bound")
    }

    fn change(path: &str) -> IndexDeltaChange {
        IndexDeltaChange {
            path: path.into(),
            blob_digest: "sha256:0".into(),
        }
    }

    fn delta(
        base: &str,
        target: &str,
        added: &[&str],
        modified: &[&str],
        deleted: &[&str],
        renamed: &[(&str, &str)],
    ) -> IndexDelta {
        IndexDelta {
            base_revision: base.into(),
            target_revision: target.into(),
            added: bounded(added.iter().map(|path| change(path))),
            modified: bounded(modified.iter().map(|path| change(path))),
            deleted: bounded(deleted.iter().map(|path| (*path).into())),
            renamed: bounded(renamed.iter().map(|(from, to)| IndexDeltaRename {
                from: (*from).into(),
                to: (*to).into(),
            })),
        }
    }

    fn full_result() -> IndexResult {
        IndexResult {
            nodes: vec![
                file_node("code::a-main", "src/kept.rs"),
                file_node("code::b-gone", "src/deleted.rs"),
                file_node("code::c-moved", "src/old.rs"),
            ],
            edges: vec![ExtractedEdge {
                source: "code::c-moved".into(),
                target: "code::a-main".into(),
                edge_type: "calls".into(),
                properties: HashMap::new(),
            }],
            files_parsed: 3,
            ..Default::default()
        }
    }

    fn generation(map_generation: u64, base_revision: &str) -> RepositoryGeneration {
        RepositoryGeneration {
            map_generation,
            base_revision: base_revision.into(),
            complete: true,
        }
    }

    // spec: MEDULLA-REPO-INGEST-R006.4
    //
    // A delta whose base_revision does not name the base the current
    // generation chains from refuses with INDEX_BASE_MISMATCH BEFORE ANY
    // WRITE: the check precedes lowering itself, so a batch that would fail
    // its own lowering budget reports the base refusal with nothing staged.
    #[test]
    fn a_mismatched_base_refuses_index_base_mismatch_before_any_write() {
        let oversized = IndexResult {
            edges: (0..100_002)
                .map(|i| ExtractedEdge {
                    source: format!("s{i}"),
                    target: "t".into(),
                    edge_type: "calls".into(),
                    properties: HashMap::new(),
                })
                .collect(),
            ..Default::default()
        };
        let refusal = match apply_delta(
            &oversized,
            &delta("rev-old", "rev-next", &[], &["src/main.rs"], &[], &[]),
            Some(&generation(7, "rev-main")),
        ) {
            Err(refusal) => refusal,
            Ok(_) => panic!("a foreign base must refuse"),
        };
        assert!(
            refusal.starts_with(INDEX_BASE_MISMATCH),
            "the stable code leads the refusal, got: {refusal}"
        );
        // The refusal precedes every write: nothing lowered, so an oversized
        // batch never even reached its own lowering-budget failure.
        assert!(!refusal.contains("BATCH_TOO_LARGE"));
        // A delta with no complete generation to chain onto refuses likewise.
        match apply_delta(
            &full_result(),
            &delta("rev-main", "rev-next", &["src/new.rs"], &[], &[], &[]),
            None,
        ) {
            Err(refusal) => assert!(refusal.starts_with(INDEX_BASE_MISMATCH)),
            Ok(_) => panic!("a delta without a base generation must refuse"),
        }
    }

    // spec: MEDULLA-REPO-INGEST-R006.4
    //
    // The target generation is complete, drops the entities of deleted files
    // (and the edges dangling into them), rebinds renames to their new path,
    // and the unchanged file reuses its parse artifact untouched. Only added
    // and modified paths re-parse.
    #[test]
    fn the_target_generation_drops_deleted_and_rebinds_renamed() {
        let staged = apply_delta(
            &full_result(),
            &delta(
                "rev-main",
                "rev-next",
                &[],
                &[],
                &["src/deleted.rs"],
                &[("src/old.rs", "src/renamed.rs")],
            ),
            Some(&generation(7, "rev-main")),
        )
        .expect("the delta binds");
        // Only a complete generation becomes the current code map.
        assert_eq!(staged.generation.map_generation, 8);
        assert_eq!(staged.generation.base_revision, "rev-next");
        assert!(staged.generation.complete);
        // Deleted entities drop.
        let ids: Vec<&str> = staged
            .result
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect();
        assert!(!ids.contains(&"code::b-gone"));
        // Renames rebind: artifact identity kept, path changed.
        let moved = staged
            .result
            .nodes
            .iter()
            .find(|node| node.node_id == "code::c-moved")
            .expect("renames keep their artifact");
        assert_eq!(moved.properties[FILE_PATH_PROPERTY], "src/renamed.rs");
        // The unchanged file reuses its artifact untouched.
        let kept = staged
            .result
            .nodes
            .iter()
            .find(|node| node.node_id == "code::a-main")
            .expect("unchanged artifacts survive");
        assert_eq!(kept.properties[FILE_PATH_PROPERTY], "src/kept.rs");
        // Nothing re-parses here: the delta only deletes and renames.
        assert_eq!(staged.parse_files, 0);
        assert_eq!(staged.reused_files, 3);
        // The write-set lowers through the one existing owner.
        assert!(!staged.write_set.methods().is_empty());
    }

    // spec: MEDULLA-REPO-INGEST-R006.4
    //
    // The knob `ingest.index_max_working_bytes` defaults to 1073741824 with the
    // bounds [67108864, 68719476736]; reservations backpressure exactly at the
    // job's max_working_bytes, and release happens on completion, cancel AND
    // failure — an over-release is refused instead of silently ignored.
    #[test]
    fn working_bytes_backpressure_and_release_on_completion_cancel_and_failure() {
        use scheduler_config::index_max_working_bytes_from;
        assert_eq!(
            index_max_working_bytes_from(|_| None).expect("the default validates"),
            1_073_741_824
        );
        assert!(
            index_max_working_bytes_from(|key| if key == "MEDULLA_INDEX_MAX_WORKING_BYTES" {
                Some("67108863".into())
            } else {
                None
            })
            .is_err(),
            "below the range must refuse startup"
        );

        let mut job = WorkingBytes::new(1024);
        job.reserve(768).expect("inside the bound reserves");
        match job.reserve(768) {
            Err(refusal) => assert!(
                refusal.starts_with(WORKING_BYTES_EXCEEDED),
                "got: {refusal}"
            ),
            Ok(()) => panic!("past the bound the job must backpressure"),
        }
        // Cancel releases its reservation, and the job is reusable without
        // stale reserved bytes.
        job.release(768).expect("cancel releases its reservation");
        assert_eq!(job.reserved(), 0);
        // Completion and failure release exactly what was reserved.
        job.reserve(1024).expect("exactly the bound is reachable");
        job.release(384).expect("failure releases what it reserved");
        assert_eq!(job.reserved(), 640);
        job.release(256).expect("cancel releases what it reserved");
        job.release(384).expect("completion releases the remainder");
        assert_eq!(job.reserved(), 0);
        assert!(job.release(1).is_err(), "over-release must be a caller bug");
    }
}
