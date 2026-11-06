"""Case table schema and loader (MEDULLA-CQ-CONN-R067.7)."""

from __future__ import annotations

import copy
import json
from pathlib import Path
from typing import Any

import pytest
from cases import CASE_INVALID, CaseInvalid, load_cases, load_file

pytestmark = pytest.mark.no_engine

GOOD: dict[str, Any] = {
    "connector": "alpha",
    "rollup": "MEDULLA-CQ-CONN-R001",
    "vocabulary_module": "data_platform-v1.ttl",
    "terms": ["dp:Collection"],
    "questions": [
        {
            "id": "alpha.q1",
            "kind": "single",
            "uql": "MATCH (c:Collection) RETURN c",
            "sources": ["alpha"],
            "optional_sources": ["beta"],
            "columns": ["c"],
            "min_rows": 1,
            "recordings": ["alpha/single.json"],
        }
    ],
}


def _mutate(path: tuple, value: object) -> dict:
    doc: Any = copy.deepcopy(GOOD)
    node: Any = doc
    for key in path[:-1]:
        node = node[key]
    node[path[-1]] = value
    return doc


INVALID = [
    ("unknown rollup", _mutate(("rollup",), "MEDULLA-CQ-CONN-R999")),
    ("no recording for source", _mutate(("questions", 0, "recordings"), [])),
    (
        "recording file missing",
        _mutate(("questions", 0, "recordings"), ["alpha/x.json"]),
    ),
    (
        "foreign connector in text",
        _mutate(("questions", 0, "uql"), "MATCH gamma RETURN 1"),
    ),
    ("bad kind", _mutate(("questions", 0, "kind"), "both")),
    ("missing key", {k: v for k, v in GOOD.items() if k != "terms"}),
]


def _write(root: Path, doc: dict, name: str = "alpha") -> Path:
    (root / "cases").mkdir(exist_ok=True)
    rec = root / "recordings"
    for source in ("alpha", "beta", "gamma"):
        (rec / source).mkdir(parents=True, exist_ok=True)
    (rec / "alpha" / "single.json").write_text("{}")
    path = root / "cases" / f"{name}.json"
    path.write_text(json.dumps(doc))
    return path


@pytest.mark.spec("MEDULLA-CQ-CONN-R067.7")
def test_every_rollup_has_a_valid_case_file(tmp_path: Path) -> None:
    """R067.7: shipped files load; each invalid variant is refused."""
    assert isinstance(load_cases(), list)
    ok = _write(tmp_path, GOOD)
    cases = load_file(ok, tmp_path / "cases", tmp_path / "recordings")
    assert [c.id for c in cases] == ["alpha.q1"]
    for label, doc in INVALID:
        path = _write(tmp_path, doc)
        with pytest.raises(CaseInvalid) as err:
            load_file(path, tmp_path / "cases", tmp_path / "recordings")
        assert err.value.code == CASE_INVALID, label
