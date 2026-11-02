"""No crate root carries a crate-level lint allow; each allow is scoped to one item."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRATE_LEVEL_ALLOW = re.compile(r"^\s*#!\[allow\(", re.MULTILINE)


def _crate_roots() -> list[Path]:
    roots = [ROOT / "src" / "lib.rs", ROOT / "src" / "main.rs"]
    for crate in sorted((ROOT / "crates").iterdir()):
        for name in ("lib.rs", "main.rs"):
            roots.append(crate / "src" / name)
    return [path for path in roots if path.is_file()]


# spec: MEDULLA-CONTRACT-R056.17
def test_no_crate_root_carries_a_crate_level_lint_allow() -> None:
    offenders = [
        str(path.relative_to(ROOT))
        for path in _crate_roots()
        if CRATE_LEVEL_ALLOW.search(path.read_text(encoding="utf-8"))
    ]
    assert not offenders, f"crate-level lint allows remain: {offenders}"
