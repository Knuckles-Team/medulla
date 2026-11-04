# MEDULLA-PLACEMENT — Quickstart

Governing plan: [plan.md](plan.md).

The shortest path that shows placement working is the served journey of MEDULLA-PLACEMENT-R001. It drives the real `Decide` dispatch, not the pure function.

```bash
uv run pytest src/server/handlers/decide/stat_tests/placement_tests.py -k "placement"
```

The ingest-worker grant journey:

```bash
uv run pytest tests/jobs/test_worker_grant_served.py -k "grant"
```

The association bound journey:

```bash
uv run pytest tests/jobs/test_association_submit_served.py
```

The capacity adjustment verdicts (owner: decision engine) replay the placement headroom premise:

```bash
uv run pytest src/server/handlers/decide/stat_tests/capacity_tests.py
```
