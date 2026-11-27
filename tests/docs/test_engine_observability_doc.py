"""The observability contract and guide state what the source defines."""

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
OBSERVE_SRC = ROOT / "crates" / "medulla-observe" / "src"
CONTRACT = ROOT / "specs" / "engine-observability" / "contracts" / "task-registry.md"
GUIDE = ROOT / "docs" / "interfaces" / "engine-observability.md"
KNOBS = ROOT / "crates" / "medulla-server-api" / "src" / "knob_config.rs"
INTROSPECT = ROOT / "crates" / "medulla-types" / "src" / "introspect.rs"
ERRORS = ROOT / "crates" / "medulla-types" / "src" / "contract" / "server_error_code.rs"

# The files whose public items a consumer binds a job through.
REGISTRY_API = ("spawn.rs", "progress.rs", "registry/tasks.rs")
PUBLIC_ITEM = re.compile(
    r"^\s*pub (?:async )?(?:fn|struct|enum|trait) ([A-Za-z_][A-Za-z_0-9]*)", re.M
)


def code_spans(text: str) -> str:
    """The text inside backtick spans, joined: what a page states as code."""
    return "\n".join(re.findall(r"`([^`]+)`", text))


def public_items() -> set[str]:
    items: set[str] = set()
    for relative in REGISTRY_API:
        source = (OBSERVE_SRC / relative).read_text()
        items.update(PUBLIC_ITEM.findall(source))
    return items


@pytest.mark.spec("MEDULLA-OBSERVE-R003.7")
def test_task_registry_contract_names_every_public_item():
    items = public_items()
    # A scan that finds nothing is a drifted scan, not an empty API.
    assert len(items) >= 25, sorted(items)
    spans = code_spans(CONTRACT.read_text())
    missing = sorted(
        item for item in items if not re.search(rf"\b{re.escape(item)}\b", spans)
    )
    assert not missing, f"the contract names no code span for: {missing}"
    # The contract states the cursor bound the registry enforces.
    registry = (OBSERVE_SRC / "registry" / "tasks.rs").read_text()
    bound = re.search(r"MAX_OPAQUE_CURSOR_BYTES: usize = (\d+)", registry)
    assert bound is not None
    assert f"at most {bound.group(1)} bytes" in CONTRACT.read_text()
    # It names the spec rows and the consumer rule.
    text = CONTRACT.read_text()
    assert "MEDULLA-OBSERVE-R003" in text and "Depends-on" in text


def knob_names() -> set[str]:
    return set(re.findall(r'"(observe\.[a-z_]+)"', KNOBS.read_text()))


def section_names() -> set[str]:
    source = INTROSPECT.read_text()
    enum = source.split("pub enum IntrospectSection", 1)[1].split("\n}", 1)[0]
    variants = re.findall(r"^\s*([A-Z][A-Za-z]+),", enum, re.M)

    def snake(name: str) -> str:
        return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()

    return {snake(variant) for variant in variants}


def error_codes() -> set[str]:
    source = ERRORS.read_text()
    wanted = {
        "INTROSPECT_LIMIT_INVALID",
        "INTROSPECT_CURSOR_INVALID",
        "PROFILE_BUSY",
        "PROFILE_DURATION_INVALID",
        "PROFILE_UNAVAILABLE",
        "PARTIAL_MATERIALIZATION",
        "NO_PROGRESS",
        "TRANSITION_REFUSED",
    }
    return {code for code in wanted if f'"{code}"' in source}


@pytest.mark.spec("MEDULLA-OBSERVE-R013")
def test_guide_matches_codes_knobs_and_sections():
    guide = GUIDE.read_text()
    spans = code_spans(guide)
    knobs = knob_names()
    assert len(knobs) >= 11, sorted(knobs)
    missing = sorted(knob for knob in knobs if knob not in spans)
    assert not missing, f"the guide states no row for the knobs: {missing}"
    sections = section_names()
    assert {
        "requests",
        "tasks",
        "runtime",
        "locks",
        "readiness",
        "startup",
        "build",
        "events",
    } <= sections
    missing = sorted(name for name in sections if name not in spans)
    assert not missing, f"the guide states no row for the sections: {missing}"
    codes = error_codes()
    assert len(codes) == 8, sorted(codes)
    missing = sorted(code for code in codes if code not in spans)
    assert not missing, f"the guide states no row for the codes: {missing}"
    # The log targets the engine writes are the ones the guide lists.
    for target in (
        "medulla::request",
        "medulla::transition",
        "medulla::refusal",
        "medulla::startup",
        "medulla::build",
    ):
        assert target in spans, f"the guide lists no log target {target}"
    # Nothing the guide calls a knob is absent from the table.
    stated = set(re.findall(r"observe\.[a-z_]+", spans))
    assert stated <= knobs, f"the guide names knobs that are not rows: {stated - knobs}"
    # The symbolization command it documents exists.
    assert "scripts/symbolize.sh" in guide
    assert (ROOT / "scripts" / "symbolize.sh").is_file()
