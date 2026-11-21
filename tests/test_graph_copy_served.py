"""Served proof for MEDULLA-OBJECT-STORAGE-BACKEND-R003.2: graph copy to an object root.

`medulla-server store-upgrade graph-copy` copies one graph of a stopped engine's data
directory to an object root and publishes it only after the destination digest matches the
source. A copy into a graph that already holds objects is refused and writes nothing. The
digest-mismatch case, where one staged batch is corrupted and the destination is left
empty, is proved by the store tests in `graph_copy.rs`, which can inject the corruption
between the stage and the verify pass. The binary needs the `graph-copy` feature.
"""

import json
import os
import subprocess
from pathlib import Path

import pytest

from conftest import find_server_binary

pytestmark = [
    pytest.mark.no_engine,
    pytest.mark.spec("MEDULLA-OBJECT-STORAGE-BACKEND-R003.2"),
]


def _run(binary: str, *args: str, data_dir: Path) -> tuple[int, dict]:
    result = subprocess.run(
        [binary, "store-upgrade", "graph-copy", str(data_dir), *args],
        capture_output=True,
        text=True,
        env={**os.environ, "GRAPH_SERVICE_PERSIST_DIR": str(data_dir)},
        timeout=120,
    )
    lines = [line for line in result.stdout.splitlines() if line.strip()]
    summary = json.loads(lines[-1]) if lines else {}
    return result.returncode, summary


def _objects(root: Path) -> list[Path]:
    return sorted(path for path in root.rglob("*") if path.is_file())


def test_digest_mismatch_leaves_destination_empty(tmp_path):
    binary = find_server_binary()
    if binary is None:
        pytest.skip("no medulla-server binary")
    data_dir = tmp_path / "data"
    data_dir.mkdir()
    destination = tmp_path / "object-root"

    # An unconfirmed copy opens nothing and writes nothing.
    code, summary = _run(binary, "g", "--to", str(destination), data_dir=data_dir)
    assert code == 1
    assert summary["outcome"] == "failed"
    assert not destination.exists() or _objects(destination) == []

    # A destination that already holds the graph is refused and left as it was.
    graph_dir = destination / "root" / "default" / "g"
    graph_dir.mkdir(parents=True)
    (graph_dir / "HEAD").write_text("{}")
    before = _objects(destination)
    code, summary = _run(
        binary, "g", "--to", str(destination), "--confirm", data_dir=data_dir
    )
    assert code == 1
    assert _objects(destination) == before
