"""The release symbol split keeps the build-id in both files (MEDULLA-CONTRACT-R060.1)."""

from __future__ import annotations

import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = [pytest.mark.no_engine]

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "split_release_symbols.sh"
PROGRAM = """\
#include <stdio.h>

int release_split_probe(int value) {
    return value * 2 + 1;
}

int main(void) {
    printf("%d\\n", release_split_probe(20));
    return 0;
}
"""


def _run(*args: str) -> str:
    return subprocess.run(
        list(args), check=True, capture_output=True, text=True
    ).stdout


def _build_id(path: Path) -> str:
    notes = _run("readelf", "-n", str(path))
    found = re.search(r"Build ID:\s*([0-9a-f]+)", notes)
    assert found, f"{path.name} carries no GNU build-id"
    return found.group(1)


def _sections(path: Path) -> str:
    return _run("readelf", "-S", "-W", str(path))


def _build(workdir: Path) -> Path:
    source = workdir / "probe.c"
    source.write_text(PROGRAM)
    binary = workdir / "probe"
    compiler = shutil.which("cc") or shutil.which("gcc")
    assert compiler, "a C compiler is required to build the symbol-split fixture"
    subprocess.run(
        [
            compiler,
            "-g",
            "-O0",
            "-Wl,--build-id=sha1",
            "-o",
            str(binary),
            str(source),
        ],
        check=True,
        cwd=workdir,
    )
    return binary


def _split(binary: Path, outdir: Path) -> Path:
    subprocess.run(["bash", str(SCRIPT), str(binary), str(outdir)], check=True)
    return outdir / f"{binary.name}.debug"


# spec: MEDULLA-CONTRACT-R060.1
@pytest.mark.spec("MEDULLA-CONTRACT-R060.1")
@pytest.mark.skipif(
    not sys.platform.startswith("linux"),
    reason="the Linux branch of the split script; macOS and Windows run on their own runners",
)
def test_split_keeps_build_id_and_symbolizes(tmp_path: Path) -> None:
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    binary = _build(first)
    shutil.copy2(binary, second / "probe")
    original_id = _build_id(binary)
    assert "debug_info" in _sections(binary)

    debug = _split(binary, first / "symbols")

    # The shipped binary lost its debug data and kept the build-id; the symbol file
    # holds the debug data under the same build-id.
    assert "debug_info" not in _sections(binary)
    assert "debug_info" in _sections(debug)
    assert _build_id(binary) == original_id
    assert _build_id(debug) == original_id

    # The symbol file symbolizes an address of the stripped binary.
    symbols = _run("nm", str(debug))
    address = re.search(r"^([0-9a-f]+)\s+[Tt]\s+release_split_probe$", symbols, re.M)
    assert address, "the probe function is in the symbol file"
    located = _run("addr2line", "-e", str(debug), f"0x{address.group(1)}")
    assert "probe.c" in located

    # The split is a pure function of its input: the same bytes in, the same two
    # files out.
    other = _split(second / "probe", second / "symbols")
    assert (second / "probe").read_bytes() == binary.read_bytes()
    assert other.read_bytes() == debug.read_bytes()
