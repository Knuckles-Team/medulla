"""MEDULLA-CONTRACT-R055: the reachability gate refuses an orphan public item."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_SPEC = importlib.util.spec_from_file_location(
    "check_decision_reachability", ROOT / "scripts" / "check_decision_reachability.py"
)
assert _SPEC is not None and _SPEC.loader is not None
gate = importlib.util.module_from_spec(_SPEC)
sys.modules["check_decision_reachability"] = gate
_SPEC.loader.exec_module(gate)


def _tree(tmp_path: Path, caller: str) -> Path:
    types_src = tmp_path / "crates/medulla-decision-types/src"
    types_src.mkdir(parents=True)
    (types_src / "lib.rs").write_text(
        "pub fn orphan_item() {}\npub fn wired_item() {}\n", encoding="utf-8"
    )
    for scan in gate.SCAN_ROOTS[1:]:
        (tmp_path / scan).mkdir(parents=True, exist_ok=True)
    server = tmp_path / "crates/medulla-h-decide/src"
    server.mkdir(parents=True)
    (server / "lib.rs").write_text(caller, encoding="utf-8")
    return tmp_path


# spec: MEDULLA-CONTRACT-R055
def test_an_orphan_public_item_fails_the_gate(tmp_path: Path) -> None:
    # spec: MEDULLA-CONTRACT-R055
    root = _tree(
        tmp_path,
        "fn run() { wired_item(); }\n#[cfg(test)]\n"
        "mod tests { fn t() { orphan_item(); } }\n",
    )
    orphans, declared, files = gate.find_orphans(root)
    assert declared == 2 and files == 2
    assert [item.name for item in orphans] == ["orphan_item"]


def test_a_production_caller_clears_every_item(tmp_path: Path) -> None:
    # spec: MEDULLA-CONTRACT-R055
    root = _tree(tmp_path, "fn run() { wired_item(); orphan_item(); }\n")
    orphans, _, _ = gate.find_orphans(root)
    assert orphans == []
