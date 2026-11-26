# ruff: noqa: E501
"""A prompt blueprint survives export, disk write and re-export byte for byte.

MEDULLA-CONTRACT-R001.2. The blueprint is written to disk in the canonical byte profile:
UTF-8 without a byte-order mark, sorted keys, two-space indentation, LF endings, one final
newline, declared defaults, nulls and array order kept, NaN and Infinity refused, and the
number profile ``medulla-number`` (an integer without fraction or exponent, any other finite
number as its shortest decimal). The test plays the governed writer and reader of the disk
leg: medulla adds no filesystem writer. The semantic compare ignores key order and reports
the first difference as a JSON pointer; the byte compare is stricter and reports an offset.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import sys
import uuid
from decimal import Decimal
from pathlib import Path
from typing import Any

import pytest

_TESTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(_TESTS))
sys.path.insert(0, str(_TESTS / "typed_packs"))

from _commons_generation import connect as connect_engine
from _commons_generation import restartable_engine
from test_prompt_blueprints_served import (
    _content,
    _edited_envelope,
    _export,
    _import_blueprint,
    _publish_prompt,
)

pytestmark = [pytest.mark.no_engine]


# -- the canonical byte profile --------------------------------------------------------------


def encode_number(value: Any) -> str:
    """``medulla-number``: an integer plainly, any other finite number shortest, no exponent."""
    if isinstance(value, bool):
        raise TypeError("a boolean is not a number")
    if isinstance(value, int):
        return str(value)
    number = Decimal(repr(value)) if isinstance(value, float) else Decimal(value)
    if not number.is_finite():
        raise ValueError("NaN and Infinity are refused")
    if number == number.to_integral_value():
        return str(int(number))
    return format(number.normalize(), "f")


def encode(value: Any, depth: int = 0) -> str:
    """The canonical text of a parsed JSON value (no final newline)."""
    pad, inner = "  " * depth, "  " * (depth + 1)
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float, Decimal)):
        return encode_number(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, list):
        if not value:
            return "[]"
        items = ",\n".join(inner + encode(item, depth + 1) for item in value)
        return f"[\n{items}\n{pad}]"
    if isinstance(value, dict):
        if not value:
            return "{}"
        fields = ",\n".join(
            f"{inner}{json.dumps(key, ensure_ascii=False)}: {encode(value[key], depth + 1)}"
            for key in sorted(value)
        )
        return f"{{\n{fields}\n{pad}}}"
    raise TypeError(f"{type(value).__name__} is not a JSON value")


def canonical_bytes(value: Any) -> bytes:
    return (encode(value) + "\n").encode("utf-8")


def _refuse_constant(name: str) -> Any:
    raise ValueError(f"{name} is refused")


def _no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    keys = [key for key, _ in pairs]
    if len(set(keys)) != len(keys):
        raise ValueError("a duplicate key is refused")
    return dict(pairs)


def strict_parse(data: bytes) -> Any:
    """Parse canonical bytes: no byte-order mark, no NaN or Infinity, no duplicate key, and
    every number kept exact (a float is never coerced)."""
    if data.startswith(b"\xef\xbb\xbf"):
        raise ValueError("a byte-order mark is refused")
    return json.loads(
        data.decode("utf-8"),
        parse_float=Decimal,
        parse_constant=_refuse_constant,
        object_pairs_hook=_no_duplicates,
    )


def semantic_difference(left: Any, right: Any, pointer: str = "") -> str | None:
    """The JSON pointer of the first value that differs, ignoring key order."""
    if isinstance(left, dict) and isinstance(right, dict):
        for key in sorted(set(left) | set(right)):
            path = f"{pointer}/{key.replace('~', '~0').replace('/', '~1')}"
            if key not in left or key not in right:
                return path
            found = semantic_difference(left[key], right[key], path)
            if found is not None:
                return found
        return None
    if isinstance(left, list) and isinstance(right, list):
        for index, (a, b) in enumerate(zip(left, right, strict=False)):
            found = semantic_difference(a, b, f"{pointer}/{index}")
            if found is not None:
                return found
        return (
            None
            if len(left) == len(right)
            else f"{pointer}/{min(len(left), len(right))}"
        )
    same = type(left) is type(right) or (
        isinstance(left, (int, Decimal)) and isinstance(right, (int, Decimal))
    )
    return None if same and left == right else (pointer or "/")


def byte_difference(left: bytes, right: bytes) -> int | None:
    """The offset of the first differing byte."""
    for offset, (a, b) in enumerate(zip(left, right, strict=False)):
        if a != b:
            return offset
    return None if len(left) == len(right) else min(len(left), len(right))


def binding(export: Any, file_bytes: bytes) -> dict[str, Any]:
    """What an export binds: the blueprint revision, the model revision, a semantic digest
    over the parsed value and a digest of the canonical bytes."""
    semantic = canonical_bytes(strict_parse(file_bytes))
    return {
        "blueprint_revision": export.entry.entry_revision,
        "model_revision": export.schema_version,
        "semantic_digest": "sha256:" + hashlib.sha256(semantic).hexdigest(),
        "canonical_digest": "sha256:" + hashlib.sha256(file_bytes).hexdigest(),
    }


# -- the profile itself ----------------------------------------------------------------------


# spec: MEDULLA-CONTRACT-R001.2
def test_profile_encodes_and_refuses_as_declared() -> None:
    value = {
        "b": [3, 1, 2],
        "a": {"z": None, "y": True, "x": ""},
        "n": [0, -7, 9007199254740993, 0.5, 1e-07, 3.0, 12345678901234567890],
        "u": "role: é中文 \U0001f642",
        "e": {},
        "l": [],
    }
    encoded = canonical_bytes(value)
    assert encoded.endswith(b"}\n") and not encoded.endswith(b"\n\n")
    assert b"\r" not in encoded and not encoded.startswith(b"\xef\xbb\xbf")
    text = encoded.decode()
    assert text.index('"a"') < text.index('"b"') < text.index('"e"')
    assert '  "b": [\n    3,\n    1,\n    2\n  ],' in text
    assert "9007199254740993" in text and "0.0000001" in text and "e-" not in text
    assert '"x": ""' in text and '"z": null' in text
    # Parsing refuses what the profile refuses, and re-encoding the parse is a fixed point.
    assert canonical_bytes(strict_parse(encoded)) == encoded
    for refused in (
        b'{"a": NaN}',
        b'{"a": Infinity}',
        b'{"a": 1, "a": 2}',
        b"\xef\xbb\xbf{}",
    ):
        with pytest.raises(ValueError):
            strict_parse(refused)
    with pytest.raises(ValueError):
        canonical_bytes({"a": float("nan")})
    with pytest.raises(ValueError):
        canonical_bytes({"a": float("inf")})
    # The two compares: key order is no semantic difference, a byte compare names an offset.
    assert semantic_difference({"a": 1, "b": [1, 2]}, {"b": [1, 2], "a": 1}) is None
    assert semantic_difference({"a": {"b": [1, 2]}}, {"a": {"b": [1, 3]}}) == "/a/b/1"
    assert semantic_difference({"a": 1}, {"a": 1, "c": 0}) == "/c"
    assert byte_difference(b"abcd", b"abXd") == 2
    assert byte_difference(b"abc", b"abc") is None
    assert byte_difference(b"abc", b"abcd") == 3


# -- the served round trip -------------------------------------------------------------------


def _document(role: str) -> dict[str, Any]:
    return {
        "task": "summarise the ticket",
        "schema_revision": 1,
        "role": role,
        "instructions": ["collect", "summarise", "collect"],
        "variables": [],
        "extensions": {
            "default_priority": 0,
            "note": None,
            "nested": [{"k": "v", "n": None}, [1, 2, 3]],
            "limit": 9007199254740993,
            "ratio": 0.5,
            "epsilon": 1e-07,
        },
    }


# spec: MEDULLA-CONTRACT-R001.2
def test_blueprint_canonical_disk_roundtrip_has_no_difference(tmp_path: Path) -> None:
    component_id = f"prompt:canonical-{uuid.uuid4().hex[:12]}"
    disk = tmp_path / "disk"
    disk.mkdir()
    original = canonical_bytes(_document("a careful analyst \U0001f642"))
    edited = canonical_bytes(_document("a careful analyst, édited 中文"))
    bound: dict[str, Any] = {}

    async def run_one(engine: Any) -> None:
        client = await connect_engine(engine=engine)
        try:
            pin = hashlib.sha256(original).hexdigest()
            await _publish_prompt(client, component_id, original.decode(), pin, 0)
            first = await _export(client, component_id, 1)
            # Export: the body is the published bytes, already canonical, and re-encoding the
            # parse of it is a fixed point.
            assert bytes(first.body) == original
            assert canonical_bytes(strict_parse(bytes(first.body))) == bytes(first.body)

            # The disk leg (the governed writer's): write the edited canonical bytes, read
            # them back, and bind them.
            (disk / "blueprint.json").write_bytes(edited)
            on_disk = (disk / "blueprint.json").read_bytes()
            assert byte_difference(on_disk, edited) is None
            bound.update(binding(first, on_disk))
            assert bound["blueprint_revision"] == 1

            committed = await _import_blueprint(
                client, _edited_envelope(first, on_disk), expected_revision=1
            )
            assert committed.component.entry_revision == 2
        finally:
            await client.close()

    async def run_two(engine: Any) -> None:
        client = await connect_engine(engine=engine)
        try:
            on_disk = (disk / "blueprint.json").read_bytes()
            stored = await _content(client, component_id, 2)
            reexport = await _export(client, component_id, 2)
            # No difference by either compare, after the restart.
            assert byte_difference(stored, on_disk) is None
            assert byte_difference(bytes(reexport.body), on_disk) is None
            assert (
                semantic_difference(
                    strict_parse(on_disk), strict_parse(bytes(reexport.body))
                )
                is None
            )
            assert canonical_bytes(strict_parse(bytes(reexport.body))) == on_disk
            rebound = binding(reexport, on_disk)
            assert rebound["blueprint_revision"] == 2
            assert rebound["model_revision"] == bound["model_revision"]
            assert rebound["semantic_digest"] == bound["semantic_digest"]
            assert rebound["canonical_digest"] == bound["canonical_digest"]
            # The first revision is unchanged by the second.
            assert await _content(client, component_id, 1) == original
        finally:
            await client.close()

    with restartable_engine(tmp_path, "run1", bootstrap=True) as engine:
        asyncio.run(run_one(engine))
    with restartable_engine(tmp_path, "run2", bootstrap=False) as engine:
        asyncio.run(run_two(engine))
