"""An independent Iceberg reader loads the lake through its REST catalog.

Driven by `tests/lake_manifest_interoperability.rs::external_reader_loads_v2_manifests`,
which starts the lake listener over a seeded table and exports its address in
`LAKE_REST_URI` (and `LAKE_REST_TOKEN`). Run on its own it skips.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

pytest.importorskip("pyiceberg")
from pyiceberg.catalog.rest import RestCatalog  # noqa: E402

URI = os.environ.get("LAKE_REST_URI")
TOKEN = os.environ.get("LAKE_REST_TOKEN", "")
EXPECTED_ROWS = int(os.environ.get("LAKE_EXPECT_ROWS", "5"))
NAMESPACE = os.environ.get("LAKE_NAMESPACE", "engine")
TABLE = os.environ.get("LAKE_TABLE", "")

pytestmark = pytest.mark.skipif(
    URI is None, reason="LAKE_REST_URI is set by the Rust interoperability driver"
)


def _table():
    props = {"uri": URI, "py-io-impl": "lake_file_io.LakeFileIO"}
    if TOKEN:
        props["token"] = TOKEN
    catalog = RestCatalog("lake", **props)
    return catalog.load_table((NAMESPACE, TABLE))


# spec: MEDULLA-FEDERATED-QUERY-R079.6
def test_reader_sees_committed_rows() -> None:
    table = _table()
    assert [f.name for f in table.schema().fields] == ["ts", "value"]
    # Field ids are stable across the retained and the current snapshot.
    assert [f.field_id for f in table.schema().fields] == [1, 2]
    snapshots = table.metadata.snapshots
    assert snapshots, "the table has committed snapshots"
    for snapshot in snapshots:
        ids = [f.field_id for f in table.schema_by_id(snapshot.schema_id).fields]
        assert ids == [1, 2]
        assert snapshot.manifest_list.startswith("lake://")
    current = table.scan().to_arrow()
    assert current.num_rows == EXPECTED_ROWS
    assert sorted(current.column("value").to_pylist()) == [
        i * 1.5 for i in range(EXPECTED_ROWS)
    ]
    # The retained (first) snapshot reads through the same file route.
    retained = table.scan(snapshot_id=snapshots[0].snapshot_id).to_arrow()
    assert 0 < retained.num_rows <= EXPECTED_ROWS
