"""Static ownership checks for the native WorkItem/development-lane routes."""

from __future__ import annotations

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
HANDLERS = ROOT / "crates" / "medulla-h-jobs" / "src"

pytestmark = pytest.mark.no_engine

WORK_ITEM_METHODS = {
    "ClaimWorkItem",
    "RenewWorkItemLease",
    "CommitWorkItemResult",
    "CancelWorkItem",
    "DeferWorkItem",
    "CasWorkItemMetadata",
    # graph-os MEDULLA-2: native control-lease writes share the WorkItem kernel.
    "IssueHostStartGrant",
    "IssueControlLease",
    "TransitionControlLease",
    # MEDULLA-DURABLE-KERNEL-R028.5: fenced snapshots use the WorkItem write family.
    "PublishCurrentPlan",
    # MEDULLA-DURABLE-KERNEL-R028.1: checkpoints share the same fenced kernel.
    "PublishRunCheckpoint",
    # EH-346: the engine-internal policy-record store shares the kernel.
    "PolicyEvolutionStore",
    "GapUpsert",
    "GapTransition",
    "GapSettle",
    "WorkOfferPut",
    "RetireSealedRecord",
}
DEVELOPMENT_LANE_WRITES = {
    "ReserveDevelopmentLane",
    "RenewDevelopmentLane",
    "ObserveDevelopmentLane",
    "FinishDevelopmentLane",
    "CleanupDevelopmentLane",
    "UpdateDevelopmentLaneQuota",
}
DEVELOPMENT_LANE_READS = {"QueryDevelopmentLane", "DevelopmentLaneStatus"}


def _owned_methods(path: Path) -> set[str]:
    return set(re.findall(r"Method::([A-Za-z0-9_]+)", path.read_text()))


def test_work_item_handler_explicitly_owns_its_transitions() -> None:
    source = (HANDLERS / "work_item.rs").read_text()
    assert _owned_methods(HANDLERS / "work_item.rs") == WORK_ITEM_METHODS
    assert "mutation_batch::commit_work_item" in source
    assert "_ =>" not in source
    assert source.count("other => return Err(other)") == 1


def test_development_lane_handler_has_six_explicit_writes_and_two_reads() -> None:
    source = (HANDLERS / "development_lane.rs").read_text()
    assert _owned_methods(HANDLERS / "development_lane.rs") == (
        DEVELOPMENT_LANE_WRITES | DEVELOPMENT_LANE_READS
    )
    assert "commit_development_lane" in source
    assert "_ =>" not in source
    assert source.count("other => return Err(other)") == 1


def test_dispatch_routes_to_handlers_without_domain_classifiers() -> None:
    graph_dispatch = ROOT / "crates" / "medulla-server-graph" / "src" / "dispatch"
    split_pipeline = graph_dispatch / "graph_pipeline.rs"
    # The native WorkItem/DevelopmentLane routes live in the pipeline's
    # declared child modules (e.g. `graph_pipeline/native_routes.rs`).
    dispatch = split_pipeline.read_text() + "\n".join(
        path.read_text()
        for path in sorted((graph_dispatch / "graph_pipeline").glob("*.rs"))
    )
    routing_sources = dispatch
    mutation_batch_paths = [
        ROOT / "crates" / "medulla-server-mutation" / "src" / "mutation_batch.rs"
    ]
    mutation_batch_paths.extend(
        sorted(
            (
                ROOT / "crates" / "medulla-server-mutation" / "src" / "mutation_batch"
            ).rglob("*.rs")
        )
    )
    mutation_batch = "\n".join(path.read_text() for path in mutation_batch_paths)
    handler_modules = (HANDLERS / "lib.rs").read_text()

    assert "medulla_h_jobs::work_item::try_handle" in dispatch
    assert "medulla_h_jobs::development_lane::try_handle" in dispatch
    assert "is_work_item_mutation_method(&method)" not in dispatch
    assert "is_development_lane_method" not in dispatch
    assert "dispatch_op_workitem_mutation" not in routing_sources
    assert "dispatch_op_development_lane" not in routing_sources
    assert "is_development_lane_method" not in mutation_batch
    assert "pub mod work_item;" in handler_modules
    assert "pub mod development_lane;" in handler_modules


def test_deleted_lane_classifier_is_absent_from_authority_evidence() -> None:
    evidence_paths = (
        ROOT / "crates" / "medulla-raft" / "src" / "raft" / "mod.rs",
        ROOT / "crates" / "medulla-server-auth" / "src" / "access.rs",
        ROOT / "crates" / "medulla-server-mutation" / "src" / "mutation.rs",
        ROOT
        / "crates"
        / "medulla-capabilities"
        / "tests"
        / "consistency"
        / "divergence.rs",
    )
    for path in evidence_paths:
        assert "is_development_lane_method" not in path.read_text(), path
