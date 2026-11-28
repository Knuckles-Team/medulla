"""Every new public function of the machine module has a production caller (MEDULLA-
MACHINE-R008).

Static check; no engine. It reads the Rust sources, drops each ``#[cfg(test)]``
module, and requires every public function of ``crates/medulla-source-
types/src/machine/`` and ``crates/medulla-pyengine/src/machine.rs`` to be called
from non-test code other than its own definition. It also requires the ontology and
the engine registrations that the row names.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
MACHINE_FILES = sorted(
    (ROOT / "crates" / "medulla-source-types" / "src" / "machine").glob("*.rs")
) + [ROOT / "crates" / "medulla-pyengine" / "src" / "machine.rs"]
PUBLIC_FN = re.compile(r"^\s*pub fn (\w+)", re.MULTILINE)
TEST_MODULE = re.compile(r"#\[cfg\(test\)\]\s*(?:pub )?mod \w+ \{")

# Functions the arbora inventory operations call through the engine wheel are
# registered in `crates/medulla-pyengine/src/lib.rs`, which is production code and
# counts as a caller. A method that is a plain accessor on a type (`tag`, `as_str`,
# `from_tag`, ...) is exercised by the functions that use the type, so it is checked
# by name the same way.


def _production_text(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    match = TEST_MODULE.search(text)
    return text[: match.start()] if match else text


def _all_production_sources() -> dict[Path, str]:
    sources: dict[Path, str] = {}
    for crate in (ROOT / "crates").iterdir():
        src = crate / "src"
        if not src.is_dir():
            continue
        for path in src.rglob("*.rs"):
            sources[path] = _production_text(path)
    return sources


@pytest.mark.spec("MEDULLA-MACHINE-R008")
def test_every_new_symbol_has_a_production_caller() -> None:
    assert MACHINE_FILES, "the machine module files are missing"
    sources = _all_production_sources()
    uncalled: list[str] = []
    checked = 0
    for path in MACHINE_FILES:
        production = _production_text(path)
        for name in sorted(set(PUBLIC_FN.findall(production))):
            checked += 1
            call = re.compile(
                rf"(?<![\w])(?:\w+::)*{re.escape(name)}\s*[(:]|\.{re.escape(name)}\s*\(|{re.escape(name)}\s*,|wrap_pyfunction!\({re.escape(name)}\b"
            )
            definition = re.compile(rf"pub fn {re.escape(name)}\b")
            called = False
            for other, text in sources.items():
                stripped = definition.sub("", text)
                if other == path and not call.search(stripped):
                    continue
                if call.search(stripped):
                    called = True
                    break
            if not called:
                uncalled.append(f"{path.relative_to(ROOT)}::{name}")
    assert checked >= 40, f"only {checked} public functions found"
    assert not uncalled, uncalled


@pytest.mark.spec("MEDULLA-MACHINE-R008")
def test_the_vocabulary_and_the_functions_are_registered() -> None:
    schema_sources = (
        ROOT / "crates/medulla-graph-core/src/graph/schema_sources.rs"
    ).read_text(encoding="utf-8")
    assert "machine-v1.ttl" in schema_sources
    assert "machine-v1.shapes.ttl" in schema_sources
    lib = (ROOT / "crates/medulla-pyengine/src/lib.rs").read_text(encoding="utf-8")
    for function in (
        "machine_key_ref",
        "machine_normalize_key",
        "machine_iri",
        "machine_lifecycle_derive",
        "machine_lifecycle_apply",
        "normalize_hosts_file",
        "canonical_digest",
    ):
        assert f"wrap_pyfunction!({function}, m)" in lib, function
    manifest = (ROOT / "crates/medulla-pyengine/Cargo.toml").read_text(encoding="utf-8")
    assert "medulla-source-types" in manifest
