"""Static wheel suite selection does not start the shared engine (conftest contract).

The wheel completeness gate itself is `cargo xtask wheel-completeness`, with its tests
in `tools/xtask`; this module keeps the two fixture-selection proofs from beside it.
"""

from __future__ import annotations

import inspect
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

pytestmark = pytest.mark.no_engine


def test_static_wheel_suite_selection_does_not_start_engine(
    request, start_medulla_server
) -> None:
    """The complete static module is selected as no-engine by the real fixture.

    Run on its own, this module never starts the shared engine. Run inside a
    larger session, the fixture starts it exactly when some OTHER selected
    test needs it -- the one predicate both decisions come from.
    """

    import conftest

    module_path = Path(__file__).resolve()
    selected = [
        item
        for item in request.session.items
        if Path(str(item.fspath)).resolve() == module_path
    ]
    assert selected
    assert all(item.get_closest_marker("no_engine") is not None for item in selected)
    assert not conftest.selection_needs_engine(selected)
    session_needs_engine = conftest.selection_needs_engine(request.session.items)
    assert (start_medulla_server is not None) is session_needs_engine


def test_engine_selected_control_still_enters_startup(
    monkeypatch, tmp_path: Path
) -> None:
    """The no-engine marker must not weaken the real-engine selection branch."""

    import conftest

    events: list[str] = []

    class _Item:
        def get_closest_marker(self, name: str) -> object | None:
            return object() if name == "engine" else None

    class _Request:
        session = SimpleNamespace(items=[_Item()])

    class _TmpPathFactory:
        def mktemp(self, name: str) -> Path:
            path = tmp_path / name
            path.mkdir()
            return path

    class _Process:
        def poll(self) -> None:
            return None

        def terminate(self) -> None:
            events.append("terminate")

        def wait(self, timeout: float | None = None) -> None:
            events.append("wait")

    class _Bootstrap:
        consensus = None

        def __init__(self) -> None:
            self.consensus = self

        def bootstrap_system_identity(self, **_kwargs: object) -> None:
            events.append("bootstrap")

        def close(self) -> None:
            events.append("close")

    class _Client:
        @staticmethod
        def connect(**_kwargs: object) -> _Bootstrap:
            events.append("connect")
            return _Bootstrap()

    process = _Process()

    def _run(*_args: object, **_kwargs: object) -> None:
        events.append("build")

    def _popen(*_args: object, **_kwargs: object) -> _Process:
        events.append("start")
        return process

    monkeypatch.setattr(conftest, "SyncMedullaClient", _Client)
    monkeypatch.setattr(conftest, "_prebuilt_test_binary", lambda: None)
    monkeypatch.setattr(
        conftest,
        "subprocess",
        SimpleNamespace(
            PIPE=object(),
            run=_run,
            Popen=_popen,
            TimeoutExpired=subprocess.TimeoutExpired,
        ),
    )
    monkeypatch.setattr(conftest, "_socket_accepts", lambda _path: True)
    monkeypatch.setattr(conftest.os.path, "exists", lambda _path: True)
    monkeypatch.setattr(conftest.os, "remove", lambda _path: None)

    previous_environment = dict(os.environ)
    try:
        # `inspect.unwrap` (not a direct `.__wrapped__` attribute access, which
        # pytest's `FixtureFunctionDefinition` stub does not declare even
        # though `functools.wraps` sets it at runtime) reaches the fixture's
        # own undecorated generator function.
        fixture = inspect.unwrap(conftest.start_medulla_server)
        generator = fixture(_Request(), _TmpPathFactory())
        assert next(generator) is process
        assert events == ["build", "start", "connect", "bootstrap", "close"]
        with pytest.raises(StopIteration):
            next(generator)
        assert events == [
            "build",
            "start",
            "connect",
            "bootstrap",
            "close",
            "terminate",
            "wait",
        ]
    finally:
        os.environ.clear()
        os.environ.update(previous_environment)
