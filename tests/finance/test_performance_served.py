"""R005.3.4 L4: the served performance method returns TWR, MWR and XIRR figures over a range
with a benchmark, and refuses an unknown range, an unknown benchmark and an unbound account."""

from __future__ import annotations

from typing import Any

import pytest
from conftest import TEST_TENANT

SCHEMA_VERSION = "finance-v1"
NS = "http://example.org/m#"


def _dt(text: str) -> str:
    return f'"{text}"^^xsd:dateTime'


def _dec(text: str) -> str:
    return f'"{text}"^^xsd:decimal'


def _put(graph: Any, kind: str, record_id: str, fields: dict[str, str]) -> None:
    graph.finance.record_put(
        kind,
        SCHEMA_VERSION,
        {"record_id": record_id, "tenant": TEST_TENANT, "fields": fields},
    )


def _activity(
    graph: Any,
    record_id: str,
    kind: str,
    position: str,
    date: str,
    account: str = "acct",
    **extra: str,
) -> None:
    fields = {
        "activityKind": f'"{kind}"',
        "activityPosition": f"<{NS}{position}>",
        "activityAccount": f"<{NS}{account}>",
        "activityTradeDate": _dt(date),
        "recordSource": '"broker"',
        "activitySession": '"regular"',
    }
    fields.update(extra)
    _put(graph, "Activity", record_id, fields)


def _seed(graph: Any) -> None:
    _put(graph, "Account", "acct", {"accountBindingState": '"verified"'})
    _put(graph, "Account", "loose", {"accountBindingState": '"pending"'})
    usd = {"activityCurrency": '"USD"'}
    _activity(graph, "v1", "valuation", "account-value", "2026-01-31T00:00:00Z",
              activityMarketValue=_dec("100"), **usd)
    _activity(graph, "d1", "deposit", "account-cash", "2026-02-15T00:00:00Z",
              activityAmount=_dec("10"), **usd)
    _activity(graph, "v2", "valuation", "account-value", "2026-03-01T00:00:00Z",
              activityMarketValue=_dec("121"), **usd)
    _activity(graph, "b1", "benchmark_level", "SPX", "2026-01-31T00:00:00Z",
              account="account-benchmark", activityPrice=_dec("4000"))
    _activity(graph, "b2", "benchmark_level", "SPX", "2026-03-01T00:00:00Z",
              account="account-benchmark", activityPrice=_dec("4400"))


def _ask(graph: Any, method: str, range_: str, benchmark: str | None = None,
         account: str = "acct") -> Any:
    return graph.finance.finance_performance(
        account, "2026-03-01T00:00:00Z", "2026-03-01T00:00:00Z", "USD", method, range_,
        benchmark,
    )


# spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.4
def test_twr_mwr_xirr_over_ranges_with_benchmark(clean_graph) -> None:
    _seed(clean_graph)
    twr = _ask(clean_graph, "twr", "1M", "SPX")
    # (121 - 10) / 100 - 1 over the window of the two valuations.
    assert twr["value"] == "0.11000000"
    assert twr["source"] == "broker" and twr["session"] == "regular"
    assert twr["benchmark"]["name"] == "SPX"
    assert twr["benchmark"]["value"] == "0.10000000"
    for method in ("mwr", "xirr"):
        card = _ask(clean_graph, method, "1M")
        assert card["value"] is not None and card["reason"] is None
    ytd = _ask(clean_graph, "twr", "YTD")
    assert ytd["value"] is None and ytd["reason"] == "NO_VALUATION_AT_START"

    with pytest.raises(Exception, match="RANGE_UNSUPPORTED"):
        _ask(clean_graph, "twr", "2W")
    with pytest.raises(Exception, match="BENCHMARK_UNKNOWN"):
        _ask(clean_graph, "twr", "1M", "NOPE")
    with pytest.raises(Exception, match="ACCOUNT_BINDING_UNVERIFIED"):
        _ask(clean_graph, "twr", "1M", account="loose")
