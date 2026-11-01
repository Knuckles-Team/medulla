# Seam contract: task run

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-SDLC-R015.3
Consumer: meristem MERISTEM-SCM-R011.2

## Wire shape

The engine sends one task context per run. Meristem returns one result.

| Record | Fields |
|---|---|
| Task context | `{task_id, row_id, row_text, plan_sections[], contracts[], data_model, code_neighbourhood, base_digest}` |
| Task result | `{status, changeset_id?, evidence{gate_results[], test_results[], base_digest}, failure?{code, stage, evidence}}` |

## Refusal codes

- `TASK_CONTEXT_PLAN_MISSING`: the row has no plan section, so the engine sends no context.
- `task_context_incomplete`: meristem refuses a context with an empty row text or base digest.
- `gate_failed`, `conflict_retry_exhausted`, `submitter_unavailable`, `sandbox_unavailable`: typed run failures.

## Authoritative types

- `crates/medulla-decision-types/src/decision/task_plan.rs`: the dispatch decision.
- meristem `meristem/harness/task_run.py`: `TaskRunContext` and `TaskRunResult`.

## Test vectors

- `tests/sdlc/test_task_dispatch_served.py::test_dispatch_hands_context_to_a_run_and_logs_it` holds the dispatch vector.
