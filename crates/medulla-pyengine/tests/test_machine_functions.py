"""Machine identity, lifecycle and hosts-file functions of the compiled engine binding
(MEDULLA-MACHINE-R007).

The test imports the compiled ``medulla.engine`` extension directly and checks every
function against the vectors recorded beside the Rust functions, so the wheel and
the Rust code cannot drift. If no wheel is installed the module is skipped, like
``test_engine_smoke.py``.
"""

from __future__ import annotations

import importlib
import json
from pathlib import Path
from typing import Any

import pytest

_e: Any = None
for _name in ("medulla.engine", "engine"):
    try:
        _m = importlib.import_module(_name)
    except ImportError:
        continue
    if getattr(_m, "__engine__", None) == "medulla-pyengine":
        _e = _m
        break

pytestmark = pytest.mark.skipif(
    _e is None,
    reason=(
        "medulla-pyengine binding wheel not installed "
        "(build with maturin --features python)"
    ),
)

VECTORS = json.loads(
    (
        Path(__file__).resolve().parents[2]
        / "medulla-source-types"
        / "tests"
        / "vectors"
        / "machine_iri.json"
    ).read_text(encoding="utf-8")
)

FACTS_UP = json.dumps({"reachable": True, "ready": True})


def test_every_vector_and_refusal_matches_the_rust_functions() -> None:
    for row in VECTORS["iri_vectors"]:
        keys = json.dumps([{"kind": row["kind"], "value": row["raw"]}])
        out = json.loads(_e.machine_iri(row["tenant"], keys))
        assert out["iri"] == row["iri"]
        assert out["tier"] == row["kind"]
        normalized = json.loads(_e.machine_normalize_key(row["kind"], row["raw"]))
        assert normalized == {"kind": row["kind"], "value": row["normalized"]}
    for row in VECTORS["preference_vectors"]:
        chosen = json.loads(_e.machine_iri("tenant-a", json.dumps(row["keys"])))
        assert {"kind": chosen["tier"], "value": chosen["key"].split(":", 1)[1]} == row[
            "selects"
        ]
    for row in VECTORS["anchor_vectors"]:
        ref = json.loads(_e.machine_key_ref(row["kind"], row["value"]))
        assert ref["iri"] == row["iri"]

    with pytest.raises(ValueError, match="^uuid:empty$"):
        _e.machine_normalize_key("uuid", "")
    with pytest.raises(ValueError, match="^serial:placeholder$"):
        _e.machine_normalize_key("serial", "none")
    with pytest.raises(ValueError, match="^bmc:malformed$"):
        _e.machine_normalize_key("bmc", "aa:bb")
    with pytest.raises(ValueError, match="^rack:unknown_kind$"):
        _e.machine_normalize_key("rack", "x")
    nil = json.dumps(
        [{"kind": "uuid", "value": "00000000-0000-0000-0000-000000000000"}]
    )
    with pytest.raises(ValueError, match="^no_valid_key$"):
        _e.machine_iri("t", nil)


def test_lifecycle_functions_follow_the_edge_table() -> None:
    assert json.loads(_e.machine_lifecycle_derive("commissioned", FACTS_UP)) == {
        "event": "Ready"
    }
    assert json.loads(_e.machine_lifecycle_derive("active", FACTS_UP)) == {
        "event": None
    }
    assert json.loads(
        _e.machine_lifecycle_apply("commissioned", "Ready", FACTS_UP)
    ) == {
        "state": "active",
        "edge_id": "H01",
    }
    with pytest.raises(
        ValueError,
        match=r"TRANSITION_REFUSED: .*'Retire' from 'active' \(illegal_transition\)",
    ):
        _e.machine_lifecycle_apply("active", "Retire", "{}")
    with pytest.raises(ValueError, match=r"from 'retired' \(terminal_state\)"):
        _e.machine_lifecycle_apply("retired", "Ready", "{}")
    with pytest.raises(ValueError, match=r"from 'commissioned' \(guard_failed\)"):
        _e.machine_lifecycle_apply("commissioned", "Ready", "{}")
    with pytest.raises(ValueError, match=r"\(unknown_event\)"):
        _e.machine_lifecycle_apply("commissioned", "Wake", "{}")


HOSTS = """schema: hosts/v1
hosts:
  - name: beta
    addresses: [BETA.Example]
    identity:
      system_serial: SN-0001
  - name: alpha
    addresses: [alpha.example]
"""


def test_hosts_file_and_record_functions_are_canonical() -> None:
    text = _e.normalize_hosts_file(HOSTS)
    parsed = json.loads(text)
    assert [h["name"] for h in parsed["hosts"]] == ["alpha", "beta"]
    assert text == json.dumps(parsed, sort_keys=True, separators=(",", ":"))
    assert _e.canonical_digest(text) == _e.machine_hosts_digest(HOSTS)
    assert _e.canonical_digest("").startswith("sha256:e3b0c44298fc1c14")

    with pytest.raises(ValueError) as refused:
        _e.normalize_hosts_file("schema: hosts/v2\nhosts: []\n")
    assert json.loads(str(refused.value)) == {
        "code": "HOSTS_FILE_INVALID",
        "pointer": "/schema",
        "reason": "schema_version",
    }

    beta = next(h for h in parsed["hosts"] if h["name"] == "beta")
    entity = json.loads(
        _e.machine_host_entity("tenant-a", json.dumps(beta), "commissioned")
    )
    assert entity["node_type"] == "Machine"
    assert entity["properties"]["identityTier"] == "serial"
    records = json.dumps(
        {"schema": "inventory-records/v1", "entities": [entity], "relationships": []}
    )
    assert (
        json.loads(_e.machine_check_shapes(records))["entities"][0]["id"]
        == entity["id"]
    )
    with pytest.raises(ValueError, match="machineLifecycleState"):
        _e.machine_check_shapes(records.replace("commissioned", "asleep"))

    ids = json.loads(
        _e.machine_component_ids(
            entity["id"], "bond", json.dumps([{"bondName": "bond0"}, {}])
        )
    )
    assert ids[0].startswith("urn:machine-part:") and ids[1] is None
    assert _e.machine_probe_id(entity["id"], "system", "sha256:00").startswith(
        "urn:machine-probe:"
    )
    assert _e.machine_transition_id(entity["id"], "H01", "sha256:00").startswith(
        "urn:machine-transition:"
    )
