# MEDULLA-INCREMENTAL-VIEWS — Analysis

Governing spec: [spec.md](../spec.md).

## Method

- Ran `pipelines-hook spec-standard --base origin/split/all`, `pipelines-hook plan-conformance` with the cross-repo ID index, and `pipelines-hook spec-decomposition`.
- Checked each row for a task line, a test entry, a caller chain, a Shape sentence, a budget and a benchmark.
- Checked each cited file and line against the code at the branch base.

## Coverage

- Rows: 72. Rows with a task line and a test entry: 72.
- Rows bound to a shipped test: 12 (R001.1 to R001.9, R011.1 to R011.3).
- Rows with a planned test in test-spec.md: 60.
- Every `FR-` and `SC-` label maps to an existing row.
- No clarification marker is open.

## Findings

- R009, R010 and R012 to R013 depend on MEDULLA-ENGINE-PARITY-R014.1, R014.2, R015.1 and R015.2. Those rows are unimplemented and cite pre-split paths; research.md D2 and D3 give the current paths.
- The wording gate reports sentences above 20 words in long rows. It is advisory in this repository.

Result: **Pass**. Coverage 72/72.
