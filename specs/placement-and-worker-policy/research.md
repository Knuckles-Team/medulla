# MEDULLA-PLACEMENT — Research

Phase 0 unknowns and their resolution status.

## Production deployment template (Arbora, typed packs)

Unknown: the production deployment template and fleet-tool components do not exist in any Agent Library yet, so the assembled step-graph proof cannot run against production templates. Status: BLOCKED on the owner. The engine proof runs over a fixture template held in the tenant Agent Library (one template per target with the `render-artifact`, `apply` and `verify` slots and three tool components). Arbora publishes the production templates and components as a typed pack; the consumer link is ARBORA-DEPLOY-R017.

## Capacity adjustment ownership (decision engine)

Unknown at the 2026-10-30 cross-spec review: who owns the typed capacity adjustment proposal and verdict. Status: DECIDED. The question, verdict and the nine refusal codes live in `MEDULLA-DECISION-ENGINE-R824` under `MEDULLA-DECISION-ENGINE-R112`; model concurrency, circuit breaking and GPU group budgets live in `MEDULLA-DECISION-ENGINE-R825`. Arbora generates the AIMD proposal and actuates. This spec keeps one rule: the evaluator reuses the placement headroom premise and adds no second controller.
