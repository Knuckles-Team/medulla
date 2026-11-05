"""MEDULLA-CONTRACT-R055: the decision reachability gate."""

from __future__ import annotations

from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine


def _write(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _tree(root: Path) -> None:
    _write(
        root,
        "crates/medulla-decision-types/src/lib.rs",
        "pub struct Wired;\npub fn orphan_item() {}\n",
    )
    _write(root, "crates/medulla-decision-policy/src/solve/mod.rs", "pub fn x() {}\n")
    _write(root, "crates/medulla-compute/src/assemble/agent.rs", "\n")
    _write(
        root,
        "crates/medulla-protocol-types/src/protocol/method/decision.rs",
        "pub enum Method { RetrievalRound }\n",
    )
    _write(
        root,
        "crates/medulla-h-decide/src/lib.rs",
        "use medulla_decision_types::Wired;\n"
        "fn route() { let _ = RetrievalRound; x(); }\n",
    )
    # Test code never makes an item reachable.
    _write(root, "crates/medulla-h-decide/src/tests.rs", "fn t() { orphan_item(); }\n")
    _write(
        root,
        "crates/medulla-h-decide/src/inline.rs",
        "#[cfg(test)]\nmod tests {\n    fn t() { orphan_item(); }\n}\n",
    )


@pytest.mark.spec("MEDULLA-CONTRACT-R055")
def test_an_orphan_public_item_fails_the_gate(load_script, tmp_path, capsys) -> None:
    gate = load_script("check_decision_reachability")
    _tree(tmp_path)
    orphans = gate.find_orphans(tmp_path)
    assert [(o.name, o.path) for o in orphans] == [
        ("orphan_item", "crates/medulla-decision-types/src/lib.rs")
    ]
    assert gate.main(["--root", str(tmp_path)]) == 1
    assert "ORPHAN_DECISION_ITEM" in capsys.readouterr().err

    # A named method nobody routes is an orphan as well.
    _write(
        tmp_path,
        "crates/medulla-h-decide/src/lib.rs",
        "use medulla_decision_types::Wired;\nfn route() { x(); orphan_item(); }\n",
    )
    names = [o.name for o in gate.find_orphans(tmp_path)]
    assert names == ["RetrievalRound"]

    # Wiring every item turns the gate green; a missing tree fails closed.
    _write(
        tmp_path,
        "crates/medulla-h-decide/src/lib.rs",
        "use medulla_decision_types::Wired;\n"
        "fn route() { x(); orphan_item(); let _ = RetrievalRound; }\n",
    )
    assert gate.main(["--root", str(tmp_path)]) == 0
    (tmp_path / "crates/medulla-compute/src/assemble/agent.rs").unlink()
    (tmp_path / "crates/medulla-compute/src/assemble").rmdir()
    assert gate.main(["--root", str(tmp_path)]) == 2
