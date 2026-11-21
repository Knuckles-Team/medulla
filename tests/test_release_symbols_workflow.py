"""Every release server target uploads a symbol artifact named by build-id (MEDULLA-CONTRACT-R060.2)."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.no_engine

WORKFLOW = Path(__file__).resolve().parents[1] / ".github" / "workflows" / "release.yml"


def _jobs() -> dict:
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))["jobs"]


def _runs(job: dict) -> str:
    return "\n".join(step.get("run", "") for step in job["steps"])


# spec: MEDULLA-CONTRACT-R060.2
@pytest.mark.spec("MEDULLA-CONTRACT-R060.2")
def test_every_server_target_uploads_a_symbol_artifact_named_by_build_id() -> None:
    jobs = _jobs()
    server = jobs["server-binary"]
    packaging = _runs(server)
    assert "scripts/package_release_symbols.sh" in packaging
    assert '"${{ matrix.name }}"' in packaging
    assert "dist-server-symbols" in packaging
    uploads = [
        step
        for step in server["steps"]
        if str(step.get("uses", "")).startswith("actions/upload-artifact@")
        and step["with"]["name"] == "symbols-${{ matrix.name }}"
    ]
    assert len(uploads) == 1
    assert uploads[0]["with"]["path"] == "dist-symbols/*"
    assert uploads[0]["with"]["if-no-files-found"] == "error"

    required = [
        leg["name"]
        for leg in server["strategy"]["matrix"]["include"]
        if not leg.get("optional", False)
    ]
    assert required == ["linux-aarch64", "windows-x86_64"]
    gate = jobs["symbols-gate"]
    assert gate["needs"] == ["server-binary", "build-x86"]
    checks = _runs(gate)
    assert "scripts/check_symbol_upload_set.py" in checks
    for name in required:
        assert name in checks
    assert "symbols-gate" in jobs["github-release"]["needs"]
    assert "gh release upload" in _runs(jobs["github-release"])


X86_WORKFLOW = (
    Path(__file__).resolve().parents[1] / ".github" / "workflows" / "medulla-release-x86.yml"
)


# spec: MEDULLA-CONTRACT-R060.4
@pytest.mark.spec("MEDULLA-CONTRACT-R060.4")
def test_linux_x86_64_symbol_artifact_is_uploaded_and_required() -> None:
    reusable = yaml.safe_load(X86_WORKFLOW.read_text(encoding="utf-8"))["jobs"]
    steps = next(job for job in reusable.values() if job.get("name", "").startswith("wheel linux-x86_64 (trusted tag)"))["steps"]
    packaging = [s for s in steps if "package_release_symbols.sh" in s.get("run", "")]
    assert len(packaging) == 1
    assert "linux-x86_64 dist-server-bin/medulla-server dist-server-symbols dist-symbols" in packaging[0]["run"]
    uploads = [
        s
        for s in steps
        if str(s.get("uses", "")).startswith("actions/upload-artifact@")
        and s["with"]["name"] == "symbols-linux-x86_64"
    ]
    assert len(uploads) == 1
    assert uploads[0]["with"]["path"] == "dist-symbols/*"
    assert uploads[0]["with"]["if-no-files-found"] == "error"
    assert steps.index(packaging[0]) < steps.index(uploads[0])

    jobs = _jobs()
    gate = jobs["symbols-gate"]
    assert "build-x86" in gate["needs"]
    assert "linux-aarch64 linux-x86_64 windows-x86_64" in _runs(gate)
