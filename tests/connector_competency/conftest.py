"""Fixtures of the served connector-competency journeys.

``pg_source`` is a disposable PostgreSQL container the served engine reads as an
external SQL source (MEDULLA-CQ-CONN-R067.5). The engine's outbound allow-list is
read from the environment of the server process, which the session fixture starts
from this process's environment, so the loopback endpoint is named here, at
collection time, and appended to whatever the list already holds.
"""

from __future__ import annotations

import os
import socket
import subprocess
import time
import uuid
from collections.abc import Iterator
from dataclasses import dataclass

import pytest


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


PG_PORT = _free_port()
PG_PASSWORD = "competency-test"
os.environ["MEDULLA_FEDERATION_ALLOW"] = ",".join(
    part
    for part in (
        os.environ.get("MEDULLA_FEDERATION_ALLOW", ""),
        f"127.0.0.1:{PG_PORT}",
    )
    if part
)


@dataclass(frozen=True)
class PgSource:
    """A running PostgreSQL container and the ways a test reaches it."""

    container: str
    dsn: str

    def execute(self, sql: str) -> str:
        """Run one statement as the superuser and return its output."""
        done = subprocess.run(
            [
                "docker",
                "exec",
                self.container,
                "psql",
                "-U",
                "postgres",
                "-v",
                "ON_ERROR_STOP=1",
                "-tA",
                "-c",
                sql,
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        return done.stdout.strip()


@pytest.fixture(scope="session")
def pg_source() -> Iterator[PgSource]:
    name = f"medulla-cc-pg-{uuid.uuid4().hex[:8]}"
    subprocess.run(
        [
            "docker",
            "run",
            "-d",
            "--rm",
            "--name",
            name,
            "-e",
            f"POSTGRES_PASSWORD={PG_PASSWORD}",
            "-p",
            f"127.0.0.1:{PG_PORT}:5432",
            "postgres:16-alpine",
        ],
        check=True,
        capture_output=True,
    )
    try:
        deadline = time.monotonic() + 60
        while True:
            probe = subprocess.run(
                ["docker", "exec", name, "psql", "-U", "postgres", "-c", "SELECT 1"],
                capture_output=True,
            )
            if probe.returncode == 0:
                break
            if time.monotonic() > deadline:
                raise RuntimeError("the PostgreSQL container did not become ready")
            time.sleep(0.5)
        yield PgSource(
            container=name,
            dsn=f"postgres://postgres:{PG_PASSWORD}@127.0.0.1:{PG_PORT}/postgres",
        )
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)
