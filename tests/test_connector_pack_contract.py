"""The hand-written Python pack client against the Rust round-trip vectors."""

from __future__ import annotations

import copy
import json
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from medulla.connector_pack import _annotations_digest, _entry_digest, pack_digest
from medulla.generated.connector_pack import ConnectorPackIndex

pytestmark = pytest.mark.no_engine

_VECTORS = (
    Path(__file__).resolve().parents[1]
    / "crates"
    / "medulla-types"
    / "tests"
    / "fixtures"
    / "connector_pack_digest_vectors.json"
)


def _samples() -> list[dict[str, Any]]:
    samples = json.loads(_VECTORS.read_text(encoding="utf-8"))["samples"]
    assert samples, "the Rust vector file lost every sample"
    return samples


def _digest(index: ConnectorPackIndex) -> str:
    return pack_digest(index.connector, index.catalog, index.server, index.entries)


@pytest.mark.spec("MEDULLA-TYPED-PACKS-R001.5")
def test_python_client_matches_pack_vectors() -> None:
    for sample in _samples():
        wire = sample["index"]
        index = ConnectorPackIndex.model_validate(wire)
        # Accept and emit the Rust wire shape unchanged.
        emitted = index.model_dump(mode="json", exclude_none=True)
        assert ConnectorPackIndex.model_validate(emitted) == index
        assert _digest(index) == sample["pack_digest"]
        for entry in [index.server, *index.entries]:
            expected = sample["entries"][entry.uri]
            assert _entry_digest(entry).hex() == expected["entry_digest"]
            annotations = entry.annotations
            assert annotations is not None
            assert (
                _annotations_digest(annotations).hex() == expected["annotations_digest"]
            )

        missing = copy.deepcopy(wire)
        del missing["entries"][0]["name"]
        with pytest.raises(ValidationError):
            ConnectorPackIndex.model_validate(missing)

        extra = copy.deepcopy(wire)
        extra["entries"][0]["unexpected"] = True
        with pytest.raises(ValidationError):
            ConnectorPackIndex.model_validate(extra)

        changed = copy.deepcopy(wire)
        changed["entries"][0]["name"] = changed["entries"][0]["name"] + "-changed"
        assert (
            _digest(ConnectorPackIndex.model_validate(changed)) != sample["pack_digest"]
        )
