"""SourceRecord.column_profiles digest parity with Rust (R079.4)."""

from __future__ import annotations

import copy
from typing import Any

import pytest

from medulla.generated.source_ingestion import SourceIngestionRequest

pytestmark = pytest.mark.no_engine

# Pinned against the Rust test `column_profiles_digest_vectors_match_the_python_client`
# in crates/medulla-source-types.
DIGEST_WITHOUT_PROFILES = (
    "bfa1c9d59ba6faed1a195bcd61827444b6d2c6bd962551ab3157412384c915f3"
)
DIGEST_WITH_PROFILE = "8a56a8e628f6ead40e4bcf96792891ef0974f42e4b6d8ca8d1566fa30ed2ebcb"

BATCH: dict[str, Any] = {
    "connector": "c",
    "mode": "full",
    "strict_schema": True,
    "records": [
        {
            "stream": "s",
            "record_id": "r1",
            "mapping_reference": "m",
            "payload": {"b": 1, "a": 2},
            "provenance": {
                "connector": "c",
                "adapter_kind": "sql",
                "server": "srv",
                "tool": "t",
                "tool_schema_sha256": "0" * 64,
                "source_uri": "u",
            },
        }
    ],
    "provider_checkpoint": {"stream": "s", "position": {"k": 1}},
}

PROFILE: dict[str, Any] = {
    "a": {
        # Deliberately not in Rust declaration order: the model must reorder.
        "sample_count": 10,
        "distinct_ratio_q32": 2147483648,
        "is_foreign_key": False,
        "sql_type": "numeric",
    }
}


def _digest(profiles: object = ...) -> str:
    batch = copy.deepcopy(BATCH)
    if profiles is not ...:
        batch["records"][0]["column_profiles"] = profiles
    return SourceIngestionRequest.model_validate(batch).canonical_digest()


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R079.4")
def test_record_without_profiles_keeps_its_digest():
    assert _digest() == DIGEST_WITHOUT_PROFILES


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R079.4")
def test_none_and_empty_profiles_digest_identically_to_absent():
    assert _digest(None) == DIGEST_WITHOUT_PROFILES
    assert _digest({}) == DIGEST_WITHOUT_PROFILES


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R079.4")
def test_record_with_a_profile_digests_to_the_rust_vector():
    assert _digest(PROFILE) == DIGEST_WITH_PROFILE
    assert _digest(PROFILE) != DIGEST_WITHOUT_PROFILES


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R079.4")
def test_profile_serialises_in_rust_declaration_order():
    request = SourceIngestionRequest.model_validate(
        {**BATCH, "records": [{**BATCH["records"][0], "column_profiles": PROFILE}]}
    )
    profile = request.model_dump(mode="json")["records"][0]["column_profiles"]["a"]
    assert list(profile) == [
        "sql_type",
        "is_foreign_key",
        "distinct_ratio_q32",
        "sample_count",
    ]
