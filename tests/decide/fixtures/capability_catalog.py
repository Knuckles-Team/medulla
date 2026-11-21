"""Synthetic capability catalog for the served rank tests (MEDULLA-DECISION-ENGINE-R829.3).

Every name is ``<system>_<verb>_<object>``: 23 systems x 9 verbs x 7 objects =
1,449 distinct names, ordered by a seeded digest, never by the ``random`` module.
No system name equals a word of ``VERBS``, ``OBJECTS`` or the test query, apart
from the named system ``quillbox`` itself. Nothing here talks to the engine.
"""

from __future__ import annotations

import hashlib

from medulla.decision_client import DeclaredOption

CATALOG_SEED = 829

SYSTEMS = (
    "quillbox",
    "brindle",
    "tarnwick",
    "ostrava",
    "velmora",
    "pennant",
    "cobaltine",
    "hexley",
    "marrowby",
    "zephyrn",
    "lumbrell",
    "gantry",
    "ironvale",
    "sablecot",
    "thistlow",
    "umberly",
    "wrenfall",
    "yarrowen",
    "fenmoor",
    "dovecrest",
    "ashgrove",
    "kestrelo",
    "nimbleby",
)
VERBS = (
    "list",
    "get",
    "create",
    "update",
    "delete",
    "search",
    "approve",
    "decline",
    "export",
)
OBJECTS = ("request", "pending_request", "media", "issue", "user", "setting", "report")


def catalog_names(seed: int = CATALOG_SEED) -> list[str]:
    """Every name, in the order fixed by ``sha256(f"{seed}:{name}")``."""
    names = [f"{s}_{v}_{o}" for s in SYSTEMS for v in VERBS for o in OBJECTS]
    return sorted(
        names,
        key=lambda name: (hashlib.sha256(f"{seed}:{name}".encode()).hexdigest(), name),
    )


def catalog_options(seed: int = CATALOG_SEED) -> list[DeclaredOption]:
    """One option per name: ``op-0000`` upward in catalog order (ids sort)."""
    return [
        DeclaredOption(
            f"op-{i:04d}",
            texts={
                "rank.name": name,
                "rank.description": name.replace("_", " "),
                "rank.kind": "operation",
            },
        )
        for i, name in enumerate(catalog_names(seed))
    ]


def id_of(name: str, seed: int = CATALOG_SEED) -> str:
    """The option id of ``name``."""
    return f"op-{catalog_names(seed).index(name):04d}"
