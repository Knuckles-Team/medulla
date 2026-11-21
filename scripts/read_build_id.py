#!/usr/bin/env python3
"""Print the build identity of an executable as lowercase hex (MEDULLA-CONTRACT-R060.2).

    read_build_id.py <binary>

Linux: the ``NT_GNU_BUILD_ID`` note of the ELF file. macOS: the ``LC_UUID`` load
command of the Mach-O file. Windows: the GUID of the CodeView ``RSDS`` record in the
PE debug directory, the 16 bytes in ``GUID`` field order. A file with none exits 1
with ``BUILD_ID_MISSING``. Python standard library only.
"""

from __future__ import annotations

import struct
import sys

NT_GNU_BUILD_ID = 3
PT_NOTE = 4
LC_UUID = 0x1B
IMAGE_DEBUG_TYPE_CODEVIEW = 2
DEBUG_DIRECTORY_INDEX = 6


class BuildIdMissing(Exception):
    """The file carries no build identity."""


def _elf_build_id(data: bytes) -> str:
    bits64 = data[4] == 2
    order = "<" if data[5] == 1 else ">"
    if bits64:
        (phoff,) = struct.unpack_from(order + "Q", data, 32)
        phentsize, phnum = struct.unpack_from(order + "HH", data, 54)
    else:
        (phoff,) = struct.unpack_from(order + "I", data, 28)
        phentsize, phnum = struct.unpack_from(order + "HH", data, 42)
    for index in range(phnum):
        base = phoff + index * phentsize
        if bits64:
            kind, _flags, offset, _va, _pa, filesz = struct.unpack_from(order + "IIQQQQ", data, base)[:6]
        else:
            kind, offset, _va, _pa, filesz = struct.unpack_from(order + "IIIII", data, base)[:5]
        if kind != PT_NOTE:
            continue
        cursor, end = offset, offset + filesz
        while cursor + 12 <= end:
            namesz, descsz, note_type = struct.unpack_from(order + "III", data, cursor)
            name_at = cursor + 12
            desc_at = name_at + ((namesz + 3) & ~3)
            if note_type == NT_GNU_BUILD_ID and data[name_at : name_at + namesz] == b"GNU\x00":
                return data[desc_at : desc_at + descsz].hex()
            cursor = desc_at + ((descsz + 3) & ~3)
    raise BuildIdMissing


def _macho_build_id(data: bytes) -> str:
    order = "<" if data[:4] in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe") else ">"
    header = 32 if data[:4] in (b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf") else 28
    commands = struct.unpack_from(order + "I", data, 16)[0]
    cursor = header
    for _ in range(commands):
        command, size = struct.unpack_from(order + "II", data, cursor)
        if command == LC_UUID:
            return data[cursor + 8 : cursor + 24].hex()
        if size < 8:
            break
        cursor += size
    raise BuildIdMissing


def _pe_build_id(data: bytes) -> str:
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe : pe + 4] != b"PE\x00\x00":
        raise BuildIdMissing
    (sections,) = struct.unpack_from("<H", data, pe + 6)
    (optional_size,) = struct.unpack_from("<H", data, pe + 20)
    optional = pe + 24
    plus = struct.unpack_from("<H", data, optional)[0] == 0x20B
    directories = optional + (112 if plus else 96)
    rva, size = struct.unpack_from("<II", data, directories + DEBUG_DIRECTORY_INDEX * 8)
    if rva == 0 or size == 0:
        raise BuildIdMissing
    table = optional + optional_size
    file_offset = None
    for index in range(sections):
        _name, vsize, va, rawsize, rawptr = struct.unpack_from("<8sIIII", data, table + index * 40)
        if va <= rva < va + max(vsize, rawsize):
            file_offset = rawptr + (rva - va)
            break
    if file_offset is None:
        raise BuildIdMissing
    for index in range(size // 28):
        entry = file_offset + index * 28
        (kind,) = struct.unpack_from("<I", data, entry + 12)
        (data_size,) = struct.unpack_from("<I", data, entry + 16)
        (pointer,) = struct.unpack_from("<I", data, entry + 24)
        if kind == IMAGE_DEBUG_TYPE_CODEVIEW and data_size >= 20 and data[pointer : pointer + 4] == b"RSDS":
            return data[pointer + 4 : pointer + 20].hex()
    raise BuildIdMissing


def read_build_id(data: bytes) -> str:
    """The build identity of the executable bytes, as lowercase hex."""
    try:
        if data[:4] == b"\x7fELF":
            return _elf_build_id(data)
        if data[:4] in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xfe\xed\xfa\xce"):
            return _macho_build_id(data)
        if data[:2] == b"MZ":
            return _pe_build_id(data)
    except (struct.error, IndexError) as error:
        raise BuildIdMissing from error
    raise BuildIdMissing


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: read_build_id.py <binary>", file=sys.stderr)
        return 2
    with open(argv[1], "rb") as handle:
        data = handle.read()
    try:
        print(read_build_id(data))
    except BuildIdMissing:
        print(f"BUILD_ID_MISSING: {argv[1]}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
