"""Wire vector for a unified plan row (MEDULLA-CQ-XSRC-R002.7, R003.4, R003.5).

A `UnifiedQuery` row is `[id, score | nil]`; a row a source answered adds the
named text values it attached as a third element. The Python result model takes
both shapes and the client turns the third element into the row's ``texts``.
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
    [["n1", 0.5], ["SUP-0001", None, {"supplier_name": "Acme Cloud, Inc."}]]
)


@pytest.mark.spec("MEDULLA-CQ-XSRC-R002.7")
def test_unified_rows_take_both_shapes() -> None:
    rows = msgpack.unpackb(VECTOR, raw=False)
    for result in (models.UnifiedQueryResult, models.TxnUnifiedQueryResult):
        adapter = TypeAdapter(result)
        assert adapter.dump_python(adapter.validate_python(rows), mode="json") == [
            ["n1", 0.5],
            ["SUP-0001", None, {"supplier_name": "Acme Cloud, Inc."}],
        ]
    assert _unified_rows(rows) == [
        {"id": "n1", "score": 0.5},
        {"id": "SUP-0001", "score": None, "texts": {"supplier_name": "Acme Cloud, Inc."}},
    ]


@pytest.mark.spec("MEDULLA-CQ-XSRC-R003.4")
def test_a_four_element_row_is_refused() -> None:
    with pytest.raises(Exception):
        TypeAdapter(models.UnifiedQueryResult).validate_python([["a", None, {}, 1]])
