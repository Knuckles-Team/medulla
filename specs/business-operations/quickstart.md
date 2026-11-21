# MEDULLA-BUSINESS-OPS — Quickstart

The shortest served proof stores one recurrence rule and its holiday calendar, then asks for the next due dates through one UQL statement.

## Scenario

1. Start a loopback engine with one tenant and one graph.
2. Write a `:RuleSpec` node `rule:sales-tax` (kind `recurrence`, version 1, monthly on day 20, next business day) and a `:HolidayCalendar` node `cal:1`.
3. Send `Uql` with `MATCH (:HolidayCalendar) WHERE id = 'cal:1' |> EVALUATE RULE 'rule:sales-tax' ASOF 1767225600000 COUNT 2` and read the due dates, the rule digest and the input revisions.

## Commands

Run the stage proof for `MEDULLA-BUSINESS-OPS-R002.7` (planned):

```text
cargo test -p medulla-server-served-tests evaluate_rule_stage_serves_recurrence_with_inputs
```

Run the digest proofs for `MEDULLA-BUSINESS-OPS-R003.1` to `R003.3` (planned):

```text
cargo test -p medulla-server-served-tests digest_
```
