"""One command symbolizes a stack from a symbol artifact (MEDULLA-CONTRACT-R060.3)."""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = [pytest.mark.no_engine]

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
PROGRAM = """\
#include <stdio.h>

int symbolize_probe(int value) {
    return value * 3 + 1;
}

int main(void) {
    printf("%d\\n", symbolize_probe(14));
    return 0;
}
"""


def _run(
    *args: str, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(args), capture_output=True, text=True, check=False, env=env
    )


def _build(workdir: Path) -> Path:
    source = workdir / "probe.c"
    source.write_text(PROGRAM)
    binary = workdir / "probe"
    compiler = shutil.which("cc") or shutil.which("gcc")
    assert compiler, "a C compiler is required to build the symbolize fixture"
    subprocess.run(
        [compiler, "-g", "-O0", "-Wl,--build-id=sha1", "-o", str(binary), str(source)],
        check=True,
        cwd=workdir,
    )
    return binary


# spec: MEDULLA-CONTRACT-R060.3
@pytest.mark.spec("MEDULLA-CONTRACT-R060.3")
@pytest.mark.skipif(
    not sys.platform.startswith("linux"),
    reason="the Linux symbol file is the one addr2line reads",
)
# proves R060.3: One command symbolizes a stack
def test_symbolize_resolves_offsets_from_a_local_artifact(tmp_path: Path) -> None:
    binary = _build(tmp_path)
    symbols = tmp_path / "symbols"
    split = _run(
        "bash", str(SCRIPTS / "split_release_symbols.sh"), str(binary), str(symbols)
    )
    assert split.returncode == 0, split.stderr
    build_id = _run(
        sys.executable, str(SCRIPTS / "read_build_id.py"), str(binary)
    ).stdout.strip()
    assert re.fullmatch(r"[0-9a-f]+", build_id)

    out = tmp_path / "dist-symbols"
    env = {**os.environ, "SOURCE_DATE_EPOCH": "1700000000"}
    packaged = _run(
        "bash",
        str(SCRIPTS / "package_release_symbols.sh"),
        "linux-aarch64",
        str(binary),
        str(symbols),
        str(out),
        env=env,
    )
    assert packaged.returncode == 0, packaged.stderr

    nm = _run("nm", str(symbols / "probe.debug")).stdout
    address = re.search(r"^([0-9a-f]+)\s+[Tt]\s+symbolize_probe$", nm, re.M)
    assert address, "the probe function is in the symbol file"
    offset = f"0x{address.group(1)}"

    def symbolize(*extra: str) -> subprocess.CompletedProcess[str]:
        return _run(
            "bash",
            str(SCRIPTS / "symbolize.sh"),
            "--artifact-dir",
            str(out),
            "--build-id",
            build_id,
            *extra,
            "v0.0.0",
            "linux-aarch64",
            offset,
        )

    resolved = symbolize()
    assert resolved.returncode == 0, resolved.stderr
    assert offset in resolved.stdout
    assert "symbolize_probe" in resolved.stdout
    assert "probe.c" in resolved.stdout

    # A server whose build-id has no artifact refuses before resolving anything.
    other = _run(
        "bash",
        str(SCRIPTS / "symbolize.sh"),
        "--artifact-dir",
        str(out),
        "--build-id",
        "0" * len(build_id),
        "v0.0.0",
        "linux-aarch64",
        offset,
    )
    assert other.returncode == 1
    assert "SYMBOL_ARTIFACT_MISSING" in other.stderr

    # A tampered artifact refuses on its digest.
    archive = next(out.glob("symbols-*.tar.zst"))
    archive.write_bytes(archive.read_bytes() + b"x")
    tampered = symbolize()
    assert tampered.returncode == 1
    assert "SYMBOL_ARTIFACT_DIGEST_MISMATCH" in tampered.stderr
