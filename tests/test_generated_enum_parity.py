"""Every generated Python enum lists exactly the members of its Rust enum.

The Python wire models are maintained by hand beside the Rust serde types, and
the wire vectors only exercise the members their samples happen to use. A member
added in Rust and not in Python therefore passed every vector and still failed a
live decode. This gate compares member sets directly; the Rust source is read in
the test, so nothing generated is committed.
"""

from __future__ import annotations

import enum
import importlib
import pkgutil
import re
from pathlib import Path

import pytest

import medulla.generated as generated_pkg
from medulla.generated.connector_pack import PackViolation, PackViolationCode

ROOT = Path(__file__).resolve().parents[1]
pytestmark = [pytest.mark.no_engine]

_ENUM = re.compile(r"((?:[ \t]*(?:#\[[^\n]*\]|///[^\n]*)\n)*)pub enum (\w+)\s*\{")
_RENAME_ALL = re.compile(r'rename_all\s*=\s*"([\w-]+)"')
_VARIANT = re.compile(r"^    ([A-Z]\w*)\s*,", re.M)
# Client-only members that are not engine codes.
CLIENT_ONLY = {("PackViolationCode", "UNKNOWN")}


def _words(variant: str) -> list[str]:
    return re.findall(r"[A-Z][a-z0-9]*", variant)


def _wire(variant: str, style: str) -> str | None:
    words = _words(variant)
    return {
        "SCREAMING_SNAKE_CASE": "_".join(words).upper(),
        "snake_case": "_".join(words).lower(),
        "kebab-case": "-".join(words).lower(),
        "lowercase": "".join(words).lower(),
    }.get(style)


def _rust_enums() -> dict[str, list[set[str]]]:
    """Wire values of each plain unit-variant Rust enum, by enum name."""
    found: dict[str, list[set[str]]] = {}
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        if "target" in path.parts:
            continue
        text = path.read_text(errors="ignore")
        for match in _ENUM.finditer(text):
            attrs, name = match.group(1), match.group(2)
            depth, i = 1, match.end()
            while depth and i < len(text):
                depth += (text[i] == "{") - (text[i] == "}")
                i += 1
            body = text[match.end() : i - 1]
            style = _RENAME_ALL.search(attrs)
            plain = (
                style is not None
                and "tag =" not in attrs
                and "untagged" not in attrs
                and "serde(rename" not in body
                and "(" not in re.sub(r"//[^\n]*|#\[[^\n]*\]", "", body)
                and "{" not in body
            )
            if not plain:
                continue
            assert style is not None
            wired = [_wire(v, style.group(1)) for v in _VARIANT.findall(body)]
            values = {w for w in wired if w is not None}
            if len(values) == len(wired) and values:
                found.setdefault(name, []).append(values)
    return found


def _python_enums() -> list[tuple[str, type[enum.Enum]]]:
    out = []
    for info in pkgutil.iter_modules(generated_pkg.__path__):
        module = importlib.import_module(f"medulla.generated.{info.name}")
        for name, obj in vars(module).items():
            if (
                isinstance(obj, type)
                and issubclass(obj, enum.Enum)
                and obj.__module__ == module.__name__
            ):
                out.append((f"{info.name}.{name}", obj))
    return out


# spec: MEDULLA-TYPED-PACKS-R001.10
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R001.10")
def test_generated_enums_equal_the_rust_enums() -> None:
    rust = _rust_enums()
    drift: list[str] = []
    checked: set[str] = set()
    for qualified, cls in _python_enums():
        name = qualified.rsplit(".", 1)[1]
        candidates = rust.get(name, [])
        if len(candidates) != 1:
            continue  # absent, shared name or not a plain enum: not comparable
        checked.add(name)
        python = {m.value for m in cls if (name, m.name) not in CLIENT_ONLY}
        if python != candidates[0]:
            drift.append(
                f"{qualified}: python-only {sorted(python - candidates[0])}, "
                f"rust-only {sorted(candidates[0] - python)}"
            )
    assert drift == []
    # The gate reports the enums it compared; the ones that matter are among them.
    assert {"PackViolationCode", "PackWarningCode", "AgentComponentKind"} <= checked
    assert len(checked) > 100, f"only {len(checked)} enums were comparable"


# spec: MEDULLA-TYPED-PACKS-R001.10
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R001.10")
def test_pack_violation_codes_match_engine() -> None:
    text = (
        ROOT / "crates/medulla-catalog-types/src/connector_pack/result.rs"
    ).read_text()
    body = re.search(r"pub enum PackViolationCode \{(.*?)\n\}", text, re.S)
    assert body is not None
    rust = {_wire(v, "SCREAMING_SNAKE_CASE") for v in _VARIANT.findall(body.group(1))}
    assert len(rust) >= 41
    assert {m.value for m in PackViolationCode} - {"UNKNOWN"} == rust


# spec: MEDULLA-TYPED-PACKS-R001.11
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R001.11")
def test_the_parity_gate_sees_planted_drift() -> None:
    planted = {"A_CODE", "B_CODE"}
    assert {m.value for m in PackViolationCode} != planted
    sample = "pub enum Planted {\n    OnlyInRust,\n}"
    assert _VARIANT.findall(sample) == ["OnlyInRust"]
    assert _wire("OnlyInRust", "SCREAMING_SNAKE_CASE") == "ONLY_IN_RUST"


# spec: MEDULLA-TYPED-PACKS-R001.12
@pytest.mark.spec("MEDULLA-TYPED-PACKS-R001.12")
def test_unknown_violation_code_keeps_the_raw_code() -> None:
    parsed = PackViolation.model_validate(
        {"code": "A_CODE_FROM_A_NEWER_ENGINE", "detail": "d", "uri": "u"}
    )
    assert parsed.code is PackViolationCode.UNKNOWN
    assert parsed.raw_code == "A_CODE_FROM_A_NEWER_ENGINE"
    known = PackViolation.model_validate({"code": "MALFORMED_INDEX", "detail": "d"})
    assert known.code is PackViolationCode.MALFORMED_INDEX
    assert known.raw_code is None
