"""MEDULLA-CONTRACT-R066.5: every duckdb-building job fetches the pinned library.

The workflows are parsed, not run. A job qualifies when one of its cargo or maturin
commands enables the duckdb feature: `source-duckdb` named, or `--all-features`
over the workspace (no `-p` restriction). Release jobs reach the build through the
serving scripts, which fetch the library themselves, so a script step counts when
its text fetches first.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
ACTION = "./.github/actions/duckdb-prebuilt"
FETCH = "fetch_duckdb_lib.py"
BUILD = re.compile(
    r"\b(?:cargo\s+(?:build|test|clippy|bench|check|run|nextest)|maturin\s+build)\b|cargo_test_rescue\.py"
)
SCRIPT = re.compile(r"scripts/[A-Za-z0-9_.-]+\.sh")


def _load(path: Path) -> dict:
    return yaml.safe_load(path.read_text(encoding="utf-8"))


def _steps(job: dict) -> list[dict]:
    """The job's steps in order, with each local composite action expanded in place."""
    flat: list[dict] = []
    for step in job.get("steps") or []:
        uses = step.get("uses", "")
        if uses.startswith("./.github/actions/") and uses != ACTION:
            action = _load(ROOT / uses[2:] / "action.yml")
            flat.extend(_steps({"steps": action["runs"].get("steps")}))
        else:
            flat.append(step)
    return flat


def _jobs() -> dict[str, dict]:
    found: dict[str, dict] = {}
    for path in sorted((ROOT / ".github/workflows").glob("*.yml")):
        for name, job in (_load(path).get("jobs") or {}).items():
            found[f"{path.name}:{name}"] = job
    return found


def _script_fetches_first(text: str) -> bool:
    """A serving script fetches before it builds and exports both variables."""
    if FETCH not in text:
        return False
    build = BUILD.search(text) or re.search(r"\bmaturin build\b", text)
    return bool(
        build
        and text.index(FETCH) < build.start()
        and "DUCKDB_LIB_DIR" in text
        and "DUCKDB_INCLUDE_DIR" in text
    )


def _is_fetch(step: dict) -> bool:
    if step.get("uses") == ACTION:
        return True
    run = step.get("run", "")
    return FETCH in run and "DUCKDB_LIB_DIR" in run


def _script_of(step: dict) -> str | None:
    match = SCRIPT.search(step.get("run", ""))
    if match and (ROOT / match.group(0)).is_file():
        text = (ROOT / match.group(0)).read_text(encoding="utf-8")
        return text if FETCH in text else None
    return None


def _enables_duckdb(command: str) -> bool:
    for line in command.splitlines():
        if not BUILD.search(line):
            continue
        if "source-duckdb" in line:
            return True
        if "--all-features" in line and not re.search(r"(?:^|\s)-p\s|--package", line):
            return True
    return False


def _matrix_flags(job: dict) -> list[str]:
    """Every `flags` value of the job matrix, so a templated command reads per leg."""
    matrix = (job.get("strategy") or {}).get("matrix") or {}
    return [
        entry["flags"]
        for value in matrix.values()
        if isinstance(value, list)
        for entry in value
        if isinstance(entry, dict) and "flags" in entry
    ]


def _step_enables_duckdb(step: dict, job: dict) -> bool:
    run = step.get("run", "")
    legs = [
        re.sub(r"\$\{\{\s*matrix\.\w+\.flags\s*\}\}", flags, run)
        for flags in _matrix_flags(job)
    ]
    if any(_enables_duckdb(command) for command in [run, *legs]):
        return True
    script = _script_of(step)
    return (script is not None and "source-duckdb" in script) or "source-duckdb" in str(
        step.get("with", "")
    )


def _builds_duckdb(job: dict) -> bool:
    return any(_step_enables_duckdb(step, job) for step in _steps(job))


# spec: MEDULLA-CONTRACT-R066.5
def test_every_duckdb_job_fetches_the_prebuilt_library() -> None:
    qualifying = {name: job for name, job in _jobs().items() if _builds_duckdb(job)}
    # The all-features test sweep, the all-features lint and the release server build.
    assert "ci.yml:gates-crates" in qualifying
    assert "ci.yml:quality-advisory" in qualifying
    assert any(name.startswith("release.yml:") for name in qualifying)
    failures = []
    for name, job in qualifying.items():
        steps = _steps(job)
        first_cargo = next(
            i for i, step in enumerate(steps) if _step_enables_duckdb(step, job)
        )
        fetched = False
        for step in steps[: first_cargo + 1]:
            if _is_fetch(step):
                fetched = True
            script = _script_of(step)
            if script is not None and _script_fetches_first(script):
                fetched = True
        if not fetched:
            failures.append(name)
    assert not failures, (
        f"jobs build the duckdb feature without fetching the library first: {failures}"
    )


# spec: MEDULLA-CONTRACT-R066.5
def test_the_fetch_step_exports_both_variables_and_keys_its_cache_on_the_pin() -> None:
    action = _load(ROOT / ".github/actions/duckdb-prebuilt/action.yml")
    steps = action["runs"]["steps"]
    text = "\n".join(str(step) for step in steps)
    assert FETCH in text
    assert "DUCKDB_LIB_DIR=" in text and "DUCKDB_INCLUDE_DIR=" in text
    assert "GITHUB_ENV" in text
    cache = next(
        step for step in steps if str(step.get("uses", "")).startswith("actions/cache@")
    )
    assert "hashFiles('scripts/duckdb-pin.toml')" in cache["with"]["key"]
    # No `|| true`: a checksum mismatch (DUCKDB_CHECKSUM_MISMATCH) must fail the job.
    assert "|| true" not in text and "continue-on-error" not in text


# spec: MEDULLA-CONTRACT-R066.5
def test_the_check_rejects_a_job_that_builds_without_fetching() -> None:
    job = {"steps": [{"run": "cargo test --locked --workspace --all-features"}]}
    assert _builds_duckdb(job)
    assert not any(_is_fetch(step) for step in _steps(job))
    assert not _enables_duckdb("cargo test --locked -p medulla-storage --all-features")
