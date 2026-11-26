"""MEDULLA-UNIFIED-DATA-PLANE-R038.4 served acceptance: ``hive()`` over a metastore.

A registered metastore source is read through the ``hive()`` SQL table function.
The endpoint comes from the registered source, never from SQL text. The source
belongs to the principal that registered it: another principal cannot name it. A
scan returns its exact receipt in the query warnings as ``hive_scan_receipt``.
"""

from __future__ import annotations

import json
import os

import pytest

from medulla.client import EngineResponseError, SyncMedullaClient

try:
    from conftest import request_context
except ImportError:  # pragma: no cover - rootdir import layout
    from tests.conftest import request_context
from .hms_recorded import AVRO_INPUT, HmsTable, RecordedMetastore

pa = pytest.importorskip("pyarrow")
orc = pytest.importorskip("pyarrow.orc")

DAYS = {"mon": [1, 2], "tue": [3, 4, 5], "wed": [6]}


def _write_orc_facts(root) -> HmsTable:
    table = HmsTable(
        db="db",
        name="orc_facts",
        columns=[("id", "bigint"), ("label", "string")],
        partition_keys=["day"],
        location=str(root / "orc_facts"),
    )
    for day, ids in DAYS.items():
        directory = root / "orc_facts" / f"day={day}"
        directory.mkdir(parents=True)
        batch = pa.table(
            {"id": pa.array(ids, pa.int64()), "label": [f"{day}-{i}" for i in ids]}
        )
        orc.write_table(batch, str(directory / "part-0.orc"))
        table.partitions.append(([day], str(directory)))
    return table


EVENTS = {
    "mon": [(10, "open"), (11, "close")],
    "tue": [(12, "open")],
    "wed": [(13, "open")],
}


def _avro_long(value: int) -> bytes:
    """One Avro long: zigzag, then base-128 varint."""
    zigzag = (value << 1) ^ (value >> 63)
    out = bytearray()
    while zigzag > 0x7F:
        out.append((zigzag & 0x7F) | 0x80)
        zigzag >>= 7
    out.append(zigzag)
    return bytes(out)


def _avro_bytes(value: bytes) -> bytes:
    return _avro_long(len(value)) + value


def _avro_file(rows: list[tuple[int, str]]) -> bytes:
    """An Avro object container file (null codec) of `{id: long, kind: string}`
    records."""
    schema = json.dumps(
        {
            "type": "record",
            "name": "event",
            "fields": [
                {"name": "id", "type": "long"},
                {"name": "kind", "type": "string"},
            ],
        }
    ).encode()
    sync = bytes(range(16))
    meta = (
        _avro_long(2)
        + _avro_bytes(b"avro.schema")
        + _avro_bytes(schema)
        + _avro_bytes(b"avro.codec")
        + _avro_bytes(b"null")
        + _avro_long(0)
    )
    body = b"".join(_avro_long(i) + _avro_bytes(kind.encode()) for i, kind in rows)
    return (
        b"Obj\x01"
        + meta
        + sync
        + _avro_long(len(rows))
        + _avro_long(len(body))
        + body
        + sync
    )


def _write_avro_events(root) -> HmsTable:
    table = HmsTable(
        db="db",
        name="avro_events",
        columns=[("id", "bigint"), ("kind", "string")],
        partition_keys=["day"],
        location=str(root / "avro_events"),
        input_format=AVRO_INPUT,
    )
    for day, rows in EVENTS.items():
        directory = root / "avro_events" / f"day={day}"
        directory.mkdir(parents=True)
        (directory / "part-0.avro").write_bytes(_avro_file(rows))
        table.partitions.append(([day], str(directory)))
    return table


@pytest.fixture
def metastore(tmp_path, hms_port):
    server = RecordedMetastore(
        [_write_orc_facts(tmp_path), _write_avro_events(tmp_path)], hms_port
    )
    yield server
    server.close()


def _source(port: int) -> dict:
    return {
        "Metastore": {
            "endpoint": f"127.0.0.1:{port}",
            "database_filter": "db",
            "auth_ref": "env://HMS_TOKEN",
        }
    }


def _other_principal(name: str) -> SyncMedullaClient:
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(principal=name, agent_id=name),
    )


def _receipt(warnings: list[str]) -> dict:
    prefix = "hive_scan_receipt="
    (entry,) = [w for w in warnings if w.startswith(prefix)]
    return json.loads(entry[len(prefix) :])


# proves R038.4.3: Wiring: the hive() table function and discovery
@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.4.3")
def test_hive_function_registered_and_scoped(clean_graph, metastore, hms_port):
    clean_graph.query.register_foreign_source("hive-a", _source(hms_port))
    contracts = [
        props
        for _, props in clean_graph.nodes.list_by_label("MetadataContract", 0)
        if props["source_name"] == "hive-a"
    ]
    assert len(contracts) == 1
    assert contracts[0]["source_kind"] == "hadoop_catalog"
    assert len(contracts[0]["content_hash"]) == 64
    assert "get_table_req" in metastore.calls, "discovery read the catalog"

    # The owner reads the table by name, and without a name when it owns one source.
    named, _ = clean_graph.query.sql_with_warnings(
        "SELECT id, label, day FROM hive('hive-a', 'db.orc_facts') "
        "WHERE day = 'mon' ORDER BY id"
    )
    assert named == [
        {"id": 1, "label": "mon-1", "day": "mon"},
        {"id": 2, "label": "mon-2", "day": "mon"},
    ]
    bare = clean_graph.query.sql("SELECT count(*) AS n FROM hive('db.orc_facts')")
    assert bare == [{"n": 6}]

    # A second principal registered nothing: the first one's source cannot be named,
    # and the refusal is the one an unknown name gets, so it hides the source.
    stranger = _other_principal("hive-stranger")
    for query in (
        "SELECT * FROM hive('hive-a', 'db.orc_facts')",
        "SELECT * FROM hive('db.orc_facts')",
        "SELECT * FROM hive('never-registered', 'db.orc_facts')",
    ):
        with pytest.raises(EngineResponseError) as refused:
            stranger.query.sql(query)
        assert "METASTORE_SPEC_INVALID" in str(refused.value)
    with pytest.raises(EngineResponseError) as unknown:
        stranger.query.sql("SELECT * FROM hive('hive-a', 'db.orc_facts')")
    with pytest.raises(EngineResponseError) as absent:
        stranger.query.sql("SELECT * FROM hive('never-registered', 'db.orc_facts')")
    assert str(unknown.value).replace("hive-a", "X") == str(absent.value).replace(
        "never-registered", "X"
    )


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.4.3")
def test_unreachable_metastore_registers_nothing(clean_graph, dead_port):
    before = clean_graph.nodes.count()
    with pytest.raises(EngineResponseError) as refused:
        clean_graph.query.register_foreign_source("hive-dead", _source(dead_port))
    assert "DISCOVERY_UNREACHABLE" in str(refused.value)
    assert clean_graph.nodes.count() == before


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.4.5")
def test_served_scan_receipt_counts_what_the_scan_opened(
    clean_graph, metastore, hms_port
):
    clean_graph.query.register_foreign_source("hive-a", _source(hms_port))
    _, warnings = clean_graph.query.sql_with_warnings(
        "SELECT id FROM hive('hive-a', 'db.orc_facts') WHERE day = 'tue'"
    )
    receipt = _receipt(warnings)
    assert receipt["partitions_read"] == 1
    assert receipt["partitions_pruned"] == 2
    assert receipt["files_read"] == 1
    assert receipt["stripes_read"] == 1
    assert receipt["stripes_skipped"] == 0
    assert receipt["bytes_read"] > 0
    _, full = clean_graph.query.sql_with_warnings(
        "SELECT id FROM hive('hive-a', 'db.orc_facts')"
    )
    assert _receipt(full)["partitions_read"] == 3
    assert _receipt(full)["files_read"] == 3


# proves R038.4.6: Hot-path proof: a pruned scan over a recorded metastore
@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R038.4.6")
def test_partition_pruned_scan(clean_graph, metastore, hms_port):
    """A predicate on the partition key reads only matching partitions, and every
    answer equals the full-scan oracle computed from the files' known rows."""
    clean_graph.query.register_foreign_source("hive-p", _source(hms_port))

    # ORC: a partition-key predicate prunes the other two partitions.
    rows, warnings = clean_graph.query.sql_with_warnings(
        "SELECT id FROM hive('hive-p', 'db.orc_facts') WHERE day = 'tue' ORDER BY id"
    )
    assert [row["id"] for row in rows] == DAYS["tue"]
    receipt = _receipt(warnings)
    assert receipt["partitions_read"] == 1
    assert receipt["partitions_pruned"] == 2

    # The unpruned scan is the oracle: every row of every partition.
    rows, warnings = clean_graph.query.sql_with_warnings(
        "SELECT id, day FROM hive('hive-p', 'db.orc_facts') ORDER BY id"
    )
    oracle = sorted((i, day) for day, ids in DAYS.items() for i in ids)
    assert [(row["id"], row["day"]) for row in rows] == oracle
    assert _receipt(warnings)["partitions_read"] == len(DAYS)
    pruned_ids = sorted(i for i, day in oracle if day == "tue")
    assert pruned_ids == DAYS["tue"]

    # Avro: the same pruning, and the same equality with the oracle.
    rows, warnings = clean_graph.query.sql_with_warnings(
        "SELECT id, kind FROM hive('hive-p', 'db.avro_events') "
        "WHERE day = 'mon' ORDER BY id"
    )
    assert [(row["id"], row["kind"]) for row in rows] == EVENTS["mon"]
    receipt = _receipt(warnings)
    assert receipt["partitions_read"] == 1
    assert receipt["partitions_pruned"] == 2
    rows, warnings = clean_graph.query.sql_with_warnings(
        "SELECT id, kind FROM hive('hive-p', 'db.avro_events') ORDER BY id"
    )
    assert [(row["id"], row["kind"]) for row in rows] == sorted(
        row for rows_ in EVENTS.values() for row in rows_
    )
    assert _receipt(warnings)["partitions_read"] == len(EVENTS)
