"""The server ships inside the medulla wheel (MEDULLA-CONTRACT-R053.3, .9-.12)."""

from __future__ import annotations

import os
import stat
import subprocess
import sys
import zipfile
from pathlib import Path

import pytest
import tomllib
import yaml

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / ".github/workflows/release.yml"
CI = ROOT / ".github/workflows/ci.yml"
ACTION = ROOT / ".github/actions/folded-wheel/action.yml"
BUILD_SCRIPT = ROOT / "scripts/build_release_server.sh"
INJECT = ROOT / "scripts/inject_server_binary.py"
SMOKE = ROOT / "scripts/release_wheel_smoke.sh"
DOCKERFILE = ROOT / "docker/Dockerfile"
# The retired second distribution (its project and wheel names) and its import.
RETIRED = ("medulla" + "-stem", "medulla" + "_stem", "import " + "medulla_server")


def _yaml(path: Path) -> dict:
    return yaml.safe_load(path.read_text(encoding="utf-8"))


def _script(job: dict) -> str:
    return "\n".join(str(step.get("run", "")) for step in job["steps"])


def _version() -> str:
    return tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]


@pytest.mark.spec("MEDULLA-CONTRACT-R053.3")
def test_one_distribution_no_server_extra_and_no_retired_name() -> None:
    pyproject = tomllib.loads((ROOT / "pyproject.toml").read_text())
    assert pyproject["project"]["name"] == "medulla"
    assert "server" not in pyproject["project"]["optional-dependencies"]
    assert "sources" not in pyproject.get("tool", {}).get("uv", {})
    assert not (ROOT / "packaging").exists()
    assert not (ROOT / ".github/actions/build-server-wheel").exists()
    lock = (ROOT / "uv.lock").read_text()
    assert 'source = { directory = "' not in lock
    surfaces = [
        ROOT / "pyproject.toml",
        ROOT / "uv.lock",
        ROOT / ".config/bumpversion.cfg",
        DOCKERFILE,
        ROOT / "README.md",
        *sorted((ROOT / ".github").rglob("*.yml")),
        *sorted((ROOT / "scripts").glob("*.py")),
        *sorted((ROOT / "scripts").glob("*.sh")),
        *sorted((ROOT / "medulla").rglob("*.py")),
    ]
    for path in surfaces:
        text = path.read_text(encoding="utf-8")
        for name in (*RETIRED, "from " + "medulla_server"):
            assert name not in text, f"{path.relative_to(ROOT)} names {name}"


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
def test_folded_wheel_builds_the_server_per_target_and_folds_it_in() -> None:
    steps = _yaml(ACTION)["runs"]["steps"]
    names = [s.get("name", "") for s in steps]
    engine = steps[names.index("Build engine extension wheel")]
    assert engine["with"]["prepare-script"] == (
        "${{ inputs.server-artifact == '' && 'scripts/build_release_server.sh' || '' }}"
    )
    native = steps[names.index("Build the release server executable (native)")]
    assert native["if"] == "${{ inputs.server-artifact == '' && runner.os != 'Linux' }}"
    assert native["run"] == "bash scripts/build_release_server.sh"
    swap = steps[names.index("Add swap for the server link")]
    assert "runner.environment == 'github-hosted'" in swap["if"]
    fold = steps[names.index("Fold the server executable into the medulla wheel")]
    assert "scripts/inject_server_binary.py" in fold["run"]
    assert "medulla-server(\\.exe)?" in fold["run"]
    order = [
        names.index(n)
        for n in (
            "Add swap for the server link",
            "Build engine extension wheel",
            "Fold numeric kernel into the medulla wheel",
            "Fold the server executable into the medulla wheel",
            "Strip console scripts",
            "Normalize and audit wheel",
        )
    ]
    assert order == sorted(order)
    complete = next(
        s["run"] for s in steps if "check_wheel_completeness" in s.get("run", "")
    )
    assert "--require-server-binary" in complete
    script = BUILD_SCRIPT.read_text()
    assert "--profile release-server --features full,ast-extended" in script
    assert "--bin medulla-server" in script
    assert "medulla-server.exe" in script
    assert "aarch64-unknown-linux-gnu | *-apple-darwin)" in script
    assert "CARGO_PROFILE_RELEASE_SERVER_LTO=thin" in script
    assert os.access(BUILD_SCRIPT, os.X_OK)


def _wheel(path: Path, members: dict[str, bytes]) -> Path:
    with zipfile.ZipFile(path, "w") as archive:
        for name, data in members.items():
            archive.writestr(name, data)
        archive.writestr("medulla-1.0.dist-info/RECORD", "")
    return path


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
@pytest.mark.parametrize("exe", ["medulla-server", "medulla-server.exe"])
def test_inject_places_an_executable_server_member_and_rewrites_record(
    tmp_path: Path, exe: str
) -> None:
    wheel = _wheel(
        tmp_path / "medulla-1.0-cp39-abi3-linux_x86_64.whl",
        {
            "medulla/__init__.py": b"",
            "medulla/_bin/medulla-server": b"stale",
        },
    )
    binary = tmp_path / exe
    binary.write_bytes(b"\x7fELF server")
    for _ in range(2):  # idempotent: a second fold replaces the member
        subprocess.run(
            [sys.executable, str(INJECT), str(wheel), str(binary)], check=True
        )
    with zipfile.ZipFile(wheel) as archive:
        members = [i for i in archive.infolist() if "/_bin/" in i.filename]
        record = archive.read("medulla-1.0.dist-info/RECORD").decode()
        assert [i.filename for i in members] == [f"medulla/_bin/{exe}"]
        mode = members[0].external_attr >> 16
        assert stat.S_ISREG(mode) and mode & 0o777 == 0o755
        assert archive.read(f"medulla/_bin/{exe}") == b"\x7fELF server"
        assert f"medulla/_bin/{exe},sha256=" in record
        assert record.rstrip().endswith("medulla-1.0.dist-info/RECORD,,")


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
def test_inject_refuses_anything_but_the_server_executable(tmp_path: Path) -> None:
    wheel = _wheel(tmp_path / "medulla-1.0-py3-none-any.whl", {"medulla/x.py": b""})
    other = tmp_path / "nemesis"
    other.write_bytes(b"x")
    done = subprocess.run(
        [sys.executable, str(INJECT), str(wheel), str(other)],
        capture_output=True,
        text=True,
    )
    assert done.returncode != 0
    assert "not a medulla-server executable" in done.stderr


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
def test_release_builds_publish_and_verify_only_medulla() -> None:
    jobs = _yaml(RELEASE)["jobs"]
    assert "build-server" not in jobs
    assert set(jobs["verify-pypi"]["needs"]) == {"build", "publish-x86", "build-sdist"}
    verify = _script(jobs["verify-pypi"])
    assert "--package medulla " in verify
    for tag in ("manylinux_2_28_x86_64", "manylinux_2_28_aarch64", "win_amd64"):
        assert tag in verify
    assert "server-wheel-cache" not in _yaml(CI)["jobs"]
    smoke = SMOKE.read_text()
    assert "resolve_server_binary" in smoke
    assert '[binary, "--version"]' in smoke
    assert 'f"medulla-server {version}"' in smoke


@pytest.mark.spec("MEDULLA-CONTRACT-R053.10")
def test_server_reports_the_release_version() -> None:
    main = (ROOT / "src/main.rs").read_text()
    head = main[main.index("#[derive(Parser, Debug)]") : main.index("struct Args")]
    assert "#[command(version)]" in head
    assert '#[command(name = "medulla-server")]' in head
    cfg = (ROOT / ".config/bumpversion.cfg").read_text()
    assert "[bumpversion:file:Cargo.toml]" in cfg


@pytest.mark.spec("MEDULLA-CONTRACT-R053.11")
def test_image_is_built_from_the_medulla_wheel_and_published_as_one_manifest() -> None:
    dockerfile = DOCKERFILE.read_text()
    assert "AS runtime" in dockerfile
    assert "/wheels/medulla-*.whl" in dockerfile
    assert '"medulla==${EGS_VERSION}"' in dockerfile
    assert "*/site-packages/medulla/_bin/medulla-server" in dockerfile
    assert '"${server}" --version' in dockerfile
    assert "EGS_VERSION must be numeric" in dockerfile
    assert 'ENTRYPOINT ["medulla-server"]' in dockerfile
    assert "EXPOSE 9100 9101" in dockerfile
    assert "USER 10001:10001" in dockerfile
    jobs = _yaml(RELEASE)["jobs"]
    image = jobs["server-image"]
    assert image["needs"] == ["guard", "build", "build-x86"]
    assert "startsWith(github.ref, 'refs/tags/v')" in image["if"]
    legs = {
        leg["platform"]: leg["wheel"] for leg in image["strategy"]["matrix"]["include"]
    }
    assert legs == {
        "linux/amd64": "wheel-linux-x86_64",
        "linux/arm64": "wheel-linux-aarch64",
    }
    assert "push-by-digest=true" in _script(image)
    manifest = jobs["server-manifest"]
    assert manifest["needs"] == "server-image"
    assert "docker buildx imagetools create" in _script(manifest)
    assert "medulla-server" in str(manifest["steps"])


@pytest.mark.spec("MEDULLA-CONTRACT-R053.12")
def test_one_version_and_a_build_only_dispatch() -> None:
    version = _version()
    assert f"ARG EGS_VERSION={version}" in DOCKERFILE.read_text()
    cfg = (ROOT / ".config/bumpversion.cfg").read_text()
    assert "[bumpversion:file:docker/Dockerfile]" in cfg
    assert "ARG EGS_VERSION={current_version}" in cfg
    doc = _yaml(RELEASE)
    triggers = doc[True] if True in doc else doc["on"]
    assert set(triggers["workflow_dispatch"]["inputs"]) == {"force_untested"}
    guard = _script(doc["jobs"]["guard"])
    assert "head_sha=${SHA}&status=success" in guard
    tag_push = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')"
    for job in doc["jobs"].values():
        for step in job.get("steps", []):
            body = str(step.get("run", "")) + str(step.get("uses", ""))
            if "publish-distribution" in body or "twine upload" in body:
                assert tag_push in str(step.get("if", "")) + str(job.get("if", ""))
    assert tag_push in doc["jobs"]["server-image"]["if"]


def test_tag_release_creates_the_github_release_after_pypi() -> None:
    """The GitHub release follows the PyPI check and the codec requalification."""
    job = _yaml(RELEASE)["jobs"]["github-release"]
    assert job["needs"] == ["verify-pypi", "method-codec"]
    assert job["permissions"] == {"contents": "write"}
    script = _script(job)
    assert "https://pypi.org/project/medulla/$version/" in script
    assert 'pip install \\"medulla==$version\\"' in script
    assert "--latest" in script


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
def test_release_compiles_the_server_once_per_platform_and_folds_those_bytes() -> None:
    release = _yaml(ROOT / ".github/workflows/release.yml")["jobs"]
    server, build = release["server-binary"], release["build"]

    def legs(job: dict) -> dict:
        return {m["name"]: m["target"] for m in job["strategy"]["matrix"]["include"]}

    assert legs(server) == legs(build)
    assert any(
        s.get("uses") == "./.github/actions/server-binary" for s in server["steps"]
    )
    upload = next(s for s in server["steps"] if "upload-artifact" in s.get("uses", ""))
    assert upload["with"]["name"] == "server-${{ matrix.name }}"
    folds = [
        s for s in build["steps"] if s.get("uses") == "./.github/actions/folded-wheel"
    ]
    assert [f["with"]["pass"] for f in folds] == ["primary", "reproduction"]
    assert {f["with"]["server-artifact"] for f in folds} == {
        "server-${{ matrix.name }}"
    }
    names = [s.get("name", "") for s in _yaml(ACTION)["runs"]["steps"]]
    assert names.index("Download the prebuilt server executable") < names.index(
        "Fold the server executable into the medulla wheel"
    )
    action = _yaml(ROOT / ".github/actions/server-binary/action.yml")["runs"]["steps"]
    assert any(
        s.get("run", "") == "bash scripts/build_release_server.sh" for s in action
    )
