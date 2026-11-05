"""Pull-request affected-crate selection (spec MEDULLA-CONTRACT-R053.7)."""

from __future__ import annotations

import importlib.util
import pathlib

import pytest
import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "ci_affected_crates", ROOT / "scripts" / "ci_affected_crates.py"
)
assert _spec is not None and _spec.loader is not None
mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(mod)


def _meta() -> dict:
    def pkg(name, path, deps=()):
        return {
            "id": name,
            "name": name,
            "manifest_path": f"/ws/{path}Cargo.toml",
            "dependencies": [{"name": d, "path": "/x"} for d in deps]
            + [{"name": "serde"}],
        }

    pkgs = [
        pkg("medulla-types", "crates/medulla-types/"),
        pkg("medulla-storage", "crates/medulla-storage/", ["medulla-types"]),
        pkg("medulla-viz", "crates/medulla-viz/"),
        pkg("medulla-numeric", "crates/medulla-numeric/", ["medulla-types"]),
        pkg("facade", "", ["medulla-storage", "medulla-viz", "medulla-numeric"]),
    ]
    return {
        "workspace_root": "/ws",
        "packages": pkgs,
        "workspace_members": [p["id"] for p in pkgs],
    }


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7")
@pytest.mark.parametrize(
    ("changed", "expected"),
    [
        (["docs/a.md", "specs/x/requirements.md", "tests/test_a.py"], []),
        (["crates/medulla-viz/src/lib.rs"], ["facade", "medulla-viz"]),
        (
            ["crates/medulla-types/src/lib.rs"],
            ["facade", "medulla-numeric", "medulla-storage", "medulla-types"],
        ),
        (["src/main.rs"], ["facade"]),
        (
            ["Cargo.lock"],
            [
                "facade",
                "medulla-numeric",
                "medulla-storage",
                "medulla-types",
                "medulla-viz",
            ],
        ),
        (
            ["Cargo.toml"],
            [
                "facade",
                "medulla-numeric",
                "medulla-storage",
                "medulla-types",
                "medulla-viz",
            ],
        ),
    ],
)
def test_affected_is_owners_plus_reverse_dependencies(changed, expected) -> None:
    assert mod.affected(_meta(), changed) == expected


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7")
def test_runs_keep_main_feature_profiles() -> None:
    runs = mod.commands(
        _meta(), ["facade", "medulla-numeric", "medulla-storage", "medulla-types"]
    )
    assert runs == [
        "{tool} -p medulla-types --all-features",
        "{tool} -p medulla-storage --all-features {serial}",
        "{tool} -p medulla-numeric --features "
        + mod.pyo3_features(["medulla-numeric"]),
        "{tool} -p facade --features full",
    ]
    assert mod.commands(_meta(), []) == []


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7")
def test_pull_request_runs_one_gate() -> None:
    ci = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8"))
    jobs = ci["jobs"]
    assert jobs["ci"]["name"] == "CI"
    assert jobs["ci"]["if"] == "github.event_name == 'pull_request'"
    assert "ci" in jobs["ci-required"]["needs"]
    for name in ("light-gates", "changes"):
        assert jobs[name]["if"] == "github.event_name != 'pull_request'"
    assert "pull_request" in jobs["security"]["if"]
    script = "\n".join(s.get("run", "") for s in jobs["ci"]["steps"])
    assert "ci_affected_crates.py" in script
    assert "--from-ref" in script and "spec-trailers" in script
    assert "cargo nextest run" in script and "cargo clippy" in script
    runs_on = jobs["ci"]["runs-on"]
    for needle in (
        "MEDULLA_WARM_CI_RUNNER",
        "head.repo.full_name",
        "medulla-warm",
        "dependabot[bot]",
        "'ubuntu-24.04'",
    ):
        assert needle in runs_on
    assert "arbora-ci-trusted" not in runs_on
    assert "SCCACHE_GHA_ENABLED=false" in script and "CARGO_TARGET_DIR=" in script


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7")
def test_own_job_packages_are_left_to_their_own_job() -> None:
    runs = mod.commands(_meta(), ["medulla-types", "medulla-tts-piper"])
    assert runs == ["{tool} -p medulla-types --all-features"]
    assert mod.commands(_meta(), ["medulla-tts-piper"]) == []


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7")
def test_pyo3_features_name_only_selected_packages() -> None:
    features = mod.pyo3_features(["medulla-pyengine"]).split(",")
    assert features and all(f.startswith("medulla-pyengine/") for f in features)
    both = mod.pyo3_features(["medulla-numeric", "medulla-pyengine"])
    assert both == mod.PYO3_FEATURES


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7.1")
def test_spec_file_read_by_a_test_selects_its_package(tmp_path, monkeypatch) -> None:
    """A changed ``specs/`` file selects every package whose sources read it."""
    reader = tmp_path / "crates" / "medulla-types" / "tests"
    reader.mkdir(parents=True)
    (reader / "engine_capabilities.rs").write_text(
        'let plan = include_str!("../../../specs/engine-parity/plan.md");\n',
        encoding="utf-8",
    )
    other = tmp_path / "crates" / "medulla-viz" / "src"
    other.mkdir(parents=True)
    (other / "lib.rs").write_text("// specs/engine-parity-other\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)
    mod._sources.cache_clear()
    got = mod.affected(_meta(), ["specs/engine-parity/plan.md"])
    assert "medulla-types" in got
    assert "medulla-viz" not in got
    assert mod.affected(_meta(), ["specs/unread/plan.md"]) == []
    mod._sources.cache_clear()


@pytest.mark.spec("MEDULLA-CONTRACT-R053.7.1")
def test_pr_spec_only_decision_uses_the_crate_selection() -> None:
    ci = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8"))
    for job in ("pr-scope", "ci"):
        script = "\n".join(s.get("run", "") for s in ci["jobs"][job]["steps"])
        assert "ci_affected_crates.py" in script
    steps = {s.get("id"): s for s in ci["jobs"]["ci"]["steps"]}
    assert "spec-scope" in steps
    assert "steps.spec-scope.outputs.scope" in steps["crates"]["if"]
