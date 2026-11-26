"""A generated repository of spec artifacts, shared by the served spec journeys.

MEDULLA-SDLC-R014.9 and R014.14 ingest a repository of requirement rows, task lines
and contracts. The tree is generated here, never written by hand:
``repository_files`` returns the ``(path, bytes)`` list ``index_repository`` takes,
and ``scope_of`` the scope that names it.
"""

from __future__ import annotations

import hashlib

from medulla.generated.index_repository import IndexRepositoryScope

REQUIREMENTS = "specs/demo/requirements.md"
TASKS = "specs/demo/tasks.md"
REVISION = "d" * 40


def row_id(n: int) -> str:
    return f"DEMO-R{n:04d}"


def task_id(n: int) -> str:
    return f"T{n:04d}"


def requirements_text(rows: int) -> str:
    lines = [
        "# Requirements",
        "",
        "| ID | Requirement | Acceptance |",
        "|----|----|----|",
    ]
    for n in range(1, rows + 1):
        lines.append(
            f"| `{row_id(n)}` | **Row {n}.** Caller chain: `serve_{n}` -> "
            f"`src/demo.py:{n}`. | Test: `tests/test_demo.py::test_row_{n}` (L3). |"
        )
    return "\n".join(lines) + "\n"


def tasks_text(tasks: int, ticked: int = 0) -> str:
    lines = ["# Tasks", "", "## Phase 1: Build"]
    for n in range(1, tasks + 1):
        box = "x" if n <= ticked else " "
        lines.append(f"- [{box}] {task_id(n)} {row_id(n)} Build row {n}")
    return "\n".join(lines) + "\n"


def contract_text(provider_row: str, consumer_row: str) -> str:
    return (
        f"# Seam {provider_row}\n\nProvider: demo {provider_row}\n"
        f"Consumer: demo {consumer_row}\n\n## Wire shape\n\nA request and a receipt.\n"
    )


def repository_files(
    rows: int = 2_000, tasks: int = 2_000, contracts: int = 10, ticked: int = 0
) -> list[tuple[str, bytes]]:
    """2,000 rows, 2,000 task lines and 10 contracts by default.

    Contract ``k`` joins the row ``rows - k + 1`` (provider) to the row
    ``rows - contracts - k + 1`` (consumer), so the tasks of the last
    ``2 * contracts`` rows are blocked by an unverified provider."""
    files = [
        (REQUIREMENTS, requirements_text(rows).encode()),
        (TASKS, tasks_text(tasks, ticked).encode()),
        ("src/demo.py", b"def serve():\n    return 1\n"),
    ]
    for k in range(1, contracts + 1):
        provider = row_id(rows - k + 1)
        consumer = row_id(rows - contracts - k + 1)
        files.append(
            (
                f"specs/demo/contracts/seam-{k:02d}.md",
                contract_text(provider, consumer).encode(),
            )
        )
    return files


def scope_of(repository: str, files: list[tuple[str, bytes]]) -> IndexRepositoryScope:
    return IndexRepositoryScope.model_validate(
        {
            "repository_id": repository,
            "commits": [],
            "refs": [
                {
                    "ref_name": "refs/heads/main",
                    "revision_id": REVISION,
                    "status": "live",
                }
            ],
            "file_versions": [
                {
                    "ref_name": "refs/heads/main",
                    "path": path,
                    "blob_digest": "sha256:" + hashlib.sha256(body).hexdigest(),
                }
                for path, body in files
            ],
        }
    )
