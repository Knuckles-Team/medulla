"""The main-only all-features crate sweep frees runner disk before it builds."""

from __future__ import annotations

import pathlib

import pytest
import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]


@pytest.mark.spec("MEDULLA-SDLC-R010")
def test_full_crate_sweep_frees_hosted_runner_disk_before_building() -> None:
    # The ~140-crate all-features sweep filled the hosted runner's disk
    # ("No space left on device") on the 3.1.0 main run.
    steps = yaml.safe_load(
        (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    )["jobs"]["gates-crates"]["steps"]
    names = [step.get("name", "") for step in steps]
    free = names.index("Free hosted-runner disk for the full crate sweep")
    sweep = next(
        i
        for i, name in enumerate(names)
        if name.startswith("Test (every other workspace crate")
    )
    toolchain = next(
        i for i, step in enumerate(steps) if "rust-toolchain" in step.get("uses", "")
    )
    assert free < toolchain < sweep
    step = steps[free]
    assert (
        step["if"]
        == "github.job == 'gates-crates' && github.event_name != 'pull_request'"
    )
    assert (
        "/usr/share/dotnet" in step["run"] and "/usr/local/lib/android" in step["run"]
    )
