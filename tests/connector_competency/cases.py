"""Competency case table: schema and loader (MEDULLA-CQ-CONN-R067.7).

One file per connector rollup, ``cases/<connector>.json``. The loader refuses an
invalid file with the single code ``CASE_INVALID``.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
CASES_DIR = HERE / "cases"
RECORDINGS_DIR = HERE / "recordings"

CASE_INVALID = "CASE_INVALID"
KINDS = ("single", "cross")
ROLLUP = re.compile(r"^MEDULLA-CQ-CONN-R0(0[1-9]|[1-5][0-9]|6[0-6])$")
FILE_KEYS = ("connector", "rollup", "vocabulary_module", "terms", "questions")
QUESTION_KEYS = (
    "id",
    "kind",
    "uql",
    "sources",
    "optional_sources",
    "columns",
    "min_rows",
    "recordings",
)


class CaseInvalid(ValueError):
    """A case file broke the schema; ``code`` is always ``CASE_INVALID``."""

    code = CASE_INVALID

    def __init__(self, path: Path, reason: str) -> None:
        super().__init__(f"{CASE_INVALID}: {path.name}: {reason}")
        self.reason = reason


@dataclass(frozen=True)
class Case:
    """One competency question of one connector rollup."""

    id: str
    connector: str
    rollup: str
    kind: str
    uql: str
    sources: tuple[str, ...]
    optional_sources: tuple[str, ...]
    columns: tuple[str, ...]
    min_rows: int
    recordings: tuple[str, ...]
    vocabulary_module: str
    terms: tuple[str, ...]


def _require(ok: bool, path: Path, reason: str) -> None:
    if not ok:
        raise CaseInvalid(path, reason)


def _text(value: object) -> bool:
    return isinstance(value, str) and bool(value)


def _strings(value: object) -> bool:
    return isinstance(value, list) and all(isinstance(v, str) and v for v in value)


def _check_keys(doc: object, keys: tuple[str, ...], path: Path, what: str) -> dict:
    if not isinstance(doc, dict):
        raise CaseInvalid(path, f"{what} is not an object")
    missing = [k for k in keys if k not in doc]
    _require(not missing, path, f"{what} lacks {missing}")
    return doc


def _check_file(doc: dict, path: Path) -> None:
    _require(ROLLUP.match(str(doc["rollup"])) is not None, path, "unknown rollup")
    _require(doc["connector"] == path.stem, path, "connector differs from file name")
    _require(_text(doc["vocabulary_module"]), path, "vocabulary_module is empty")
    _require(_strings(doc["terms"]) and bool(doc["terms"]), path, "terms must be IRIs")
    _require(isinstance(doc["questions"], list), path, "questions is not a list")


def _check_question(q: dict, known: set[str], recordings: Path, path: Path) -> None:
    _require(q["kind"] in KINDS, path, f"{q['id']}: kind is not single or cross")
    _require(_text(q["uql"]), path, f"{q['id']}: uql is empty")
    for key in ("sources", "optional_sources", "columns", "recordings"):
        _require(_strings(q[key]), path, f"{q['id']}: {key} is not a string list")
    _require(
        isinstance(q["min_rows"], int) and q["min_rows"] >= 0,
        path,
        f"{q['id']}: min_rows",
    )
    _check_recordings(q, recordings, path)
    _check_text(q, known, path)


def _check_recordings(q: dict, recordings: Path, path: Path) -> None:
    for source in q["sources"]:
        mine = [r for r in q["recordings"] if r.split("/", 1)[0] == source]
        _require(bool(mine), path, f"{q['id']}: source {source} has no recording")
        _require(
            all((recordings / r).is_file() for r in mine),
            path,
            f"{q['id']}: recording of {source} is missing",
        )


def _check_text(q: dict, known: set[str], path: Path) -> None:
    allowed = set(q["sources"]) | set(q["optional_sources"])
    text = q["uql"].lower()
    for name in sorted(known - allowed):
        hit = re.search(rf"(?<![\w-]){re.escape(name.lower())}(?![\w-])", text)
        _require(hit is None, path, f"{q['id']}: text names connector {name}")


def _known_connectors(cases_dir: Path, recordings: Path) -> set[str]:
    names = {p.stem for p in cases_dir.glob("*.json")}
    if recordings.is_dir():
        names |= {p.name for p in recordings.iterdir() if p.is_dir()}
    return names


def _build(doc: dict, q: dict) -> Case:
    return Case(
        id=q["id"],
        connector=doc["connector"],
        rollup=doc["rollup"],
        kind=q["kind"],
        uql=q["uql"],
        sources=tuple(q["sources"]),
        optional_sources=tuple(q["optional_sources"]),
        columns=tuple(q["columns"]),
        min_rows=q["min_rows"],
        recordings=tuple(q["recordings"]),
        vocabulary_module=doc["vocabulary_module"],
        terms=tuple(doc["terms"]),
    )


def load_file(
    path: Path, cases_dir: Path = CASES_DIR, recordings: Path = RECORDINGS_DIR
) -> list[Case]:
    """Load one case file; raise ``CaseInvalid`` (``CASE_INVALID``) on any break."""
    try:
        raw = json.loads(path.read_text())
    except (OSError, ValueError) as err:
        raise CaseInvalid(path, f"unreadable: {err}") from err
    doc = _check_keys(raw, FILE_KEYS, path, "file")
    _check_file(doc, path)
    known = _known_connectors(cases_dir, recordings)
    out = []
    for q in doc["questions"]:
        q = _check_keys(q, QUESTION_KEYS, path, "question")
        _check_question(q, known, recordings, path)
        out.append(_build(doc, q))
    return out


def load_cases(
    cases_dir: Path = CASES_DIR, recordings: Path = RECORDINGS_DIR
) -> list[Case]:
    """Load every ``cases/<connector>.json`` in name order."""
    files = sorted(cases_dir.glob("*.json")) if cases_dir.is_dir() else []
    return [c for f in files for c in load_file(f, cases_dir, recordings)]
