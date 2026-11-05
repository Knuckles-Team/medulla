# Seam: harness consumption of decisions

Provider: medulla MEDULLA-DECISION-ENGINE-R134.111
Consumer: meristem MERISTEM-HARNESS-CYCLE-R001

## Wire shape

The harness sends a task-plan request through arbora. Medulla returns a bounded task graph with cited facts. The harness applies the plan and never reranks it.

## Refusal codes

Medulla refuses a stale or malformed plan input with a typed code before it exposes a plan.

## Authoritative types

`task_plan` types in `crates/medulla-decision-types/src/decision/task_plan.rs`.

## Test vectors

The served tests named by the provider row are the vectors; the consumer proves its side against the same request and response types.
