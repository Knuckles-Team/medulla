# MEDULLA-FEDERATED-QUERY — Quickstart

Governing plan: [plan.md](plan.md). Each scenario runs one served proof from the repository root.

## Run a federated read

Start a test server with the mock sources, then run the served federation journey.

```text
cargo nextest run -p medulla-server-served-tests
uv run pytest tests/query/test_virtual_mapping_pushdown_served.py::test_pushed_and_unpushed_filters_equal_rows
```

The scenario sends one `Uql` `FOREIGN` query with and without pushdown. It passes when both runs return equal rows.

## Run a retrieval round

```text
cargo nextest run --test retrieval_round_served
```

The scenario sends `Method::RetrievalRound` with a `sufficiency` and a `selection` part. It passes when the verdict, the selected ids and the premises match the fixture.

## Run the reasoning and analysis proofs

```text
uv run pytest tests/reasoning/test_aif_dung_served.py tests/query/test_live_artifact_served.py tests/query/test_search_task_synthesis_served.py
```

## Run a SPARQL update

```text
uv run pytest tests/federated/test_sparql_update_served.py::test_insert_then_select
```

The scenario sends `Method::ApplyMutation` with event type `SPARQL_HTTP_UPDATE_EVENT`, then reads the data back with a served query.
