"""Rust-only method codec builds retain byte and import verification."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.no_engine
WASM = b"\x00asm\x01\x00\x00\x00"


@pytest.fixture
def codec(monkeypatch):
    path = ROOT / "scripts" / "build_method_codec_wasm.py"
    spec = importlib.util.spec_from_file_location("build_method_codec_wasm", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, "build_method_codec_wasm", module)
    spec.loader.exec_module(module)
    return module


def test_build_runs_cargo_then_the_pinned_wasm_opt(codec, monkeypatch, tmp_path):
    target = tmp_path / "target"
    module_path = target / codec.TARGET / codec.PROFILE / codec.MODULE
    module_path.parent.mkdir(parents=True)
    module_path.write_bytes(WASM + b"\x00\x01x")
    calls = []

    def cargo(command, *, cwd, env, check):
        calls.append((command, cwd, env, check))

    monkeypatch.setattr(codec.subprocess, "run", cargo)
    monkeypatch.setattr(
        codec, "encoded_rustflags", lambda *_args, **_kwargs: ("flags", (), ())
    )
    monkeypatch.setenv("RUSTFLAGS", "-C target-cpu=native")
    optimized = WASM + b"\x00\x01y"
    monkeypatch.setattr(
        codec,
        "optimize",
        lambda module, tdir: (
            optimized if (module, tdir) == (module_path, target) else b""
        ),
    )
    assert codec.build(tmp_path, target) == optimized
    assert len(calls) == 1
    command, cwd, env, check = calls[0]
    assert command[:2] == ["cargo", "build"]
    assert "--locked" in command and "medulla-method-codec" in command
    assert cwd == tmp_path and check is True
    assert env["CARGO_TARGET_DIR"] == str(target)
    assert env["CARGO_ENCODED_RUSTFLAGS"] == "flags"
    assert "RUSTFLAGS" not in env


def test_optimize_runs_wasm_opt_oz_on_the_linked_module(codec, monkeypatch, tmp_path):
    module = tmp_path / "m.wasm"
    module.write_bytes(WASM)
    tool = tmp_path / "wasm-opt"
    calls = []

    def run(command, *, check):
        calls.append(command)
        Path(command[-1]).write_bytes(WASM + b"\x00\x01z")

    monkeypatch.setattr(codec, "wasm_opt", lambda _target: tool)
    monkeypatch.setattr(codec.subprocess, "run", run)
    assert codec.optimize(module, tmp_path) == WASM + b"\x00\x01z"
    (command,) = calls
    assert command[0] == str(tool) and "-Oz" in command
    assert command[-3:] == [str(module), "-o", str(module.with_suffix(".opt.wasm"))]


def test_wasm_opt_refuses_an_archive_with_the_wrong_digest(
    codec, monkeypatch, tmp_path
):
    class Response:
        def __enter__(self):
            return self

        def __exit__(self, *_exc):
            return False

        def read(self):
            return b"tampered"

    monkeypatch.setattr(codec.platform, "system", lambda: "Linux")
    monkeypatch.setattr(codec.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(codec.urllib.request, "urlopen", lambda *_a, **_k: Response())
    with pytest.raises(RuntimeError, match="sha256"):
        codec.wasm_opt(tmp_path)
    assert not list(tmp_path.rglob("wasm-opt"))


@pytest.mark.spec("MEDULLA-CONTRACT-R006")
def test_wasm_opt_returns_the_binary_the_pinned_archive_unpacks(
    codec, monkeypatch, tmp_path
):
    import io
    import tarfile

    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as bundle:
        member = tarfile.TarInfo(f"binaryen-{codec.BINARYEN_VERSION}/bin/wasm-opt")
        member.size = 2
        member.mode = 0o755
        bundle.addfile(member, io.BytesIO(b"#!"))
    archive = buffer.getvalue()

    class Response:
        def __enter__(self):
            return self

        def __exit__(self, *_exc):
            return False

        def read(self):
            return archive

    monkeypatch.setattr(codec.platform, "system", lambda: "Linux")
    monkeypatch.setattr(codec.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(codec, "BINARYEN_SHA256", hashlib.sha256(archive).hexdigest())
    monkeypatch.setattr(codec.urllib.request, "urlopen", lambda *_a, **_k: Response())
    binary = codec.wasm_opt(tmp_path)
    assert binary.is_file()
    assert codec.wasm_opt(tmp_path) == binary


@pytest.mark.parametrize(
    "module",
    [
        b"not wasm",
        WASM + b"\x02\x01\x00",  # even an empty import section is refused
        WASM + b"\x00\x02x",  # truncated custom section
        WASM + b"\x00\x80",  # truncated section size
    ],
)
def test_invalid_or_importing_modules_fail_closed(codec, module):
    with pytest.raises(ValueError, match="WebAssembly|imports"):
        codec.verify_no_imports(module)


@pytest.fixture
def codec_repo(codec, tmp_path):
    for client in codec.CLIENT_DIRS:
        (tmp_path / client).mkdir(parents=True)
    return tmp_path, None


# spec: MEDULLA-CONTRACT-R006.2
@pytest.mark.spec("MEDULLA-CONTRACT-R006.2")
def test_write_requires_two_identical_builds_within_budget(codec, codec_repo, capsys):
    root, _ = codec_repo
    fresh = WASM + b"\x00\x01q"
    assert codec.write(root, fresh, WASM + b"\x00\x01r") == 1
    assert "two canonical builds differ" in capsys.readouterr().err
    for client in codec.CLIENT_DIRS:
        assert not (root / client / codec.MODULE).exists()
    assert codec.write(root, fresh, fresh) == 0
    digest = hashlib.sha256(fresh).hexdigest()
    assert f"sha256={digest}" in capsys.readouterr().out
    for client in codec.CLIENT_DIRS:
        assert (root / client / codec.MODULE).read_bytes() == fresh


def test_write_refuses_an_over_budget_module(codec, codec_repo, monkeypatch, capsys):
    root, _ = codec_repo
    monkeypatch.setattr(codec, "MAX_BYTES", 4)
    assert codec.write(root, WASM, WASM) == 1
    assert "over MAX_BYTES" in capsys.readouterr().err
    for client in codec.CLIENT_DIRS:
        assert not (root / client / codec.MODULE).exists()


def test_write_refuses_symlinks_before_changing_files(codec, codec_repo, tmp_path):
    root, _ = codec_repo
    outside = tmp_path / "outside.wasm"
    outside.write_bytes(b"keep")
    (root / codec.CLIENT_DIRS[1] / codec.MODULE).symlink_to(outside)
    assert codec.write(root, WASM, WASM) == 1
    assert outside.read_bytes() == b"keep"
    assert not (root / codec.CLIENT_DIRS[0] / codec.MODULE).exists()


def test_write_refuses_invalid_module_without_changing_files(codec, codec_repo):
    root, _ = codec_repo
    with pytest.raises(ValueError, match="WebAssembly"):
        codec.write(root, b"invalid", b"invalid")
    for client in codec.CLIENT_DIRS:
        assert not (root / client / codec.MODULE).exists()


# spec: MEDULLA-CONTRACT-R006
@pytest.mark.spec("MEDULLA-CONTRACT-R006")
def test_check_compares_two_builds_and_budget(codec, codec_repo, monkeypatch, capsys):
    root, _ = codec_repo
    monkeypatch.setattr(codec, "ROOT", root)
    fresh = WASM + b"\x00\x01q"
    monkeypatch.setattr(codec, "build", lambda *_args: fresh)
    assert codec.main(["--check"]) == 0
    assert "OK:" in capsys.readouterr().out
    for client in codec.CLIENT_DIRS:
        assert not (root / client / codec.MODULE).exists()
    assert not (root / codec.PROVENANCE).exists()

    def drifting(_root, target_dir):
        return fresh + b"z" if "requalify" in str(target_dir) else fresh

    monkeypatch.setattr(codec, "build", drifting)
    assert codec.main(["--check"]) == 1
    assert "two canonical builds differ" in capsys.readouterr().err
    for client in codec.CLIENT_DIRS:
        assert not (root / client / codec.MODULE).exists()

    monkeypatch.setattr(codec, "build", lambda *_args: fresh)
    monkeypatch.setattr(codec, "MAX_BYTES", 4)
    assert codec.main(["--check"]) == 1
    assert "over MAX_BYTES" in capsys.readouterr().err
    for client in codec.CLIENT_DIRS:
        assert not (root / client / codec.MODULE).exists()


@pytest.fixture
def export_repo(codec, codec_repo, monkeypatch):
    root, _ = codec_repo
    for name in (
        "Cargo.lock",
        "Cargo.toml",
        "rust-toolchain.toml",
        "scripts/build_method_codec_wasm.py",
        "scripts/configure_rust_path_remap.py",
    ):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes((ROOT / name).read_bytes())
    monkeypatch.setattr(codec, "ROOT", root)

    def provenance_command(command, **_kwargs):
        if command[0] == "rustc":
            return "rustc pinned test version\n"
        return {"HEAD": "a" * 40, "HEAD^{tree}": "b" * 40, "--untracked-files=no": ""}[
            command[-1]
        ]

    monkeypatch.setattr(codec.subprocess, "check_output", provenance_command)
    output = root / "step-output"
    monkeypatch.setenv("GITHUB_OUTPUT", str(output))
    return root, root / "target" / "canonical-method-codec", output


@pytest.mark.spec("MEDULLA-CONTRACT-R006.1")
def test_export_records_provenance_and_places_client_copies(
    codec, export_repo, monkeypatch
):
    root, directory, output = export_repo
    fresh = WASM + b"\x00\x01x"
    monkeypatch.setattr(codec, "build", lambda *_args: fresh)
    assert codec.main(["--export-dir", str(directory)]) == 0
    assert (directory / codec.MODULE).read_bytes() == fresh
    provenance = json.loads((directory / codec.PROVENANCE).read_text())
    assert provenance["sha256"] == hashlib.sha256(fresh).hexdigest()
    assert provenance["size_bytes"] == len(fresh)
    assert provenance["source_revision"] == "a" * 40
    assert provenance["source_tree"] == "b" * 40
    assert provenance["source_dirty"] is False
    assert provenance["build_command"] == list(codec.BUILD_COMMAND)
    assert provenance["target"] == codec.TARGET
    assert provenance["profile"] == codec.PROFILE
    assert provenance["rustc_version"] == "rustc pinned test version"
    assert (
        provenance["recipe_sha256"]["Cargo.lock"]
        == hashlib.sha256((root / "Cargo.lock").read_bytes()).hexdigest()
    )
    assert output.read_text() == "fresh_export=true\n"
    for client in codec.CLIENT_DIRS:
        assert (root / client / codec.MODULE).read_bytes() == fresh


@pytest.mark.spec("MEDULLA-CONTRACT-R006.1")
def test_failed_build_never_exports_cached_module(codec, export_repo, monkeypatch):
    root, directory, output = export_repo
    directory.mkdir(parents=True)
    for name in (codec.MODULE, codec.PROVENANCE):
        (directory / name).write_bytes(b"stale previous export")
    cached = root / "target" / codec.TARGET / codec.PROFILE / codec.MODULE
    cached.parent.mkdir(parents=True)
    cached.write_bytes(WASM)

    def fail(*_args, **_kwargs):
        raise codec.subprocess.CalledProcessError(1, ["cargo", "build"])

    monkeypatch.setattr(codec.subprocess, "run", fail)
    with pytest.raises(codec.subprocess.CalledProcessError):
        codec.main(["--export-dir", str(directory)])
    assert list(directory.iterdir()) == []
    assert not output.exists()
    assert cached.read_bytes() == WASM


@pytest.mark.spec("MEDULLA-CONTRACT-R006.1")
def test_invalid_fresh_module_has_no_export_signal(codec, export_repo, monkeypatch):
    _, directory, output = export_repo
    monkeypatch.setattr(codec, "build", lambda *_args: b"invalid")
    with pytest.raises(ValueError, match="WebAssembly"):
        codec.main(["--export-dir", str(directory)])
    assert not output.exists()
    assert not (directory / codec.PROVENANCE).exists()


@pytest.mark.spec("MEDULLA-CONTRACT-R006.1")
def test_failed_provenance_has_no_export_signal(codec, export_repo, monkeypatch):
    _, directory, output = export_repo
    monkeypatch.setattr(codec, "build", lambda *_args: WASM)

    def fail(*_args, **_kwargs):
        raise codec.subprocess.CalledProcessError(1, ["git", "rev-parse"])

    monkeypatch.setattr(codec.subprocess, "check_output", fail)
    with pytest.raises(codec.subprocess.CalledProcessError):
        codec.main(["--export-dir", str(directory)])
    assert not output.exists()
    assert not (directory / codec.PROVENANCE).exists()


@pytest.mark.spec("MEDULLA-CONTRACT-R006.1")
def test_ci_builds_the_codec_before_client_tests_and_uploads_it():
    import yaml

    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
    steps = workflow["jobs"]["language-clients"]["steps"]
    build = next(step for step in steps if step.get("id") == "method-codec")
    assert "--export-dir target/canonical-method-codec" in build["run"]
    assert "--check" not in build["run"]
    assert not build.get("continue-on-error", False)
    upload = steps[steps.index(build) + 1]
    assert "!cancelled()" in upload["if"]
    assert "steps.method-codec.outputs.fresh_export == 'true'" in upload["if"]
    assert (
        upload["uses"]
        == "actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9"
    )
    assert upload["with"]["if-no-files-found"] == "error"
    names = [step.get("name", "") for step in steps]
    first_client_test = min(i for i, name in enumerate(names) if "client tests" in name)
    assert steps.index(build) < first_client_test
