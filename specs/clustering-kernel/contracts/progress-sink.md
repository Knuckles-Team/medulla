# Seam contract: training progress

Governing spec: [spec.md](../spec.md).

Provider: medulla MEDULLA-OBSERVE-R003
Consumer: medulla MEDULLA-CLUSTER-R004.3

## Provider

Repository: medulla. The provider row is `MEDULLA-OBSERVE-R003` (task registry); the registry contract is `specs/engine-observability/contracts/task-registry.md`. A training run registers `train:<run id>` with owner `clustering` and reports `Counter(step)`.

## Consumer

Repository: medulla, row `MEDULLA-CLUSTER-R004.3`. The kernel never imports the registry. It calls `ProgressSink::report(&Progress)`. The index build adapts the sink to the registry entry of its task.

## Wire shape

| Field | Type | Meaning |
|---|---|---|
| `state` | text | `Seeding`, `Sampling`, `Updating`, `Converged`, `BudgetExhausted` or `Refused` |
| `step` | integer | Current step, from zero |
| `max_steps` | integer | The step budget of the plan |
| `shift` | decimal | Relative inertia change of the last step |
| `elapsed_ms` | integer | Milliseconds since the run started |

A sink receives at most one report per 250 ms and one at each state change. A sink must not block; a slow sink drops the in-between reports.

## Refusal codes

`CLUSTERING_K_INVALID`, `CLUSTERING_K_EXCEEDS_ROWS`, `CLUSTERING_TOO_FEW_DISTINCT`, `CLUSTERING_SHAPE_INVALID`, `CLUSTERING_INPUT_NOT_FINITE`, `CLUSTERING_MEMORY_BUDGET`, `CLUSTERING_CANCELLED`, `CLUSTERING_STATE_ILLEGAL`. The served data-science method prefixes its error text with the code.
