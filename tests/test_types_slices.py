"""The medulla-types split: every module has one owning crate (MEDULLA-SDLC-R021.2)."""

from __future__ import annotations

from collections import defaultdict
from pathlib import Path

import pytest

pytestmark = [pytest.mark.no_engine, pytest.mark.spec("MEDULLA-SDLC-R021.2")]
ROOT = Path(__file__).resolve().parents[1]
CRATES = ROOT / "crates"

STAY_SET = frozenset(
    {
        "contract",
        "msgpack",
        "types",
        "keyset_page",
        "tenant_cursor",
        "runtime_limit",
        "lease_vocab",
        "concept_id",
        "evidence_class",
        "modality",
        "durable_unit",
        "durability_class",
        "delta",
        "transition_refused",
        "test_support",
    }
)
REMAINING = (
    "acl",
    "agent_bus",
    "architecture_view",
    "asset_usage",
    "atomic_primitive",
    "attached_source_dialect",
    "attached_source_governance",
    "belief_revision",
    "benchmark_claim",
    "branch",
    "claims",
    "code_health",
    "consolidation_isolation",
    "context_facts",
    "correction",
    "cross_store_scope",
    "data_prep_declaration",
    "data_product",
    "dialect_conformance",
    "document_conformance",
    "embedding_promotion",
    "engine_capabilities",
    "entity_link_proposal",
    "external_schema",
    "extracted_fact",
    "failure_patterns",
    "fast_path",
    "finance_event_envelope",
    "finance_research",
    "finance_wallet_trust",
    "glossary",
    "gramps_baseline_digest",
    "gramps_cutover_gate",
    "gramps_gap_fix",
    "gramps_pilot_phase",
    "graph_change_feed",
    "health_incident",
    "hot_store_engine",
    "immich_pilot_phase",
    "incident",
    "ingest_capabilities",
    "ingest_drain",
    "ingest_readback",
    "ingest_rules",
    "ingestion_wire",
    "knowledge_atom",
    "knowledge_stream",
    "loops",
    "meeting_transcript",
    "memory_sync",
    "metadata_acceleration",
    "mirror_sink",
    "mysql_mariadb_connection",
    "ontology_diff",
    "ontology_edit",
    "ontology_inspection",
    "ontology_proposal",
    "pg_diff_baseline",
    "pg_operational_surface",
    "pgrx_spike",
    "pgrx_spike_review",
    "pipeline_batch",
    "pitr_schedule",
    "profile",
    "quality",
    "rdf_report",
    "replay_conformance",
    "restore_drill",
    "retention_cleanup",
    "retention_policy",
    "rule_correction",
    "run_observation",
    "scheduler_config",
    "schema_context",
    "schema_inference",
    "search_source",
    "series_expr",
    "shared_db_platform",
    "skill_plan",
    "solve_scalar",
    "source_changeset",
    "source_governance",
    "source_symbol_edit",
    "source_toc",
    "statechart",
    "step_verdict",
    "stewardship",
    "task_queue",
    "usage",
    "virtual_graph_view",
    "warehouse_conformance",
    "warehouse_federation",
    "web_source",
    "wire",
    "wire_datascience",
    "wire_finance",
    "wire_mining",
    "wire_pipeline",
    "wire_query_attribution",
    "wire_query_core",
    "wire_query_kinds",
    "wire_query_modalities",
    "wire_query_samples",
    "wire_query_uql",
    "wire_query_uql_modal",
    "wire_query_uql_pred",
    "wire_streaming",
    "workload_class",
    "write_conflict",
)
SLICES = (
    "medulla-query-types",
    "medulla-identity-types",
    "medulla-mutation-types",
    "medulla-work-types",
    "medulla-decision-types",
    "medulla-catalog-types",
    "medulla-source-types",
    "medulla-finance-types",
    "medulla-ingestion-types",
)


def _modules(crate: str) -> set[str]:
    """Top-level module names of a crate; a slice not yet created has none."""
    src = CRATES / crate / "src"
    if not src.is_dir():
        return set()
    names = {path.stem for path in src.glob("*.rs") if path.stem != "lib"}
    return names | {path.name for path in src.iterdir() if path.is_dir()}


@pytest.mark.xfail(
    strict=True,
    reason="medulla-types still holds: " + ", ".join(REMAINING),
)
def test_module_set_of_medulla_types_is_the_stay_set() -> None:
    assert _modules("medulla-types") == STAY_SET


def test_each_moved_module_lives_in_exactly_one_slice() -> None:
    owners: dict[str, list[str]] = defaultdict(list)
    for crate in ("medulla-types", *SLICES):
        for module in _modules(crate):
            owners[module].append(crate)
    shared = {module: crates for module, crates in owners.items() if len(crates) > 1}
    assert shared == {}


QUERY_MODULES = (
    "asr_wire",
    "causal_wire",
    "derived_fact",
    "document_ops",
    "graphlearn_nary",
    "nl_query",
    "plan_cache_key",
    "quantum",
    "sensitive_detect",
    "telemetry_derive",
    "topology",
    "viz",
)


@pytest.mark.spec("MEDULLA-SDLC-R021.3")
def test_query_modules_live_only_in_the_query_slice() -> None:
    assert _modules("medulla-query-types") >= set(QUERY_MODULES)
    for crate in ("medulla-types", *SLICES[1:]):
        assert _modules(crate) & set(QUERY_MODULES) == set(), crate
