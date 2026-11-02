"""MEDULLA-CONTRACT-R035: the CI workflow runs the `solve` test target with its
feature enabled, with no gap between a test target *declared* and one actually
*executed*.

`crates/medulla-decision-policy/tests/solve.rs` is gated
`#![cfg(feature = "solve")]` (cargo auto-discovers it as the `solve`
integration-test binary because the crate does not set `autotests = false`). If
the cargo-test invocation covering `medulla-decision-policy` ever stopped passing
`--all-features` (or an explicit `--features solve`), that whole test binary
would compile to zero tests and the step would report success while covering
nothing -- exactly the "source that no entry point ever compiles" gap this
requirement closes.

This audit parses the workflow YAML directly (it does not replace the native
jobs' own compilation/execution evidence); it exists so a future edit to the
workspace test step cannot narrow its feature selection or exclude the crate and
silently drop `solve` coverage without a test noticing.
"""

import re
import shlex
from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
FEATURE_GATE_RE = re.compile(r'#!\[cfg\(feature\s*=\s*"([^"]+)"\)\]')


def _job_steps():
    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
    for job in workflow["jobs"].values():
        yield from job.get("steps", [])


def _cargo_test_invocations(steps):
    """Yield (covered, excluded, all_features, features) per `cargo test` step.

    `covered` holds the `-p` packages, or `["--workspace"]` for a workspace run
    (whose `--exclude` packages are in `excluded`).
    """
    for step in steps:
        tokens = shlex.split(step.get("run", ""), comments=True)
        if "cargo" not in tokens:
            continue
        idx = tokens.index("cargo")
        if idx + 1 >= len(tokens) or tokens[idx + 1] != "test":
            continue
        covered = [tokens[i + 1] for i, t in enumerate(tokens) if t == "-p"]
        excluded = [tokens[i + 1] for i, t in enumerate(tokens) if t == "--exclude"]
        if "--workspace" in tokens:
            covered = ["--workspace"]
        if not covered:
            continue
        all_features = "--all-features" in tokens
        features = []
        if "--features" in tokens:
            raw = tokens[tokens.index("--features") + 1]
            features = raw.replace(",", " ").split()
        yield covered, excluded, all_features, features


def _required_feature(test_file: Path) -> str | None:
    match = FEATURE_GATE_RE.search(test_file.read_text())
    return match.group(1) if match else None


@pytest.mark.spec("MEDULLA-CONTRACT-R035")
def test_solve_step_enables_the_solve_feature():
    """Some `cargo test` step must cover `medulla-decision-policy` and enable the
    `solve` feature, or `tests/solve.rs` compiles to an empty test binary and the
    solver coverage this requirement names never actually executes."""
    crate = "medulla-decision-policy"
    invocations = [
        (all_features, features)
        for covered, excluded, all_features, features in _cargo_test_invocations(
            _job_steps()
        )
        if (crate in covered or "--workspace" in covered) and crate not in excluded
    ]
    assert invocations, f"CI must declare a cargo test invocation covering {crate}"
    assert any(
        all_features or "solve" in features for all_features, features in invocations
    ), (
        f"{crate}'s cargo test step must pass --all-features or --features solve "
        "so crates/medulla-decision-policy/tests/solve.rs is not silently skipped"
    )

    solve_test = ROOT / "crates" / crate / "tests" / "solve.rs"
    assert solve_test.is_file(), f"expected {solve_test} to exist"
    assert _required_feature(solve_test) == "solve", (
        "tests/solve.rs is expected to stay gated on the `solve` feature; if that "
        "changes, this audit's coverage mapping must be revisited alongside it"
    )
