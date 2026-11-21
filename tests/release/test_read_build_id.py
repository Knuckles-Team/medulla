"""The one build-id reader reads ELF, Mach-O and PE files (MEDULLA-CONTRACT-R060.2)."""

from __future__ import annotations

import struct
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = [pytest.mark.no_engine]

READER = Path(__file__).resolve().parents[2] / "scripts" / "read_build_id.py"
ELF_ID = bytes(range(1, 21))
MACHO_ID = bytes(range(0x10, 0x20))
PE_GUID = bytes(range(0xA0, 0xB0))


def _elf() -> bytes:
    note = struct.pack("<III", 4, len(ELF_ID), 3) + b"GNU\x00" + ELF_ID
    header = bytearray(64)
    header[0:4] = b"\x7fELF"
    header[4], header[5], header[6] = 2, 1, 1
    struct.pack_into("<Q", header, 32, 64)
    struct.pack_into("<HH", header, 54, 56, 1)
    program = struct.pack("<IIQQQQQQ", 4, 4, 64 + 56, 0, 0, len(note), len(note), 4)
    return bytes(header) + program + note


def _macho() -> bytes:
    command = struct.pack("<II", 0x1B, 24) + MACHO_ID
    header = struct.pack("<IiiIIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 1, len(command), 0, 0)
    return header + command


def _pe() -> bytes:
    section_rva, section_raw = 0x1000, 0x200
    codeview = b"RSDS" + PE_GUID + struct.pack("<I", 1) + b"x.pdb\x00"
    entry = struct.pack("<IIHHIIII", 0, 0, 0, 0, 2, len(codeview), section_rva + 28, section_raw + 28)
    optional = bytearray(112 + 16 * 8)
    struct.pack_into("<H", optional, 0, 0x20B)
    struct.pack_into("<II", optional, 112 + 6 * 8, section_rva, 28)
    pe_at = 0x40
    coff = struct.pack("<HHIIIHH", 0x8664, 1, 0, 0, 0, len(optional), 0)
    section = struct.pack("<8sIIIIIIHHI", b".rdata", 0x100, section_rva, 0x100, section_raw, 0, 0, 0, 0, 0)
    head = bytearray(pe_at) + b"PE\x00\x00" + coff + bytes(optional) + section
    head[0:2] = b"MZ"
    struct.pack_into("<I", head, 0x3C, pe_at)
    head = head.ljust(section_raw, b"\x00")
    return bytes(head) + entry + codeview


def _read(path: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(READER), str(path)], capture_output=True, text=True, check=False
    )


# spec: MEDULLA-CONTRACT-R060.2
@pytest.mark.spec("MEDULLA-CONTRACT-R060.2")
def test_reads_the_id_of_an_elf_a_macho_and_a_pe_fixture(tmp_path: Path) -> None:
    for name, data, expected in (
        ("elf", _elf(), ELF_ID.hex()),
        ("macho", _macho(), MACHO_ID.hex()),
        ("pe", _pe(), PE_GUID.hex()),
    ):
        path = tmp_path / name
        path.write_bytes(data)
        run = _read(path)
        assert run.returncode == 0, run.stderr
        assert run.stdout.strip() == expected

    empty = tmp_path / "plain"
    empty.write_bytes(b"not an executable")
    run = _read(empty)
    assert run.returncode == 1
    assert "BUILD_ID_MISSING" in run.stderr
