# MEDULLA-OBSERVE — Analysis

Governing spec: [spec.md](../spec.md).

## Method

- Ran `pipelines-hook spec-standard`, `pipelines-hook plan-conformance` and `pipelines-hook spec-decomposition`.
- Checked each row for a task line, a test entry, a caller chain or Shape sentence, a budget and a benchmark.
- Checked each cited file and line against the code at the branch base.

## Coverage

- Rows in this spec: 60. Rows with a task line and a test entry: 60.
- Rows bound to a shipped test: 0. All 60 name a planned test.
- Extension rows in other specs: MEDULLA-DURABLE-KERNEL-R873 (3), MEDULLA-CONTRACT-R060 (4). Consumer row: ARBORA-HOST-R008.1007.1.
- Every `FR-` and `SC-` label maps to an existing row.
- No clarification marker is open.

## Findings

- The incident cause chain maps to rows: stripped binary (MEDULLA-CONTRACT-R060, R009), wrapped typed error (R001), no listing of work (R003, R006), unguarded paging and quadratic cursor (MEDULLA-DURABLE-KERNEL-R873), missing log targets and request id (R002), hanging readiness (R007), unrecorded migration refusal (R008).
- Introspection of SQL catalogs (MEDULLA-ENGINE-PARITY-R008) is a different capability and stays there.
- The clustering kernel spec is not on the default branch at this revision. The contract names its ID as text, not as a link.

Result: **Pass**.
