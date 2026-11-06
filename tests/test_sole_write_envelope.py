"""MEDULLA-DURABLE-KERNEL-R043.102: no direct table write outside the txn crates."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]


def _gate():
    spec = importlib.util.spec_from_file_location(
        "sole_write_envelope", ROOT / "scripts" / "check_sole_write_envelope.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


DIRECT = """
fn seed(db: &redb::Database) {
    let write = db.begin_write().unwrap();
    // write.open_table(COMMENTED) is not a call
    write.open_table(TABLE).unwrap();
}
"""


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R043.102")
def test_direct_write_is_reported_and_allowlist_needs_reason(tmp_path: Path) -> None:
    gate = _gate()
    crate = tmp_path / "crates" / "medulla-demo" / "src"
    crate.mkdir(parents=True)
    (crate / "lib.rs").write_text(DIRECT, encoding="utf-8")
    exempt = tmp_path / "crates" / "medulla-transaction" / "src"
    exempt.mkdir(parents=True)
    (exempt / "lib.rs").write_text(DIRECT, encoding="utf-8")
    reader = tmp_path / "src"
    reader.mkdir()
    (reader / "read.rs").write_text(
        "fn r(db: &redb::Database) {\n"
        "    let read = db.begin_read().unwrap();\n"
        "    read.open_table(T);\n"
        "}\n",
        encoding="utf-8",
    )

    errors = gate.scan(tmp_path, "")
    code = "SOLE_ENVELOPE_VIOLATION"
    assert errors == [
        f"{code} crates/medulla-demo/src/lib.rs:5: direct redb table write"
    ]

    assert gate.scan(tmp_path, "crates/medulla-demo/src/lib.rs seeds a fixture\n") == []

    no_reason = gate.scan(tmp_path, "crates/medulla-demo/src/lib.rs\n")
    assert any("has no reason" in error for error in no_reason)
    assert any("lib.rs:5" in error for error in no_reason)

    stale = gate.scan(
        tmp_path,
        "crates/medulla-demo/src/lib.rs seeds a fixture\nsrc/read.rs not a writer\n",
    )
    assert stale == [
        f"{code} scripts/sole_write_envelope.allow: src/read.rs "
        "no longer writes a table directly; remove it"
    ]

    assert gate.scan(ROOT, (ROOT / gate.ALLOW).read_text(encoding="utf-8")) == []
