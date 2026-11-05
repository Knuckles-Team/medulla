# MEDULLA-FINANCE-PRIMITIVES — Quickstart

The shortest served proofs of the finance primitives drive one request per area through the production dispatch with a tenant and principal.

## Scenario

1. Start the served harness, which builds a tenant, a principal and synthetic fixtures.
2. Put and get an `Account` through `FinanceRecordPut` and `FinanceRecordGet`, then repeat with an unknown schema version.
3. Seal one `BacktestRun` through `FinanceMarket` and compare its digest with the pinned constant.
4. Evaluate one kernel through `NumericEvaluate` and compare it with the independent oracle vector.

## Commands

Run the proof for `MEDULLA-FINANCE-PRIMITIVES-R001`:

```text
cargo test -p medulla-finance-market a_sealed_backtest_run_digest_is_pinned_across_hosts
```

Run the served record proof for `MEDULLA-FINANCE-PRIMITIVES-R004.3.2.1`:

```text
cargo test --test finance_records_served account_served_round_trip_and_refusals
```

Run the served kernel proof for `MEDULLA-FINANCE-PRIMITIVES-R002.2.5`:

```text
cargo test --test finance_numeric_evaluate_served
```

Run the client round trips:

```text
uv run pytest tests/test_finance_record_client.py
```
