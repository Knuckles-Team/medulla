"""A recorded-response Hive metastore for the served federation tests (test-only).

It speaks the strict Thrift binary protocol over TCP and answers the calls the
engine's metastore client makes: ``get_all_databases``, ``get_all_tables``,
``get_table_req``, ``get_partitions`` and ``get_partitions_by_filter``. Tables
are plain data handed in by the test; nothing is replayed from the network.
"""

from __future__ import annotations

import re
import socket
import struct
import threading
from dataclasses import dataclass, field

VERSION_1 = 0x80010000
MSG_CALL, MSG_REPLY = 1, 2
T_STOP, T_BOOL, T_BYTE, T_DOUBLE = 0, 2, 3, 4
T_I16, T_I32, T_I64, T_STRING = 6, 8, 10, 11
T_STRUCT, T_MAP, T_SET, T_LIST = 12, 13, 14, 15

ORC_INPUT = "org.apache.hadoop.hive.ql.io.orc.OrcInputFormat"
AVRO_INPUT = "org.apache.hadoop.hive.ql.io.avro.AvroContainerInputFormat"


@dataclass
class HmsTable:
    db: str
    name: str
    columns: list[tuple[str, str]]
    location: str
    partition_keys: list[str] = field(default_factory=list)
    input_format: str = ORC_INPUT
    # (key values in partition_keys order, directory)
    partitions: list[tuple[list[str], str]] = field(default_factory=list)


class _Conn:
    def __init__(self, sock: socket.socket) -> None:
        self.sock = sock

    def exact(self, n: int) -> bytes:
        out = b""
        while len(out) < n:
            chunk = self.sock.recv(n - len(out))
            if not chunk:
                raise EOFError
            out += chunk
        return out

    def string(self) -> str:
        (n,) = struct.unpack(">i", self.exact(4))
        return self.exact(n).decode()

    def value(self, ty: int):
        if ty == T_BOOL or ty == T_BYTE:
            return self.exact(1)[0]
        if ty == T_DOUBLE:
            return self.exact(8)
        if ty == T_I16:
            return struct.unpack(">h", self.exact(2))[0]
        if ty == T_I32:
            return struct.unpack(">i", self.exact(4))[0]
        if ty == T_I64:
            return struct.unpack(">q", self.exact(8))[0]
        if ty == T_STRING:
            return self.string()
        if ty == T_STRUCT:
            fields = {}
            while True:
                field_type = self.exact(1)[0]
                if field_type == T_STOP:
                    return fields
                (fid,) = struct.unpack(">h", self.exact(2))
                fields[fid] = self.value(field_type)
        if ty in (T_LIST, T_SET):
            elem = self.exact(1)[0]
            (n,) = struct.unpack(">i", self.exact(4))
            return [self.value(elem) for _ in range(n)]
        if ty == T_MAP:
            key, val = self.exact(1)[0], self.exact(1)[0]
            (n,) = struct.unpack(">i", self.exact(4))
            return {self.value(key): self.value(val) for _ in range(n)}
        raise ValueError(f"unknown thrift type {ty}")


def _s(text: str) -> bytes:
    raw = text.encode()
    return struct.pack(">i", len(raw)) + raw


def _field(ty: int, fid: int, body: bytes) -> bytes:
    return struct.pack(">Bh", ty, fid) + body


def _list(elem: int, items: list[bytes]) -> bytes:
    return struct.pack(">Bi", elem, len(items)) + b"".join(items)


def _field_schema(name: str, ty: str) -> bytes:
    return _field(T_STRING, 1, _s(name)) + _field(T_STRING, 2, _s(ty)) + b"\x00"


def _storage(table: HmsTable, location: str) -> bytes:
    cols = _list(T_STRUCT, [_field_schema(n, t) for n, t in table.columns])
    return (
        _field(T_LIST, 1, cols)
        + _field(T_STRING, 2, _s(location))
        + _field(T_STRING, 3, _s(table.input_format))
        + b"\x00"
    )


def _table(table: HmsTable) -> bytes:
    keys = _list(T_STRUCT, [_field_schema(k, "string") for k in table.partition_keys])
    return (
        _field(T_STRING, 1, _s(table.name))
        + _field(T_STRING, 2, _s(table.db))
        + _field(T_STRUCT, 7, _storage(table, table.location))
        + _field(T_LIST, 8, keys)
        + b"\x00"
    )


def _partition(table: HmsTable, values: list[str], location: str) -> bytes:
    return (
        _field(T_LIST, 1, _list(T_STRING, [_s(v) for v in values]))
        + _field(T_STRING, 2, _s(table.db))
        + _field(T_STRING, 3, _s(table.name))
        + _field(T_STRUCT, 6, _storage(table, location))
        + b"\x00"
    )


def _matches(table: HmsTable, values: list[str], filter_text: str) -> bool:
    """Evaluate ``key = 'value' and ...`` against one partition's key values."""
    for clause in filter_text.split(" and "):
        found = re.fullmatch(r"\s*(\w+)\s*=\s*'(.*)'\s*", clause)
        if found is None:
            raise ValueError(f"unsupported filter clause: {clause!r}")
        key, wanted = found.group(1), found.group(2).replace("\\'", "'")
        if values[table.partition_keys.index(key)] != wanted:
            return False
    return True


class RecordedMetastore:
    """Serves ``tables`` on ``127.0.0.1:port``; ``calls`` records each call's name."""

    def __init__(self, tables: list[HmsTable], port: int) -> None:
        self.tables = {(t.db, t.name): t for t in tables}
        self.port = port
        self.calls: list[str] = []
        self._server = socket.socket()
        self._server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server.bind(("127.0.0.1", port))
        self._server.listen(16)
        self._stop = threading.Event()
        threading.Thread(target=self._accept, daemon=True).start()

    def close(self) -> None:
        self._stop.set()
        self._server.close()

    def _accept(self) -> None:
        while not self._stop.is_set():
            try:
                sock, _ = self._server.accept()
            except OSError:
                return
            threading.Thread(target=self._serve, args=(sock,), daemon=True).start()

    def _serve(self, sock: socket.socket) -> None:
        conn = _Conn(sock)
        try:
            while True:
                (head,) = struct.unpack(">I", conn.exact(4))
                if head & 0xFFFF0000 != VERSION_1 or head & 0xFF != MSG_CALL:
                    return
                name = conn.string()
                (seq,) = struct.unpack(">i", conn.exact(4))
                args = conn.value(T_STRUCT)
                self.calls.append(name)
                body = self._answer(name, args)
                reply = struct.pack(">I", VERSION_1 | MSG_REPLY) + _s(name)
                reply += struct.pack(">i", seq) + body
                sock.sendall(reply)
        except (EOFError, OSError):
            return
        finally:
            sock.close()

    def _answer(self, name: str, args: dict) -> bytes:
        if name == "get_all_databases":
            dbs = sorted({db for db, _ in self.tables})
            return _field(T_LIST, 0, _list(T_STRING, [_s(d) for d in dbs])) + b"\x00"
        if name == "get_all_tables":
            names = sorted(n for db, n in self.tables if db == args[1])
            return _field(T_LIST, 0, _list(T_STRING, [_s(n) for n in names])) + b"\x00"
        if name == "get_table_req":
            table = self.tables[(args[1][1], args[1][2])]
            return (
                _field(T_STRUCT, 0, _field(T_STRUCT, 1, _table(table)) + b"\x00")
                + b"\x00"
            )
        if name in ("get_partitions", "get_partitions_by_filter"):
            table = self.tables[(args[1], args[2])]
            chosen = [
                _partition(table, values, location)
                for values, location in table.partitions
                if name == "get_partitions" or _matches(table, values, args[3])
            ]
            return _field(T_LIST, 0, _list(T_STRUCT, chosen)) + b"\x00"
        raise ValueError(f"call not recorded: {name}")
