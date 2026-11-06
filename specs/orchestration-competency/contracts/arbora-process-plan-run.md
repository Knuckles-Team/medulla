# Seam: Process plan served and run

Provider: medulla MEDULLA-CQ-ORCH-R704.5 and MEDULLA-DECISION-ENGINE-R180.4
Consumer: arbora ARBORA-OPS-R898

## Wire shape

Arbora asks `Decide` with `QuestionKind::ProcessPlan` and receives the sealed task plan, its digest and one `StepBinding` per step. Arbora passes the plan to the run loop unchanged and asks `BranchSelect` and `DecisionTableEval` at run time. The engine methods of R702, R703 and R705 are served as generated `eg.*` operations.

## Refusal codes

`PROCESS_STEP_UNBOUND`, `PROCESS_PLAN_LOOP`, `PROCESS_PLAN_TOO_LARGE`, `WORKFLOW_UNKNOWN`, `BRANCH_NO_ARM`, `DMN_NO_RULE_MATCHED`.

## Authoritative types

`StepBinding` in `crates/medulla-decision-types/src/decision/process_plan.rs`; control steps in `crates/medulla-decision-types/src/decision/task_plan/control.rs`.

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
