# G7 split manifests (MEDULLA-CONTRACT-R056.11)

One manifest per source crate: `[[cut]]` item cuts first (in order), then `[[move]]` module
moves (top-down, highest rank first), then `[notes]`. `boundaries.toml` already carries
every move, so `move_leaf_modules.py` and `route_rows.py` agree with the manifests.

Per manifest, on its own branch:

1. `python3 scripts/crate_split/g7/check.py scripts/crate_split/g7/<crate>.toml` must print
   `violations 0`. If not, stop and report.
2. Apply the cuts in order (`itemmove.py` for item lists, `git mv` for file cuts; a `port`
   cut is its own commit). Build the touched crates.
3. Create each target crate, then run the moves in `order`. Build after each target crate.
4. `route_rows.py --update --check` passes.

## Order and parallelism

Group A runs now, each manifest on its own branch, all in parallel:
`medulla-query`, `medulla-compute`, `medulla-numeric`, `medulla-rdf`, `medulla-storage`,
`medulla-agent-types`, `medulla-catalog-types`, `medulla-decision-types`.

Two pairs share target crates. Land the first of each pair before the second rebases:
- `medulla-numeric` after `medulla-compute` (both feed `medulla-finance`,
  `medulla-finance-quant` and `medulla-decision-stats`).
- `medulla-rdf` and `medulla-compute` both feed `medulla-reasoner`: land either first.

`medulla-plan` runs in Group A too, but land its `port` cut (`exec::foreign`) first as a
separate PR.

Group B is after the chain. It waits for the facade move chain:
`medulla-store`, `medulla-persistence` and `medulla-raft` (parallel with each other), then
`medulla-server-api`. `medulla-server-api` is partial. Its last cut is `open`: the
`ServerState` and seam-trait SCC (52k lines) needs a seam-types design before any move.
Sonnet executors stop at that cut.

## Regenerating

    python3 scripts/crate_split/g7/modgraph.py <crate> <expand...> > <dir>/g-<x>.json
    python3 scripts/crate_split/g7/gen_manifests.py $PWD scripts/crate_split/g7/cuts.json \
        scripts/crate_split/g7/notes.json scripts/crate_split/g7 <dir>
    python3 scripts/crate_split/g7/apply_boundaries.py
    PYTHONPATH=<pipelines> python3 scripts/crate_split/route_rows.py --update --check

Inputs are `rules/c-*.py` (unit to target and rank), `cuts.json`, `notes.json` and
`new_crates.json` (new crate blocks, levels, chain, extra moves).
