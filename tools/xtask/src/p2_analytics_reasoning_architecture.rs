//! P2 analytics and incremental reasoning architecture gate.
//!
//! Each source that carries part of the analytics/reasoning contract must contain its declared
//! literal markers, read over the compiler-declared module tree (test-inclusive, so a required
//! test name counts). Three ordering/authority contracts no single marker expresses are checked on
//! top: no publication-completion method accepts a `Running` job, the handler stages a result before
//! completing publication, and the reasoning worker persists its index before acknowledging the
//! outbox. An absent marker is FATAL for every offset-based contract: a `find` that returns nothing
//! must never leave an empty slice that cannot fire.

use std::collections::BTreeMap;
use std::path::Path;

use crate::analysis::{module_tree, read};

pub const NAME: &str = "p2-analytics-reasoning-architecture";
pub const SUMMARY: &str =
    "P2 analytics and incremental reasoning architecture markers and ordering";

/// Every source that must carry a declared set of literal markers, in report order.
const REQUIRED_MARKERS: &[(&str, &[&str])] = &[
    (
        "crates/medulla-jobs/src/model.rs",
        &[
            r#"Publishing {"#,
            r#"WorkerLease"#,
            r#"content_digest"#,
            r#"not_before_ms"#,
        ],
    ),
    (
        "crates/medulla-jobs/src/store.rs",
        &[
            r#"claim_next("#,
            r#"renew_lease("#,
            r#"checkpoint_fenced("#,
            r#"stage_result_fenced("#,
            r#"complete_publication_fenced("#,
            r#"complete_publication_prepared("#,
            r#"release_publication_lease_fenced("#,
            r#"fail_attempt_fenced("#,
            r#"require_lease_ownership("#,
            r#"cancellation_reconcile"#,
            r#"job_id_for_batch("#,
            r#"replicated_submission_and_claim_converge_across_independent_projections"#,
        ],
    ),
    (
        "crates/medulla-h-jobs/src/jobs.rs",
        &[
            r#"mine_labeled_cancellable("#,
            r#"TenantJobQuota"#,
            r#"stage_result_fenced("#,
            r#"commit_internal_graph_methods("#,
            r#"complete_publication_fenced("#,
            r#"verified v2 RequestContext"#,
            r#"validate_remote_result_privacy("#,
            r#"association_rule_id_from_row("#,
            r#"refresh_job_metrics("#,
            r#"std::env::var("MEDULLA_ANALYTICS_WORKERS")"#,
            r#"for slot in 0..workers"#,
            r#"analytics jobs require a configured durable persistence directory"#,
            r#""kernel_cancelled""#,
            r#""invalid_payload""#,
            r#"PreparedJobPublication"#,
            r#"RoutedJobPublication"#,
            r#"FinalizeJobPublication"#,
            r#"apply_consensus_job_publication_commit("#,
            r#"apply_consensus_job_publication_finalize("#,
        ],
    ),
    (
        "crates/medulla-server-consensus/src/dispatch.rs",
        &[
            r#"execute_consensus_job_publication("#,
            r#"submit_consensus_job_publication_command("#,
            r#""target-commit""#,
            r#""scheduler-finalize""#,
        ],
    ),
    (
        "crates/medulla-raft/src/raft/mod.rs",
        &[r#"JobPublicationCommit {"#, r#"JobPublicationFinalize {"#],
    ),
    (
        "crates/medulla-work-types/src/jobs.rs",
        &[
            r#"WorkerClaim {"#,
            r#"WorkerRenew {"#,
            r#"WorkerCheckpoint {"#,
            r#"WorkerStage {"#,
            r#"WorkerPublish {"#,
            r#"WorkerFail {"#,
            r#"WorkerCancel {"#,
            r#"pub struct JobResult"#,
            r#"pub memory_bytes: Option<u64>"#,
            r#"pub required_capabilities: Vec<String>"#,
        ],
    ),
    (
        "crates/medulla-server-api/src/authority_context.rs",
        &[
            r#"allows_analytics_worker"#,
            r#"allows_action("analytics:worker")"#,
        ],
    ),
    (
        "crates/medulla-h-retrieve/src/reasoning_projection.rs",
        &[
            r#"claim_mutation_outbox("#,
            r#"read_mutation_batch("#,
            r#"ReasoningProjectionWakeup"#,
            r#"operations_sha256"#,
            r#"read_mutation_projection_cursor("#,
            r#"persist_index("#,
            r#"read_index("#,
            r#"claim_recompute("#,
            r#"authoritative graph version changed"#,
            r#"file.sync_all()"#,
            r#"ack_mutation_outbox("#,
        ],
    ),
    (
        "crates/medulla-epistemic/src/incremental.rs",
        &[
            r#"IncrementalReasoningIndex"#,
            r#"stale_dependents"#,
            r#"CONTRADICTS"#,
            r#"CAUSES"#,
            r#"opaque_identity"#,
            r#"refresh_materialization"#,
            r#"ProjectionInvalidationKind"#,
            r#"recompute_fences"#,
            r#"IncrementalReasoningEvent"#,
            r#"apply_wakeup("#,
            r#"IncrementalReasoningEvent::Recompute"#,
            r#"recompute_from_ref("#,
        ],
    ),
    (
        "crates/medulla-h-query/src/query.rs",
        &[
            r#""reasoning-recompute""#,
            r#"status: "Queued".to_string()"#,
            r#"projection_pending: true"#,
        ],
    ),
    (
        "Cargo.toml",
        &[r#""jobs""#, r#""epistemic-tms""#, r#""epistemic-causal""#],
    ),
];

/// `Succeeded` is reachable only from `Publishing`, so no publication-completion entry point may
/// carry a `Running` source arm.
const PUBLICATION_COMPLETION_FNS: [&str; 2] = [
    "pub fn complete_publication_fenced(",
    "pub fn complete_publication_prepared(",
];
const RUNNING_SOURCE_ARM: &str = "JobState::Running {";
const JOB_STORE: &str = "crates/medulla-jobs/src/store.rs";
const JOBS_HANDLER: &str = "crates/medulla-h-jobs/src/jobs.rs";
const REASONING: &str = "crates/medulla-h-retrieve/src/reasoning_projection.rs";

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    let failures = check(root)?;
    if failures.is_empty() {
        println!("P2 analytics/reasoning architecture gate passed");
        return Ok(());
    }
    Err(format!(
        "{}\n{} finding(s)",
        failures
            .iter()
            .map(|f| format!("- {f}"))
            .collect::<Vec<_>>()
            .join("\n"),
        failures.len()
    ))
}

/// Every finding for the tree at `root`; an unreadable module tree is an error, not a finding.
pub fn check(root: &Path) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();
    let mut sources = BTreeMap::new();
    for (path, markers) in REQUIRED_MARKERS {
        sources.insert(
            (*path).to_string(),
            require(root, path, markers, &mut failures)?,
        );
    }
    failures.extend(analytics_ordering_failures(&sources));
    failures.extend(full_build_failures(&sources));
    Ok(failures)
}

/// The compiler-declared source of a Rust module (test-inclusive), or a non-Rust input verbatim.
fn source(root: &Path, path: &str) -> Result<String, String> {
    if path.ends_with(".rs") {
        module_tree::read(root, path, true)
    } else {
        read(root, path)
    }
}

/// Read `path` and report every needle it lacks.
pub fn require(
    root: &Path,
    path: &str,
    needles: &[&str],
    failures: &mut Vec<String>,
) -> Result<String, String> {
    let text = source(root, path)?;
    for needle in needles {
        if !text.contains(needle) {
            failures.push(format!("{path}: missing {needle:?}"));
        }
    }
    Ok(text)
}

/// One `impl` method's text, or None when its signature is absent. The body ends at the first
/// impl-level closing brace, so a following method's doc comment is never read as part of it.
fn method_body<'a>(source: &'a str, signature: &str) -> Option<&'a str> {
    let start = source.find(signature)?;
    const CLOSE: &str = "\n    }\n";
    Some(match source[start..].find(CLOSE) {
        Some(end) => &source[start..start + end + CLOSE.len()],
        None => &source[start..],
    })
}

/// `first` must appear before `second`; an absent marker is a finding, not a pass.
fn ordered_markers(
    source: &str,
    path: &str,
    first: &str,
    second: &str,
    violation: &str,
) -> Vec<String> {
    for marker in [first, second] {
        if !source.contains(marker) {
            return vec![format!(
                "{path}: cannot locate {marker:?} to check its ordering contract"
            )];
        }
    }
    if source.find(first) > source.find(second) {
        vec![violation.to_string()]
    } else {
        vec![]
    }
}

/// No publication-completion method may accept a Running job.
fn publication_barrier_failures(store: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for signature in PUBLICATION_COMPLETION_FNS {
        match method_body(store, signature) {
            None => failures.push(format!(
                "{JOB_STORE}: cannot locate {signature:?}; the publication barrier cannot be checked"
            )),
            Some(body) if body.contains(RUNNING_SOURCE_ARM) => failures.push(format!(
                "{JOB_STORE}: {} still permits Running -> Succeeded",
                &signature["pub fn ".len()..signature.len() - 1]
            )),
            Some(_) => {}
        }
    }
    failures
}

fn analytics_ordering_failures(sources: &BTreeMap<String, String>) -> Vec<String> {
    let handler = &sources[JOBS_HANDLER];
    let reasoning = &sources[REASONING];
    let mut failures = publication_barrier_failures(&sources[JOB_STORE]);
    failures.extend(ordered_markers(
        handler,
        JOBS_HANDLER,
        "stage_result_fenced(",
        "complete_publication_fenced(",
        "handler completes publication before staging the typed result",
    ));
    failures.extend(ordered_markers(
        reasoning,
        REASONING,
        "persist_index(",
        "ack_mutation_outbox(",
        "reasoning cursor can advance before projection persistence",
    ));
    if reasoning.contains("from_slice::<Vec<MutationOperation>>(&lease.record.intent.payload)") {
        failures.push("reasoning worker treats digest-only outbox payload as mutation data".into());
    }
    if reasoning.contains("global_index()") {
        failures.push("reasoning worker references a process-global TMS authority".into());
    }
    failures
}

/// The `full` feature must build the analytics/reasoning stack it declares.
fn full_build_failures(sources: &BTreeMap<String, String>) -> Vec<String> {
    let full_line = sources["Cargo.toml"]
        .lines()
        .find(|l| l.starts_with("full ="))
        .unwrap_or("");
    let mut failures: Vec<String> = ["jobs", "epistemic-tms", "epistemic-causal"]
        .iter()
        .filter(|feature| !full_line.contains(&format!("\"{feature}\"")))
        .map(|feature| format!("full build omits {feature}"))
        .collect();
    if !sources["crates/medulla-jobs/src/model.rs"].contains("Publishing") {
        failures.push("analytics state machine has no publication barrier".into());
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    /// A job store whose two publication-completion methods both require Publishing.
    const CLEAN_STORE: &str = r#"
impl JobStore {
    pub fn complete_publication_fenced(&self, job_id: &str) -> Result<AnalyticsJob> {
        let (result_ref, checkpoint) = match &job.state {
            JobState::Publishing { result_ref, checkpoint } => (result_ref, checkpoint),
            other => return Err(invalid_transition(other)),
        };
        Ok(job)
    }

    /// A doc comment that mentions JobState::Running { checkpoint } on purpose:
    /// it belongs to the NEXT method and must never be read as this one's body.
    pub fn complete_publication_prepared(&self, job_id: &str) -> Result<AnalyticsJob> {
        match &job.state {
            JobState::Publishing { .. } => {}
            _ => return Err(invalid_transition(&job, "requires Publishing")),
        }
        Ok(job)
    }
}
"#;

    #[test]
    fn clean_publication_barrier_reports_nothing() {
        assert!(publication_barrier_failures(CLEAN_STORE).is_empty());
    }

    #[test]
    fn permissive_running_arm_fails_the_barrier() {
        for signature in PUBLICATION_COMPLETION_FNS {
            let head = format!("{signature}&self, job_id: &str) -> Result<AnalyticsJob> {{");
            let permissive = CLEAN_STORE.replacen(
                &head,
                &format!("{head}\n        let x = match &job.state {{\n            JobState::Running {{ checkpoint }} => checkpoint,\n        }};"),
                1,
            );
            assert_ne!(permissive, CLEAN_STORE);
            let failures = publication_barrier_failures(&permissive);
            assert_eq!(failures.len(), 1, "{failures:?}");
            assert!(failures[0].contains("still permits Running -> Succeeded"));
        }
    }

    #[test]
    fn a_missing_completion_entry_point_is_fatal() {
        for signature in PUBLICATION_COMPLETION_FNS {
            let renamed = CLEAN_STORE.replacen(signature, "pub fn renamed_away(", 1);
            let failures = publication_barrier_failures(&renamed);
            assert_eq!(failures.len(), 1, "{failures:?}");
            assert!(failures[0].contains("cannot locate"));
            assert!(failures[0].contains("publication barrier cannot be checked"));
        }
    }

    #[test]
    fn ordered_markers_accepts_the_declared_order() {
        assert!(ordered_markers("first() second()", "p.rs", "first(", "second(", "V").is_empty());
    }

    #[test]
    fn ordered_markers_reports_a_swapped_order() {
        assert_eq!(
            ordered_markers("second() first()", "p.rs", "first(", "second(", "V"),
            ["V"]
        );
    }

    #[test]
    fn ordered_markers_fails_closed_on_an_absent_marker() {
        for present in ["first(", "second("] {
            let failures = ordered_markers(present, "p.rs", "first(", "second(", "V");
            assert_eq!(failures.len(), 1);
            assert!(failures[0].contains("ordering contract"));
        }
    }

    #[test]
    fn required_marker_table_shape_is_pinned() {
        assert_eq!(REQUIRED_MARKERS.len(), 11, "marker table row count changed");
        assert_eq!(
            REQUIRED_MARKERS.iter().map(|(_, m)| m.len()).sum::<usize>(),
            82
        );
        let mut paths: Vec<_> = REQUIRED_MARKERS.iter().map(|(p, _)| *p).collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            [
                "Cargo.toml",
                "crates/medulla-epistemic/src/incremental.rs",
                "crates/medulla-h-jobs/src/jobs.rs",
                "crates/medulla-h-query/src/query.rs",
                "crates/medulla-h-retrieve/src/reasoning_projection.rs",
                "crates/medulla-jobs/src/model.rs",
                "crates/medulla-jobs/src/store.rs",
                "crates/medulla-raft/src/raft/mod.rs",
                "crates/medulla-server-api/src/authority_context.rs",
                "crates/medulla-server-consensus/src/dispatch.rs",
                "crates/medulla-work-types/src/jobs.rs",
            ]
        );
        assert!(REQUIRED_MARKERS
            .iter()
            .all(|(_, m)| !m.is_empty() && m.iter().all(|x| !x.is_empty())));
    }

    #[test]
    fn reads_the_declared_consensus_child_and_rejects_a_lost_call_body() {
        let t = Tree::new(&[
            ("src/server/dispatch.rs", "mod consensus;\n"),
            (
                "src/server/dispatch/consensus.rs",
                "pub fn execute_consensus_job_publication() {\n    let _ = \"target-commit\";\n    let _ = \"scheduler-finalize\";\n}\n",
            ),
        ]);
        let needles = [
            "execute_consensus_job_publication(",
            "\"target-commit\"",
            "\"scheduler-finalize\"",
        ];
        let mut failures = Vec::new();
        require(t.path(), "src/server/dispatch.rs", &needles, &mut failures).unwrap();
        assert!(failures.is_empty(), "{failures:?}");

        std::fs::write(
            t.path().join("src/server/dispatch/consensus.rs"),
            "pub fn consensus_publication_removed() {}\n",
        )
        .unwrap();
        require(
            t.path(),
            "src/server/dispatch.rs",
            &["execute_consensus_job_publication("],
            &mut failures,
        )
        .unwrap();
        assert_eq!(
            failures,
            ["src/server/dispatch.rs: missing \"execute_consensus_job_publication(\""]
        );
    }

    #[test]
    fn passes_on_the_current_tree() {
        let findings = check(&crate::workspace_root()).unwrap();
        assert!(findings.is_empty(), "{findings:?}");
    }
}
