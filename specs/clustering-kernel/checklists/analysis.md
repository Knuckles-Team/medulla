# MEDULLA-CLUSTER — Analysis

Governing spec: [spec.md](../spec.md).

## Method

- Ran `pipelines-hook spec-standard`, `pipelines-hook plan-conformance` and `pipelines-hook ste-staged`.
- Checked each row for a task line, a test entry, a caller chain, a budget and a benchmark.
- Opened every cited file and line at the branch base.

## Coverage

- Rows: 56. Rows with a task line: 56. Every `FR-` and `SC-` label maps to a row.
- No row is bound to a shipped test except the existing tests that R008 keeps.

## Findings

- `MEDULLA-OBSERVE` has no provider row yet. R004.3 ships with a progress sink and task `T-OBSERVE-BIND` binds the row later.
- Today a build holds the index write lock and the store read lock for its whole run (R011.1). This explains blocked writers and queries during a boot build.
