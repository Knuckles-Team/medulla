"""The medulla-types split: every module has one owning crate (MEDULLA-SDLC-R021.2)."""

from __future__ import annotations

import re
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
# Modules not in the stay set that no move of MEDULLA-SDLC-R021.3 to R021.6 names.
REMAINING = (
    "archimate_exchange",
    "capability_machine",
    "claim_machine",
    "code_context",
    "code_map",
    "contract_catalog",
    "contradiction_machine",
    "corpus_retrieval",
    "dashboard",
    "deployment_profile",
    "finance_accounting",
    "finance_forecast",
    "gap_machine",
    "gap_topic_machine",
    "geo_task_machine",
    "ingest_lanes",
    "ingest_machine",
    "ingest_materialize",
    "ingest_throughput",
    "numeric_kernel",
    "ontology_base",
    "participant",
    "process_exchange",
    "process_import",
    "profile_grant_machine",
    "projection_read",
    "refusal_code",
    "regime_forecast",
    "reservation_machine",
    "retrieval",
    "retrieval_round",
    "row_change",
    "schedule_machine",
    "semantic_enrichment",
    "shape_report",
    "throughput_machine",
    "unified_row",
    "viewport",
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
    "rdf_report",
    "search_source",
    "series_expr",
    "warehouse_federation",
    "wire",
    "wire_datascience",
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
    "asr_wire",
    "causal_wire",
    "derived_fact",
    "document_ops",
    "graphlearn_nary",
    "nl_query",
    "plan_cache_key",
    "quantum",
    "sensitive_detect",
    "solve_scalar",
    "telemetry_derive",
    "topology",
    "viz",
)


@pytest.mark.spec("MEDULLA-SDLC-R021.3")
def test_query_modules_live_only_in_the_query_slice() -> None:
    assert _modules("medulla-query-types") >= set(QUERY_MODULES)
    for crate in ("medulla-types", *SLICES[1:]):
        assert _modules(crate) & set(QUERY_MODULES) == set(), crate


def _dependencies(crate: str) -> set[str]:
    """Every `medulla-*` crate a slice names in its manifest (any section)."""
    manifest = (CRATES / crate / "Cargo.toml").read_text(encoding="utf-8")
    names = set()
    for line in manifest.splitlines():
        head = line.split("=", 1)[0].strip()
        if head.startswith("medulla-") and "path" in line:
            names.add(head)
    return names


@pytest.mark.spec("MEDULLA-SDLC-R021.4")
def test_acl_lives_in_the_identity_slice() -> None:
    assert "acl" in _modules("medulla-identity-types")
    for crate in ("medulla-types", *SLICES):
        if crate != "medulla-identity-types":
            assert "acl" not in _modules(crate), crate
    # The wire family reads the grant vocabulary: the dependency runs query -> identity
    # and never back; identity may not name the query slice.
    assert "medulla-identity-types" in _dependencies("medulla-query-types")
    assert "medulla-query-types" not in _dependencies("medulla-identity-types")
    old_acl_path = "medulla_types::acl"
    stale = [
        str(path.relative_to(ROOT))
        for path in CRATES.rglob("*.rs")
        if "target" not in path.parts
        and old_acl_path in path.read_text(encoding="utf-8", errors="ignore")
    ]
    assert stale == []


DOMAIN_SLICES = {
    "medulla-mutation-types": (
        "benchmark_claim",
        "branch",
        "cross_store_scope",
        "fast_path",
        "graph_change_feed",
        "harness_metrics",
        "health_incident",
        "hot_store_engine",
        "memory_sync",
        "mirror_sink",
        "pg_operational_surface",
        "pipeline_batch",
        "retention_cleanup",
        "retention_policy",
        "rule_correction",
        "versioning",
        "workload_class",
        "write_conflict",
    ),
    "medulla-work-types": (
        "agent_bus",
        "run_observation",
        "scheduler_config",
        "statechart",
        "task_queue",
    ),
    "medulla-decision-types": (
        "belief_revision",
        "claims",
        "context_facts",
        "correction",
        "failure_patterns",
        "loops",
        "source_toc",
        "usage",
    ),
    "medulla-catalog-types": (
        "architecture_view",
        "asset_usage",
        "attached_source_dialect",
        "attached_source_governance",
        "consolidation_isolation",
        "data_product",
        "dialect_conformance",
        "document_conformance",
        "embedding_promotion",
        "engine_capabilities",
        "entity_link_proposal",
        "external_schema",
        "glossary",
        "gramps_baseline_digest",
        "gramps_cutover_gate",
        "gramps_gap_fix",
        "gramps_pilot_phase",
        "immich_pilot_phase",
        "incident",
        "metadata_acceleration",
        "mysql_mariadb_connection",
        "ontology_diff",
        "ontology_edit",
        "ontology_inspection",
        "pg_diff_baseline",
        "pgrx_spike",
        "pgrx_spike_review",
        "pitr_schedule",
        "profile",
        "quality",
        "replay_conformance",
        "restore_drill",
        "schema_context",
        "schema_inference",
        "shared_db_platform",
        "stewardship",
        "virtual_graph_view",
        "warehouse_conformance",
        "web_source",
    ),
    "medulla-source-types": (
        "meeting_transcript",
        "source_changeset",
        "source_symbol_edit",
    ),
}


@pytest.mark.spec("MEDULLA-SDLC-R021.5")
def test_domain_modules_live_in_their_owning_slice() -> None:
    for owner, modules in DOMAIN_SLICES.items():
        assert _modules(owner) >= set(modules), owner
        for crate in ("medulla-types", *SLICES):
            if crate != owner:
                assert _modules(crate) & set(modules) == set(), crate
    # Hard cutover: no importer still names a moved module through medulla-types.
    moved = "|".join(module for modules in DOMAIN_SLICES.values() for module in modules)
    old_path = re.compile(r"\bmedulla_types::(" + moved + r")\b")
    stale = [
        str(path.relative_to(ROOT))
        for top in ("crates", "src", "tests", "benches")
        for path in (ROOT / top).rglob("*.rs")
        if "target" not in path.parts
        and old_path.search(path.read_text(encoding="utf-8", errors="ignore"))
    ]
    assert stale == []


FINANCE_MODULES = (
    "finance_event_envelope",
    "finance_research",
    "finance_wallet_trust",
    "wire_finance",
)
INGESTION_MODULES = (
    "code_health",
    "data_prep_declaration",
    "extracted_fact",
    "ingest_capabilities",
    "ingest_drain",
    "ingest_readback",
    "ingest_rules",
    "ingestion_wire",
    "knowledge_atom",
    "knowledge_stream",
    "ontology_proposal",
    "skill_plan",
    "source_governance",
)


@pytest.mark.spec("MEDULLA-SDLC-R021.6")
def test_finance_and_ingestion_modules_live_in_their_slices() -> None:
    owners = {
        "medulla-finance-types": FINANCE_MODULES,
        "medulla-ingestion-types": INGESTION_MODULES,
    }
    for owner, modules in owners.items():
        assert _modules(owner) == set(modules), owner
        # Both slices are contract types over medulla-types alone.
        assert _dependencies(owner) == {"medulla-types"}, owner
    moved = "|".join((*FINANCE_MODULES, *INGESTION_MODULES))
    old_path = re.compile(r"\bmedulla_types::(" + moved + r")\b")
    stale = [
        str(path.relative_to(ROOT))
        for top in ("crates", "src", "tests", "benches")
        for path in (ROOT / top).rglob("*.rs")
        if "target" not in path.parts
        and old_path.search(path.read_text(encoding="utf-8", errors="ignore"))
    ]
    assert stale == []
