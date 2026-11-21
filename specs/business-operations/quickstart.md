# MEDULLA-BUSINESS-OPS — Quickstart

The shortest served proof ingests an order, a payment and a payout, then joins them in one query.

## Scenario

1. Start a loopback engine with one tenant and one graph.
2. Send `SourceIngest` with one order, one payment and one payout from three source ids.
3. Send a `Uql` query for the order and read the payment id, the payout id and the provenance source of each leg.

## Commands

Run the proof for `MEDULLA-BUSINESS-OPS-R001.9` (planned):

```text
cargo test -p medulla-server-served-tests order_payment_payout_join_with_provenance
```

Run the recurrence proof for `MEDULLA-BUSINESS-OPS-R002.8` (planned):

```text
cargo test -p medulla-server-served-tests nl_question_returns_recurrence_with_inputs
```
