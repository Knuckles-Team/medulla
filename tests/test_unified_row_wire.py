"""Wire vector for a unified plan row (MEDULLA-CQ-XSRC-R002.7, R003.4, R003.5).

A `UnifiedQuery` row is `[id, score | nil]`; a row a source answered adds the
named values it attached as a third element ``{"texts": ..., "values": ...}``.
The Python result model takes both shapes and the client turns the third element
into the row's ``texts`` and ``values``.
"""

from __future__ import annotations

import msgpack
import pytest
from pydantic import TypeAdapter

from medulla.client import _unified_rows
from medulla.generated import models

pytestmark = pytest.mark.no_engine

# The bytes the engine sends for one stored row and one source row.
VECTOR = msgpack.packb(
    [
        ["n1", 0.5],
        ["m1", None, {"texts": {"status": "up"}, "values": {"monitorId": 7.0}}],
    ]
)


@pytest.mark.spec("MEDULLA-CQ-XSRC-R002.7")
def test_unified_rows_take_both_shapes() -> None:
    rows = msgpack.unpackb(VECTOR, raw=False)
    for result in (models.UnifiedQueryResult, models.TxnUnifiedQueryResult):
        adapter = TypeAdapter(result)
        parsed = adapter.validate_python(rows)
        assert parsed[1][2].values == {"monitorId": 7.0}, parsed
    assert _unified_rows(rows) == [
        {"id": "n1", "score": 0.5},
        {"id": "m1", "score": None, "texts": {"status": "up"}, "values": {"monitorId": 7.0}},
    ]


@pytest.mark.spec("MEDULLA-CQ-XSRC-R003.4")
def test_a_four_element_row_is_refused() -> None:
    with pytest.raises(Exception):
        TypeAdapter(models.UnifiedQueryResult).validate_python([["a", None, {}, 1]])
    with pytest.raises(Exception):
        TypeAdapter(models.UnifiedQueryResult).validate_python([["a", None, {"x": {}}]])
