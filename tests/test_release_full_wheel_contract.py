"""Release wheels must all carry the full engine and folded numeric kernel."""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest
import tomllib
import yaml

pytestmark = pytest.mark.no_engine

REPO = Path(__file__).resolve().parents[1]
WORKFLOW = REPO / ".github" / "workflows" / "release.yml"
CI_WORKFLOW = REPO / ".github" / "workflows" / "ci.yml"
WHEEL_PASS_ACTION = REPO / ".github" / "actions" / "folded-wheel" / "action.yml"
NUMERIC_CONTRACT_DOCS = (
    REPO / "AGENTS.md",
    REPO / "README.md",
    REPO / "docs" / "architecture" / "numeric_kernel.md",
    REPO / "docs" / "architecture" / "analytics_program.md",
    REPO / "docs" / "capabilities.md",
    REPO / "docs" / "concepts.md",
)


def _build_job_source(raw: str) -> str:
    """Isolate the `build` job's YAML text from the rest of `release.yml`.

    `release.yml` now also carries the `gates` job (the former rust-ci.yml
    test suite), which legitimately uses `toolchain: stable` (to run cargo
    test/clippy) and `--no-default-features` (the slim-server build check) —
    neither describes the release wheel matrix. Callers asserting on
    wheel-build-only behavior must check this slice, not the whole file, or
    they'll trip on the unrelated `gates` job content.
    """
    start = raw.index("\n  build:\n")
    end = raw.index("\n  build-x86:\n", start)
    return raw[start:end]


def test_maturin_default_and_python_extra_are_full() -> None:
    project = tomllib.loads((REPO / "pyproject.toml").read_text(encoding="utf-8"))
    maturin = project["tool"]["maturin"]
    # The public wheel is the medulla-pyengine extension crate, not the server bin.
    assert maturin["features"] == ["python", "security"]
    assert maturin["manifest-path"] == "crates/medulla-pyengine/Cargo.toml"
    assert maturin["module-name"] == "medulla.engine"
    assert maturin.get("bindings") != "bin"
    dependencies = project["project"]["dependencies"]
    assert not any(
        re.split(r"[\[<>=!~;]", dependency, maxsplit=1)[0].lower() == "numpy"
        for dependency in dependencies
    )
    assert "pyoxigraph>=0.5.11" in dependencies
    assert "httpx>=0.28.1" in dependencies
    optional = project["project"]["optional-dependencies"]
    assert optional["quant"] == []
    assert all(
        "numpy" not in requirement.lower()
        for requirements in optional.values()
        for requirement in requirements
    )
    assert optional["full"] == []
    assert optional["all"] == []
    assert optional["lake-parity"] == []
    lake_requirements = (REPO / "tests" / "lake-parity-requirements.txt").read_text(
        encoding="utf-8"
    )
    assert "msgpack>=1.2.1" in lake_requirements
    assert "pyiceberg[pyarrow]>=0.7.0" in lake_requirements
    assert "deltalake>=0.18.0" in lake_requirements


def test_current_numeric_docs_match_the_builtin_boundary_contract() -> None:
    """Current docs must not revive removed NumPy/native-ABI promises."""

    docs = {path: path.read_text(encoding="utf-8") for path in NUMERIC_CONTRACT_DOCS}
    numeric = docs[REPO / "docs" / "architecture" / "numeric_kernel.md"]
    assert "bounded built-in" in numeric
    assert "scalar↔nested-list PyO3 contract" in numeric
    assert "isolated NumPy parity oracle" in numeric
    assert "Arrow `KnowledgeBatch` currency" in numeric
    forbidden_current_claims = (
        "rust-numpy",
        "numpy-shim",
        "kernel-owned numpy tail",
        "numeric interoperability dependencies",
        "zero-copy + allow_threads",
    )
    for path, text in docs.items():
        lowered = text.lower()
        for claim in forbidden_current_claims:
            assert claim not in lowered, f"stale numeric claim in {path}: {claim}"


def test_agent_skills_have_one_canonical_owner() -> None:
    """Every authored skill has one canonical identity in the engine provider."""

    project = tomllib.loads((REPO / "pyproject.toml").read_text(encoding="utf-8"))
    entry_points = project["project"].get("entry-points", {})
    assert entry_points["agent_utilities.skill_providers"] == {
        "medulla": "medulla.skills"
    }
    skills = REPO / "medulla" / "skills"
    assert (skills / "__init__.py").is_file()
    try:
        out = subprocess.run(
            ["git", "-C", str(skills), "ls-files", "--", "SKILL.md", "*/SKILL.md"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        skill_md_files = [skills / line for line in out.splitlines() if line]
    except (subprocess.CalledProcessError, FileNotFoundError):
        skill_md_files = []
    if not skill_md_files:
        # BUG-043: prefer the git-tracked set (a raw rglob also picks up
        # gitignored, generated build output); fall back to a filesystem
        # walk only when this checkout is not inside a git working tree.
        skill_md_files = list(skills.rglob("SKILL.md"))
    assert skill_md_files, "the engine provider must not publish an empty catalog"
    identities: set[str] = set()
    for path in skill_md_files:
        assert path.resolve().parent.parent == skills.resolve(), (
            f"skill must have one canonical directory in the engine package: {path}"
        )
        document = path.read_text(encoding="utf-8")
        assert document.startswith("---\n"), f"missing skill frontmatter: {path}"
        frontmatter = yaml.safe_load(document.split("---", 2)[1])
        assert isinstance(frontmatter, dict), f"invalid skill frontmatter: {path}"
        identity = frontmatter.get("name")
        assert identity == path.parent.name, f"skill identity/path mismatch: {path}"
        assert identity.casefold() not in identities, f"duplicate skill: {identity}"
        identities.add(identity.casefold())
    assert "medulla-development" in identities, (
        "the development skill must ship from the engine provider, not a sibling"
    )


def _verified_x86_workflow() -> dict:
    """Bind the local structural proof to the reviewed immutable workflow."""
    caller = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    expected = (
        "Knuckles-Team/medulla/.github/workflows/medulla-release-x86.yml@"
        "3b67ca6c44d7efbed4c834f0dafbac023883da87"
    )
    assert caller["jobs"]["build-x86"]["uses"] == expected
    source = (WORKFLOW.parent / "medulla-release-x86.yml").read_bytes()
    assert hashlib.sha256(source).hexdigest() == (
        "def56dc51f4f861f2f0da25e816a73afbf4ba4983641d1668006b69e60115b0c"
    )
    return yaml.safe_load(source)


def test_every_supported_release_target_uses_one_full_wheel_pipeline() -> None:
    raw = WORKFLOW.read_text(encoding="utf-8")
    workflow = yaml.safe_load(raw)
    matrix = workflow["jobs"]["build"]["strategy"]["matrix"]["include"]
    targets = {entry["name"]: entry["target"] for entry in matrix}
    assert len(matrix) == len(targets) == 3
    arm = next(entry for entry in matrix if entry["name"] == "linux-aarch64")
    assert arm["runner"] == "ubuntu-24.04-arm"
    assert arm["jobs"] == 1
    x86 = _verified_x86_workflow()["jobs"]["wheel"]
    assert x86["env"]["MATURIN_FEATURES"] == "full,ast-extended"
    upload = next(
        step
        for step in x86["steps"]
        if step.get("uses", "").startswith("actions/upload-artifact@")
    )
    target_name = upload["with"]["name"].removeprefix("wheel-")
    assert target_name == "linux-x86_64" and target_name not in targets
    targets[target_name] = x86["env"]["MEDULLA_WHEEL_TARGET"]
    assert targets == {
        "linux-aarch64": "aarch64-unknown-linux-gnu",
        "linux-x86_64": "x86_64-unknown-linux-gnu",
        "macos-aarch64": "aarch64-apple-darwin",
        "windows-x86_64": "x86_64-pc-windows-msvc",
    }
    assert "MATURIN_FEATURES" not in raw
    assert "--no-default-features" not in _build_job_source(raw)
    both = raw + CI_WORKFLOW.read_text(encoding="utf-8")
    assert "scripts/inject_numeric_kernel.py" in both
    assert "scripts/normalize_wheel_build_paths.py" in both
    assert "import medulla.numeric" in both
    # The release smoke test must exercise the native module without importing
    # or installing NumPy; parity keeps its reference dependency isolated in the
    # separate gates job.
    assert "import numpy" not in _build_job_source(raw)


def test_release_wheels_are_rebuilt_and_compared_reproducibly() -> None:
    raw = WORKFLOW.read_text(encoding="utf-8")
    toolchain = tomllib.loads(
        (REPO / "rust-toolchain.toml").read_text(encoding="utf-8")
    )["toolchain"]["channel"]

    assert re.fullmatch(r"[0-9]+[.][0-9]+[.][0-9]+", toolchain)
    build_job_raw = _build_job_source(raw)
    assert "uses: ./.github/actions/pinned-rust-toolchain" in build_job_raw
    pinned = (REPO / ".github/actions/pinned-rust-toolchain/action.yml").read_text(
        encoding="utf-8"
    )
    assert "tomllib.load(open('rust-toolchain.toml','rb'))" in pinned
    assert "toolchain: ${{ steps.pin.outputs.channel }}" in pinned
    assert "toolchain: stable" not in build_job_raw
    # Wheel legs (thin LTO, zig/qemu cross builds) keep a low job count.
    # The byte-identical reproduction pass is a manual (workflow_dispatch) check,
    # so tag releases build each wheel once; the publish candidate is always the
    # primary pass.
    assert "Build reproduction wheel" in build_job_raw
    assert build_job_raw.count("uses: ./.github/actions/folded-wheel") == 2
    assert "pass: primary" in build_job_raw
    assert "pass: reproduction" in build_job_raw
    assert build_job_raw.count("github.event_name == 'workflow_dispatch'") >= 3
    assert "cp dist-primary/medulla-*.whl dist/" in build_job_raw
    assert 'CARGO_INCREMENTAL: "0"' in raw
    assert "max-parallel: 1" in raw
    assert "SOURCE_DATE_EPOCH=" in raw
    # One pass (.github/actions/folded-wheel) builds the engine extension wheel,
    # the numeric module and the release server into one shared target directory,
    # folds the numeric module and the server in, then normalizes, audits and
    # checks the result. The server build lives in scripts/build_release_server.sh.
    wheel_pass = WHEEL_PASS_ACTION.read_text(encoding="utf-8")
    for output in ("dist", "numdist"):
        assert f"--out {output}-${{{{ inputs.pass }}}}" in wheel_pass
    assert "enginedist" not in wheel_pass
    assert "--profile release-server" not in wheel_pass
    assert wheel_pass.count("scripts/build_release_server.sh") == 2
    assert wheel_pass.count("uses: ./.github/actions/release-maturin") == 2
    shared = yaml.safe_load(
        (REPO / ".github/actions/release-maturin/action.yml").read_text()
    )["runs"]["steps"]
    assert len(shared) == 1
    assert shared[0]["uses"] == (
        "PyO3/maturin-action@e83996d129638aa358a18fbd1dfb82f0b0fb5d3b"
    )
    assert shared[0]["with"]["sccache"] == "false"
    assert ("CARGO_TARGET_DIR=$RUNNER_TEMP/medulla-release-target") in wheel_pass
    for script in (
        "scripts/inject_numeric_kernel.py",
        "scripts/normalize_wheel_sbom.py",
        "scripts/normalize_wheel_build_paths.py",
        "scripts/check_wheel_privacy.py",
        "scripts/inject_server_binary.py",
        "xtask wheel-completeness --require-engine-kernel",
    ):
        assert wheel_pass.count(script) == 1, script
    # The `gates` job folds its OWN numeric kernel into a real wheel
    # (CONCEPT:MEDULLA-346) and normalizes/audits its numeric kernel, pyengine
    # kernel and folded wheel, rather than `pip install`ing the standalone
    # `medulla-numeric` build (the 2026-08-21 stale-install incident).
    ci_raw = CI_WORKFLOW.read_text(encoding="utf-8")
    both = raw + ci_raw
    # Artifact recipe hashes also name these scripts, but do not execute them.
    # Count actual Python invocations in run steps, not filenames in the YAML.
    commands = "\n".join(
        step.get("run", "")
        for source in (raw, ci_raw)
        for job in yaml.safe_load(source)["jobs"].values()
        for step in job.get("steps", [])
    )
    for script, expected in (
        ("inject_numeric_kernel", 1),
        ("normalize_wheel_sbom", 2),
        ("normalize_wheel_build_paths", 2),
        ("check_wheel_privacy", 2),
    ):
        pattern = rf"(?m)^\s*python3? scripts/{script}\.py(?=\s|$)"
        assert len(re.findall(pattern, commands)) == expected, script
    assert "Remove primary native build state" in raw
    assert "release wheel digest mismatch" in both


def test_incomplete_fallback_artifacts_cannot_be_published() -> None:
    raw = WORKFLOW.read_text(encoding="utf-8")
    assert "command: sdist" not in raw
    jobs = yaml.safe_load(raw)["jobs"]
    assert "publish-pypi" not in jobs
    publish = jobs["publish-x86"]
    assert publish["needs"] == "build-x86"
    assert publish["permissions"] == {"contents": "read"}
    steps = publish["steps"]
    gate = next(
        step["run"]
        for step in steps
        if step.get("name") == "Require a complete wheel before upload"
    )
    assert "xtask wheel-completeness --require-engine-kernel dist/*.whl" in gate
    names = [step.get("name", "") for step in steps]
    assert names.index("Require a complete wheel before upload") < names.index(
        "Publish linux-x86_64 to PyPI"
    )
    assert set(jobs["verify-pypi"]["needs"]) == {
        "build",
        "publish-x86",
        "build-sdist",
    }
    source_steps = jobs["build-sdist"]["steps"]
    assert any(
        "xtask sdist-completeness" in step.get("run", "") for step in source_steps
    )
    assert not (REPO / ".github" / "workflows" / "pipeline.yml").exists()
    # The two-workflow redesign folded the former release-build.yml (this
    # test's wheel-build workflow, pre-rename) and rust-ci.yml into
    # release.yml/advisory.yml and deleted both. A resurrected
    # release-build.yml would be exactly the kind of unguarded fallback
    # release path this test exists to rule out.
    assert not (REPO / ".github" / "workflows" / "release-build.yml").exists()
    assert not (REPO / ".github" / "workflows" / "rust-ci.yml").exists()


def _runner_labels(job: dict) -> list[str]:
    """Every runner label a job can resolve to, across its matrix legs."""

    runs_on = job.get("runs-on", [])
    if isinstance(runs_on, str) and "matrix.runner" in runs_on:
        legs = job["strategy"]["matrix"]["include"]
        values = [leg["runner"] for leg in legs]
    else:
        values = [runs_on]
    labels: list[str] = []
    for value in values:
        labels.extend(value if isinstance(value, list) else [value])
    return labels


def test_self_hosted_runners_are_unreachable_from_pull_requests() -> None:
    """Only a tag push or a manual dispatch in this repository reaches the
    project's own runners; a pull request, including one from a fork, never
    runs code on them."""

    for workflow in sorted((REPO / ".github" / "workflows").glob("*.yml")):
        doc = yaml.safe_load(workflow.read_text(encoding="utf-8"))
        for name, job in doc["jobs"].items():
            if "self-hosted" not in _runner_labels(job):
                continue
            condition = " ".join(str(job.get("if", "")).split())
            assert "github.repository == 'Knuckles-Team/medulla'" in condition, (
                f"{workflow.name}:{name}"
            )
            assert "pull_request" not in condition, f"{workflow.name}:{name}"
            assert "startsWith(github.ref, 'refs/tags/v')" in condition, (
                f"{workflow.name}:{name}"
            )


@pytest.mark.spec("MEDULLA-CONTRACT-R056.12")
def test_runner_jobs_keep_build_budget_and_bounded_authorization() -> None:
    for workflow in sorted((REPO / ".github" / "workflows").glob("*.yml")):
        doc = yaml.safe_load(workflow.read_text(encoding="utf-8"))
        for name, job in doc["jobs"].items():
            _assert_workflow_job_budget(workflow, name, job)


def _assert_runner_budget(workflow: Path, name: str, job: dict) -> None:
    if "uses" in job:  # reusable-workflow call: GitHub rejects a timeout here
        return
    if (workflow.name, name) == ("medulla-release-x86.yml", "authorize"):
        assert job == _verified_x86_workflow()["jobs"]["authorize"]
        assert job["runs-on"] == "ubuntu-latest"
        assert job.get("timeout-minutes") == 5
    elif (workflow.name, name) == ("arm64-linker-probe.yml", "probe"):
        # This compiles tiny fixtures, never the full release workspace.
        assert job["runs-on"] == "ubuntu-24.04-arm"
        assert job.get("timeout-minutes") == 10
    elif (workflow.name, name) in {
        ("nightly-decide-budgets.yml", "decide-budgets"),
        ("nightly-finance-budgets.yml", "finance-budgets"),
    }:
        # Main-only nightly release-profile benchmarks: one bench build plus
        # the measured run, never a pull request.
        assert job["if"] == "github.ref == 'refs/heads/main'"
        assert job.get("timeout-minutes") == 180
    elif (workflow.name, name) == ("macos-release-probe.yml", "probe"):
        assert job["runs-on"] == "${{ matrix.runner }}"
        assert job.get("timeout-minutes") == 15
        assert {row["runner"] for row in job["strategy"]["matrix"]["include"]} == {
            "macos-15",
        }
    else:
        # Native compilation jobs retain the longer PR build budget, including
        # the independent canonical generator. Classification/lint stay short.
        # python-server/python-suite build/run the full engine suite and exceed
        # the short budget on cold hosted PR runners.
        if workflow.name in {"ci.yml", "release.yml"} and name in {
            "gates",
            "gates-wheel",
            "gates-facade",
            "gates-whisper",
            "gates-plan-fuzz",
            "python-server",
            "python-suite",
            "no-auth-uds",
            "gates-pyo3",
            "clippy-release-profiles",
            "ci",
        }:
            expected = {
                ("ci.yml", "gates-facade"): 60,
                ("ci.yml", "clippy-release-profiles"): 60,
                (
                    "ci.yml",
                    "ci",
                ): "${{ github.event_name == 'pull_request' && 180 || 180 }}",
            }.get(
                (workflow.name, name),
                "${{ github.event_name == 'pull_request' && 90 || 90 }}",
            )
            assert job.get("timeout-minutes") == expected
        else:
            _assert_short_runner_budget(workflow, name, job)


def _assert_short_runner_budget(workflow: Path, name: str, job: dict) -> None:
    thirty_min = "${{ github.event_name == 'pull_request' && 30 || 60 }}"
    # The full workspace sweep builds and runs every one of the ~150 crates'
    # tests with all features on a cold hosted runner; it needs a push budget
    # beyond the short one (pull requests keep the 15-minute affected subset).
    workspace_sweep = "${{ github.event_name == 'pull_request' && 15 || 240 }}"
    pr_scoped = {
        ("ci.yml", "quality-advisory"): thirty_min,
        ("ci.yml", "gates-crates"): workspace_sweep,
        ("ci.yml", "gates-storage"): thirty_min,
        ("ci.yml", "gates-ontology"): thirty_min,
    }.get(
        (workflow.name, name), "${{ github.event_name == 'pull_request' && 10 || 60 }}"
    )
    if (workflow.name, name) in {
        ("ci.yml", "quality-advisory"),
        ("ci.yml", "gates-crates"),
        ("ci.yml", "gates-storage"),
        ("ci.yml", "gates-ontology"),
    }:
        assert job.get("timeout-minutes") == pr_scoped
    else:
        assert job.get("timeout-minutes") in (360, 5, pr_scoped), (
            f"{workflow.name}:{name}"
        )


@pytest.mark.parametrize(
    "timeout",
    [10, 15, 45, 60, 90, 360, "${{ github.event_name == 'pull_request' && 90 || 90 }}"],
)
def test_workspace_clippy_budget_rejects_unscoped_limits(timeout: int | str) -> None:
    """MEDULLA-CONTRACT-R031: only the workspace matrix leg receives extra time."""
    with pytest.raises(AssertionError):
        _assert_runner_budget(
            CI_WORKFLOW, "quality-advisory", {"timeout-minutes": timeout}
        )


@pytest.mark.parametrize(
    "workflow,name",
    [
        ("release.yml", "quality-advisory"),
        ("ci.yml", "security"),
        ("ci.yml", "lint-and-architecture"),
    ],
)
def test_workspace_clippy_budget_cannot_expand_other_jobs(
    workflow: str, name: str
) -> None:
    """MEDULLA-CONTRACT-R031: the matrix budget is not a general runner exception."""
    budget = "${{ github.event_name == 'pull_request' && 30 || 60 }}"
    with pytest.raises(AssertionError):
        _assert_runner_budget(Path(workflow), name, {"timeout-minutes": budget})


def _assert_workflow_job_budget(workflow: Path, name: str, job: dict) -> None:
    if (workflow.name, name) == ("release-publication-recovery.yml", "recover"):
        # This verifies and publishes existing wheels; it never builds them.
        assert job["runs-on"] == "ubuntu-latest"
        assert job.get("timeout-minutes") == 30
        assert "strategy" not in job
        assert job["env"]["SOURCE_COMMIT"] == (
            "7f5179650d87e6f763b129cfeea6dfb4f8b0273b"
        )
        assert any(
            step.get("run") == "python scripts/release_publication_recovery.py prepare"
            for step in job["steps"]
        )
    else:
        _assert_runner_budget(workflow, name, job)


def test_release_concurrency_never_cancels_a_publication() -> None:
    concurrency = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))["concurrency"]
    assert concurrency["group"] == "release-${{ github.ref }}"
    # Keep GitHub's bounded default: one running plus the latest pending run.
    assert concurrency.get("queue", "single") == "single"
    assert concurrency["cancel-in-progress"] is False


def test_ci_concurrency_supersedes_older_runs_per_pr_only() -> None:
    """spec MEDULLA-CONTRACT-R053.7: a PR push cancels the older run; main finishes."""
    doc = yaml.safe_load(CI_WORKFLOW.read_text(encoding="utf-8"))
    concurrency = doc["concurrency"]
    assert (
        concurrency["cancel-in-progress"]
        == "${{ github.event_name == 'pull_request' }}"
    )
    group = concurrency["group"]
    assert "github.workflow" in group
    assert "github.event.pull_request.number" in group
    assert "github.ref" in group


def test_sdist_backend_imports_from_declared_isolated_source_tree(tmp_path):
    """The in-tree PEP 517 backend must import from the files shipped in sdist.

    This deliberately imports only the backend and its Python helper closure;
    it does not invoke maturin or compile native code.  Keeping the source tree
    limited to the explicit sdist entries catches a missing transitive helper
    before an isolated build reaches maturin.
    """
    project = tomllib.loads((REPO / "pyproject.toml").read_text(encoding="utf-8"))
    includes = {
        entry["path"]
        for entry in project["tool"]["maturin"]["include"]
        if isinstance(entry, dict) and entry.get("format") == "sdist"
    }
    closure = {"build_backend.py"}
    pending = ["build_backend.py"]
    while pending:
        for line in (REPO / pending.pop()).read_text(encoding="utf-8").splitlines():
            match = re.match(r"\s*from scripts\.(\w+) import", line)
            if match and f"scripts/{match[1]}.py" not in closure:
                closure.add(f"scripts/{match[1]}.py")
                pending.append(f"scripts/{match[1]}.py")
    assert closure <= includes

    isolated = tmp_path / "sdist"
    for relative in sorted(closure):
        source = REPO / relative
        destination = isolated / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)

    environment = os.environ.copy()
    environment.pop("PYTHONPATH", None)
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "import build_backend; print(build_backend.PROJECT_NAME)",
        ],
        cwd=isolated,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "medulla"


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
def test_server_build_adds_swap_and_thin_links_aarch64():
    # Swap and the ARM64/macOS link profile live in the shared wheel pass and the
    # server build script, so the release legs and ci.yml's warm-cache build both
    # get them (a cache job without swap was OOM-killed on main).
    steps = yaml.safe_load(WHEEL_PASS_ACTION.read_text(encoding="utf-8"))["runs"][
        "steps"
    ]
    names = [step.get("name", "") for step in steps]
    assert names.index("Add swap for the server link") < names.index(
        "Build engine extension wheel"
    )
    script = (REPO / "scripts/build_release_server.sh").read_text(encoding="utf-8")
    assert "aarch64-unknown-linux-gnu | *-apple-darwin)" in script
    assert "export CARGO_PROFILE_RELEASE_SERVER_LTO=thin" in script
    assert "export CARGO_PROFILE_RELEASE_SERVER_CODEGEN_UNITS=16" in script
    release = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))["jobs"]
    assert "build-server" not in release


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7")
def test_tag_pipeline_builds_checks_and_publishes_only() -> None:
    """A tag publishes only a commit main CI already passed (spec R053.7)."""
    doc = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    triggers = doc[True] if True in doc else doc["on"]
    assert set(triggers) == {"push", "workflow_dispatch"}
    assert triggers["push"] == {"tags": ["v*"]}
    assert triggers["workflow_dispatch"]["inputs"]["force_untested"]["default"] is False
    jobs = doc["jobs"]
    assert set(jobs) == {
        "guard",
        "method-codec",
        "server-binary",
        "build",
        "build-x86",
        "build-sdist",
        "publish-x86",
        "server-image",
        "server-manifest",
        "verify-pypi",
        "github-release",
    }
    for name in ("method-codec", "server-binary", "build-x86", "build-sdist"):
        assert jobs[name]["needs"] == "guard"
    assert jobs["build"]["needs"] == ["guard", "server-binary"]
    guard = jobs["guard"]
    assert guard["permissions"]["actions"] == "read"
    script = "\n".join(step.get("run", "") for step in guard["steps"])
    assert "actions/workflows/ci.yml/runs?head_sha=${SHA}&status=success" in script
    assert "force_untested" in str(guard["steps"])
    assert "REFUSING to publish" in script
    # Every gate, feature build and benchmark lives in ci.yml.
    ci = yaml.safe_load(CI_WORKFLOW.read_text(encoding="utf-8"))
    ci_triggers = ci[True] if True in ci else ci["on"]
    assert set(ci_triggers) == {
        "push",
        "pull_request",
        "schedule",
        "workflow_dispatch",
    }
    assert ci_triggers["push"] == {"branches": ["main"]}
    assert {"gates", "feature-matrix", "benchmarks", "engine-main-wheel"} <= set(
        ci["jobs"]
    )
    assert not {"build", "build-x86", "build-sdist", "publish-pypi"} & set(ci["jobs"])


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
def test_every_rust_cache_saves_the_configured_target_dir():
    # `.cargo/config.toml` sends every build to its `target-dir` unless the file
    # exports its own CARGO_TARGET_DIR; a rust-cache entry naming any other
    # directory saves no compiled crates, and each run rebuilds the engine.
    config = tomllib.loads((REPO / ".cargo/config.toml").read_text(encoding="utf-8"))
    target_dir = config["build"]["target-dir"]
    mappings = []
    for path in sorted((REPO / ".github").rglob("*.yml")):
        text = path.read_text(encoding="utf-8")
        exported = [
            line.split("CARGO_TARGET_DIR=", 1)[1]
            for line in text.splitlines()
            if "CARGO_TARGET_DIR=" in line
        ]
        for line in text.splitlines():
            if "workspaces:" in line and "->" in line:
                mapped = line.split("->", 1)[1].strip().strip("\"'")
                own = any(mapped.rsplit("/", 1)[-1] in value for value in exported)
                mappings.append((path.name, mapped, mapped == target_dir or own))
    assert mappings
    wrong = [(name, mapped) for name, mapped, ok in mappings if not ok]
    assert not wrong, wrong


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
def test_no_crate_forbids_the_unsafe_code_it_allows():
    # `forbid` cannot be overridden, so a scoped `#[allow(unsafe_code)]` in a crate
    # with `#![forbid(unsafe_code)]` fails to compile, but only on the platform whose
    # `cfg` enables it (the Windows server wheel of 3.0.0 failed this way).
    conflicts = []
    for crate in sorted((REPO / "crates").iterdir()):
        roots = [crate / "src" / "lib.rs", crate / "src" / "main.rs"]
        root_text = "".join(r.read_text(encoding="utf-8") for r in roots if r.exists())
        if "#![forbid(unsafe_code)]" not in root_text:
            continue
        allows = [
            str(path.relative_to(REPO))
            for path in sorted((crate / "src").rglob("*.rs"))
            if "#[allow(unsafe_code)]" in path.read_text(encoding="utf-8")
        ]
        conflicts.extend(allows)
    assert not conflicts, conflicts


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
def test_server_links_no_opendal_startup_constructor():
    # opendal's `auto-register-services` adds a `ctor` constructor whose branch into
    # the ~280 MB server binary is out of arm64 range, so the macOS server link
    # failed in 3.0.0. The vendored iceberg-storage-opendal drops that feature.
    lock = tomllib.loads((REPO / "Cargo.lock").read_text(encoding="utf-8"))
    opendal = [p for p in lock["package"] if p["name"] == "opendal"]
    assert opendal
    assert all("ctor" not in p.get("dependencies", []) for p in opendal)
    manifest = tomllib.loads((REPO / "Cargo.toml").read_text(encoding="utf-8"))
    assert manifest["patch"]["crates-io"]["iceberg-storage-opendal"]["path"].startswith(
        "vendor/security-patches/"
    )


@pytest.mark.spec("MEDULLA-CONTRACT-R053.9")
def test_platform_only_code_declares_its_platform_crates():
    # `#[cfg(windows)]` code compiles only on the Windows wheel leg, so a crate that
    # uses a platform crate without declaring it builds everywhere else and fails
    # there (medulla-wire's pgwire TLS open after the crate split).
    missing = []
    for crate in sorted((REPO / "crates").iterdir()):
        manifest = crate / "Cargo.toml"
        if not manifest.exists():
            continue
        declared = manifest.read_text(encoding="utf-8")
        used = "".join(
            path.read_text(encoding="utf-8")
            for path in sorted((crate / "src").rglob("*.rs"))
        )
        for module, package in (
            ("windows_sys::", "windows-sys"),
            ("rustix::", "rustix"),
        ):
            if module in used and f"{package} =" not in declared:
                missing.append((crate.name, package))
    assert not missing, missing
