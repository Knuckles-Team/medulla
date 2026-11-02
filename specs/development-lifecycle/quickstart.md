# MEDULLA-SDLC — Quickstart

Governing plan: [plan.md](plan.md).

The shortest path that shows autonomous delivery working is the served journey of R019.15. It runs against a loopback engine with recorded code-host events.

```bash
uv run pytest tests/sdlc/test_delivery_served.py::test_slice_to_merged_proposal_through_feedback
```

The pure decisions run as crate tests:

```bash
cargo test -p medulla-sdlc --test delivery_arming --test delivery_stage --test delivery_feedback
```
