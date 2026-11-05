# MEDULLA-PLACEMENT — Quickstart

Governing plan: [plan.md](plan.md).

The shortest path that shows placement working is the served journey of MEDULLA-PLACEMENT-R001. It drives the real `Decide` dispatch, not the pure function.

```bash
cargo test -p medulla-h-decide placement_decide_then_assemble_journey
```

The pure decision functions that already exist run as crate tests:

```bash
cargo test -p medulla-decision-types placement
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
cargo test -p medulla-h-decide capacity_proposal_reuses_placement_headroom
```
