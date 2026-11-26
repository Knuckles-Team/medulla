"""No module of any crate re-exports another crate's items (MEDULLA-SDLC-R021.8).

No `.rs` file under a crate's `src/` holds a `pub use medulla_` line: a consumer
names the owning crate, whichever module the item was once re-exported from. This
widens the `lib.rs` and `mod.rs` rule of MEDULLA-SDLC-R021.7 to every module, because
a re-export from any module is a second import path for the same item. The facade
(`medulla-engine`) re-exports `medulla_pyengine` as its declared public surface; its
`src/lib.rs` is at the repository root, outside `crates/`, so it is excluded by
location, not by an allowance list.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

pytestmark = [
    pytest.mark.no_engine,
    pytest.mark.spec("MEDULLA-SDLC-R021.7"),
    pytest.mark.spec("MEDULLA-SDLC-R021.8"),
]
ROOT = Path(__file__).resolve().parents[1]
CRATES = ROOT / "crates"
REEXPORT = re.compile(r"^\s*pub use medulla_")
# The facade package: its `src/lib.rs` is at the repository root, outside `crates/`,
# so no file under `crates/` belongs to it; the name is kept here to state the
# exclusion.
FACADE_PACKAGE = "medulla-engine"


def _scanned_files() -> list[Path]:
    """Every `.rs` file under `crates/<crate>/src`, at any depth."""
    return sorted(CRATES.glob("*/src/**/*.rs"))


def test_no_pub_use_of_another_workspace_crate() -> None:
    files = _scanned_files()
    print(f"scanned {len(files)} .rs files under crates/*/src")
    assert files, "the scan found no .rs file under crates/*/src"
    found = [
        f"{path.relative_to(ROOT)}:{number}: {line.strip()}"
        for path in files
        for number, line in enumerate(path.read_text().splitlines(), 1)
        if REEXPORT.match(line)
    ]
    assert found == [], (
        f"{FACADE_PACKAGE} is the only facade; re-exports found: {found}"
    )
