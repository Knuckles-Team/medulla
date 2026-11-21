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
    assert gate["needs"] == "server-binary"
    checks = _runs(gate)
    assert "scripts/check_symbol_upload_set.py" in checks
    for name in required:
        assert name in checks
    assert "symbols-gate" in jobs["github-release"]["needs"]
    assert "gh release upload" in _runs(jobs["github-release"])
