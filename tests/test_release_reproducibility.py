"""Release fixed point: two builds of one tag agree (MEDULLA-CONTRACT-R053.14)."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / ".github" / "workflows" / "release.yml"
PUBLISH_JOBS = ("verify-pypi", "server-manifest", "github-release")


def _script():
    spec = importlib.util.spec_from_file_location(
        "compare_release_artifacts", ROOT / "scripts" / "compare_release_artifacts.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _build(root: Path, name: str, sdist: bytes, image: str) -> tuple[Path, Path]:
    directory = root / name
    directory.mkdir()
    (directory / "medulla-1.0.0.tar.gz").write_bytes(sdist)
    digests = root / f"{name}-images.txt"
    digests.write_text(f"server-x86_64 {image}\n", encoding="utf-8")
    return directory, digests


def _run(tmp_path: Path, primary, reproduction) -> int:
    return _script().main(
        [
            str(primary[0]),
            str(reproduction[0]),
            "--primary-images",
            str(primary[1]),
            "--reproduction-images",
            str(reproduction[1]),
        ]
    )


@pytest.mark.spec("MEDULLA-CONTRACT-R053.14")
def test_a_differing_sdist_or_image_digest_is_refused_with_both_digests(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    same = _build(tmp_path, "a", b"sdist", "sha256:aa")
    again = _build(tmp_path, "b", b"sdist", "sha256:aa")
    assert _run(tmp_path, same, again) == 0
    assert "release_fixed_point=passed" in capsys.readouterr().out

    sdist_differs = _build(tmp_path, "c", b"sdist-with-a-timestamp", "sha256:aa")
    assert _run(tmp_path, same, sdist_differs) == 1
    out = capsys.readouterr().out
    left = "sha256:" + __import__("hashlib").sha256(b"sdist").hexdigest()
    assert f"medulla-1.0.0.tar.gz primary={left} reproduction=sha256:" in out

    image_differs = _build(tmp_path, "d", b"sdist", "sha256:bb")
    assert _run(tmp_path, same, image_differs) == 1
    out = capsys.readouterr().out
    assert "image:server-x86_64 primary=sha256:aa reproduction=sha256:bb" in out

    missing = _build(tmp_path, "e", b"sdist", "sha256:aa")
    (missing[0] / "medulla-1.0.0.tar.gz").unlink()
    assert _run(tmp_path, same, missing) == 1
    assert "reproduction=<absent>" in capsys.readouterr().out


@pytest.mark.spec("MEDULLA-CONTRACT-R053.14")
def test_publish_jobs_need_the_release_fixed_point_job() -> None:
    jobs = yaml.safe_load(RELEASE.read_text(encoding="utf-8"))["jobs"]
    check = jobs["release-fixed-point"]
    assert set(check["needs"]) == {"build-sdist", "server-image"}
    assert any(
        "compare_release_artifacts.py" in step.get("run", "") for step in check["steps"]
    )
    for name in PUBLISH_JOBS:
        needs = jobs[name]["needs"]
        needs = [needs] if isinstance(needs, str) else needs
        assert "release-fixed-point" in needs, name
    assert "needs.release-fixed-point.result == 'success'" in jobs["verify-pypi"]["if"]
    image = "\n".join(step.get("run", "") for step in jobs["server-image"]["steps"])
    assert image.count("docker buildx build") == 2
    assert image.count("rewrite-timestamp=true") == 2
    assert "SOURCE_DATE_EPOCH" in image
    sdist = "\n".join(step.get("run", "") for step in jobs["build-sdist"]["steps"])
    assert "SOURCE_DATE_EPOCH" in sdist and "dist-sdist-reproduction" in sdist


@pytest.mark.spec("MEDULLA-CONTRACT-R053.14")
def test_nightly_builds_the_linux_x86_64_wheel_twice_and_compares() -> None:
    doc = yaml.safe_load(
        (RELEASE.parent / "nightly-wheel-reproducibility.yml").read_text("utf-8")
    )
    triggers = doc[True] if True in doc else doc["on"]
    assert "schedule" in triggers
    assert doc["env"]["MEDULLA_WHEEL_TARGET"] == "x86_64-unknown-linux-gnu"
    (job,) = doc["jobs"].values()
    passes = [
        step["with"]["pass"]
        for step in job["steps"]
        if step.get("uses") == "./.github/actions/folded-wheel"
    ]
    assert passes == ["primary", "reproduction"]
    runs = "\n".join(step.get("run", "") for step in job["steps"])
    assert "compare_wheel_reproducibility.py dist-primary dist-reproduction" in runs
    assert "SOURCE_DATE_EPOCH" in runs
