"""No crate re-exports another crate's items (MEDULLA-SDLC-R021.7).

A `lib.rs` or `mod.rs` under `crates/` holds no `pub use medulla_` line: a consumer names the
owning crate. The facade (`medulla-engine`) re-exports `medulla_pyengine` as its declared
public surface and is excluded by package name, not by an allowance list.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

pytestmark = [pytest.mark.no_engine, pytest.mark.spec("MEDULLA-SDLC-R021.7")]
ROOT = Path(__file__).resolve().parents[1]
CRATES = ROOT / "crates"
REEXPORT = re.compile(r"^\s*pub use medulla_")
# The facade package: its `src/lib.rs` is at the repository root, outside `crates/`, so no
# file under `crates/` belongs to it; the name is kept here to state the exclusion.
FACADE_PACKAGE = "medulla-engine"


def _scanned_files() -> list[Path]:
    files = [*CRATES.rglob("lib.rs"), *CRATES.rglob("mod.rs")]
    return [path for path in files if "target" not in path.relative_to(ROOT).parts]


def test_no_pub_use_of_another_workspace_crate() -> None:
    files = _scanned_files()
    print(f"scanned {len(files)} lib.rs and mod.rs files under crates/")
    assert files, "the scan found no lib.rs or mod.rs file"
    found = [
        f"{path.relative_to(ROOT)}:{number}: {line.strip()}"
        for path in files
        for number, line in enumerate(path.read_text().splitlines(), 1)
        if REEXPORT.match(line)
    ]
    assert found == [], f"{FACADE_PACKAGE} is the only facade; re-exports found: {found}"
