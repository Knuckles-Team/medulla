# Seam: Branch, decision and approval-wait steps applied in a run

Provider: medulla MEDULLA-DECISION-ENGINE-R134.111.14 and MEDULLA-DECISION-ENGINE-R180
Consumer: meristem MERISTEM-RUN-R018

## Wire shape

The harness receives branch, join, decision and approval-wait steps inside the canonical task plan. It asks the host decision port for each branch verdict and decision result, and it applies them. It decides nothing.

## Refusal codes

`BRANCH_NO_ARM`, `DMN_NO_RULE_MATCHED`, `DMN_EXPRESSION_UNSUPPORTED`, `PLAN_UNKNOWN`.

## Authoritative types

`BranchSpec`, `JoinSpec`, `DecisionSpec` and `ApprovalSpec` in `crates/medulla-decision-types/src/decision/task_plan/control.rs`; `BranchVerdict` and `TableVerdict` in `crates/medulla-decision-types/src/decision/process_decision.rs`.

## Test vectors

The served tests named by the rows of this seam in `test-spec.md` hold the vectors that both sides test against.
