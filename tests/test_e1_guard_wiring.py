"""Discriminating tests for the relocated/generated E1 envelope guards."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]


def _load_gate(filename: str, module_name: str):
    path = ROOT / "scripts" / filename
    spec = importlib.util.spec_from_file_location(module_name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_universal_read_protocol_child_variant_is_composed_and_required() -> None:
    module = _load_gate("check_universal_read_rls.py", "e1_universal_protocol_family")
    protocol = module.protocol_source()
    policies = module.capability_inventory()
    assert "KnowledgeStream {" in protocol
    module.require_protocol_inventory(protocol, policies)

    omitted = protocol.replace("KnowledgeStream {", "KnowledgeStreamRemoved {", 1)
    with pytest.raises(SystemExit, match="protocol/policy inventories differ"):
        module.require_protocol_inventory(omitted, policies)


def test_modality_retry_binds_caller_tenant_and_physical_graph_key() -> None:
    source = (
        ROOT / "crates/medulla-server-graph/src/dispatch/graph_pipeline/modality.rs"
    ).read_text(encoding="utf-8")
    binding_start = source.index("pub fn modality_receipt_binding_matches(")
    binding_end = source.index("\n}\n", binding_start)
    binding = source[binding_start:binding_end]

    # These identities deliberately differ on both axes: the caller tenant is
    # preserved in the envelope while the durable scope is rebound to the shard
    # tenant and the escaped physical key for logical graph `docs/a#b`.
    caller_tenant = "tenant-a"
    durable_scope_tenant = "__shard__"
    logical_graph = "docs/a#b"
    graph_fname = "docs~2fa~23b"
    assert caller_tenant != durable_scope_tenant
    assert logical_graph != graph_fname

    assert "record.committing_tenant()" in binding
    assert "record.batch.identity.tenant()" not in binding
    assert "== Some(graph_fname)" in binding
    assert "== Some(ctx.graph_name)" not in binding
    assert (
        "modality_receipt_binding_matches(ctx, &record, batch_id, graph_fname)"
        in source
    )


def test_universal_read_cache_key_follows_query_children() -> None:
    module = _load_gate("check_universal_read_rls.py", "e1_universal_query_family")
    query = module.read_module_tree(
        "crates/medulla-h-query/src/query.rs", root_dir=ROOT
    )
    rdf = module.rdf_handler_source()
    dispatch = module.served_dispatch_source()
    module.require_query_result_cache_rls(query, rdf, dispatch)

    broken = query.replace('format!("rls:{caller}:{kind}")', 'format!("{kind}")', 1)
    with pytest.raises(SystemExit, match="result-cache actor key"):
        module.require_query_result_cache_rls(broken, rdf, dispatch)


def test_mint_guard_rejects_a_call_site_without_verified_mac_binding(tmp_path) -> None:
    module = _load_gate("check_mint_lease_call_sites.py", "e1_mint_guard")
    sites = module.find_call_sites(module.ROOT)
    production = [site for site in sites if not module._is_test_only(site[0])]
    only_path, only_line = module.require_sole_owning_call_site(module.ROOT, production)
    module.require_self_constructed_authorization(module.ROOT, only_path, only_line)
    router = module.ROOT / only_path

    target = tmp_path / only_path
    target.parent.mkdir(parents=True)
    target.write_text(
        router.read_text(encoding="utf-8").replace(
            "MintAuthorization::new", "MintAuthorization::construct", 1
        ),
        encoding="utf-8",
    )
    with pytest.raises(
        SystemExit, match="does not construct its own MintAuthorization"
    ):
        module.require_self_constructed_authorization(tmp_path, only_path, only_line)


def test_universal_read_guard_rejects_an_unprojected_sql_snapshot() -> None:
    module = _load_gate("check_universal_read_rls.py", "e1_universal_rls_guard")
    wire = module.read("crates/medulla-wire-core/src/wire/mod.rs")
    module._check_sql_wire_read_contract(wire)

    broken = wire.replace(
        "self.filter_view_for_verified_actor(&mut snap).await?",
        "self.filter_view_for_verified_actor_removed(&mut snap).await?",
        1,
    )
    with pytest.raises(SystemExit, match="SQL-wire snapshot bypasses"):
        module._check_sql_wire_read_contract(broken)


def test_persisted_blob_guard_rejects_split_refcount_commit() -> None:
    module = _load_gate(
        "check_persisted_mutation_contract.py", "e1_persisted_mutation_guard"
    )
    sources = module.mutation_inventory_sources()
    module._check_blob_result_contract(
        sources["blob_store"], sources["blob_shared"], sources["blob_store_tests"]
    )

    # RF-RULING-004 moved the CAS/refcount tables into the storage kernel's
    # shared blob handle (`medulla-storage`'s `owner/blob_shared.rs`), which the
    # gate now reads directly from disk regardless of this `blob_store`
    # argument -- so mutating `CAS_REFCOUNT` here (a token the carrier no
    # longer references locally) is a no-op against the gate. Mutate a token
    # the gate DOES still check within `blob_store`'s own call graph instead.
    broken = sources["blob_store"].replace(
        "insert_chunk_if_absent", "chunk_insert_when_missing"
    )
    with pytest.raises(SystemExit, match="atomically bind CAS"):
        module._check_blob_result_contract(
            sources["blob_store"], broken, sources["blob_store_tests"]
        )
